use super::wire::Packet;
use rusty_opus::{Application, OpusEncoder};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};

#[derive(Default)]
pub struct Controls {
    pub held: AtomicBool,
    pub muted: AtomicBool,
    pub disabled: AtomicBool,
    pub transmitting: AtomicBool,
    pub recording_message: AtomicBool,
}
pub struct Config {
    pub device: String,
    pub activation: bool,
    pub threshold: f32,
}

// Integer phase preserves timing across callback boundaries and avoids cumulative drift.
pub struct Convert {
    rate: u64,
    channels: usize,
    partial: Vec<f32>,
    previous: Option<f32>,
    index: u64,
    next: u64,
    frame: Vec<f32>,
}
impl Convert {
    pub fn new(rate: u32, channels: usize) -> Self {
        Self {
            rate: rate as u64,
            channels,
            partial: Vec::new(),
            previous: None,
            index: 0,
            next: 0,
            frame: Vec::new(),
        }
    }
    pub fn push(&mut self, samples: &[f32], mut emit: impl FnMut(Vec<f32>)) {
        for &s in samples {
            self.partial.push(if s.is_finite() { s } else { 0.0 });
            if self.partial.len() != self.channels {
                continue;
            }
            let mono = self.partial.iter().sum::<f32>() / self.channels as f32;
            self.partial.clear();
            if let Some(prev) = self.previous {
                while self.next <= self.index * 48000 {
                    let fraction = (self.next - (self.index - 1) * 48000) as f32 / 48000.0;
                    self.frame.push(prev + (mono - prev) * fraction);
                    self.next += self.rate;
                    if self.frame.len() == 960 {
                        emit(std::mem::take(&mut self.frame));
                    }
                }
            } else {
                self.frame.push(mono);
                self.next = self.rate;
            }
            self.previous = Some(mono);
            self.index += 1;
        }
    }
}
#[derive(Default)]
pub struct Gate {
    remaining: usize,
}
impl Gate {
    pub fn active(&mut self, frame: &[f32], threshold: f32) -> bool {
        let power = frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32;
        if power >= 10f32.powf(threshold / 10.0) {
            self.remaining = 12000;
            return true;
        }
        let active = self.remaining > 0;
        self.remaining = self.remaining.saturating_sub(frame.len());
        active
    }
}
pub struct Sender {
    encoder: OpusEncoder,
    seq: i64,
    burst: bool,
    mtu: usize,
    gate: Gate,
    pub frames: Receiver<Vec<f32>>,
    pub controls: Arc<Controls>,
    config: Config,
    wav: Option<(Vec<f32>, usize)>,
    next: std::time::Instant,
    #[cfg(feature = "audio")]
    _stream: Option<cpal::Stream>,
}
impl Sender {
    pub fn new(
        codec: u8,
        mtu: usize,
        controls: Arc<Controls>,
        config: Config,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let application = match codec {
            0 => Application::Voip,
            1 => Application::Audio,
            _ => Application::RestrictedLowDelay,
        };
        let encoder = OpusEncoder::new(48000, 1, application)?;
        let (tx, frames) = mpsc::sync_channel(5);
        let wav = std::env::var_os("HORIZON_VOICECHAT_MIC_WAV")
            .map(|p| read_wav(std::path::Path::new(&p)))
            .transpose()?
            .map(|v| (v, 0));
        #[cfg(feature = "audio")]
        let stream = if wav.is_none() && !cfg!(test) {
            match capture(&config.device, tx) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::warn!("voicechat: microphone: {e}");
                    None
                }
            }
        } else {
            None
        };
        #[cfg(not(feature = "audio"))]
        let _ = tx;
        Ok(Self {
            encoder,
            seq: 0,
            burst: false,
            mtu,
            gate: Gate::default(),
            frames,
            controls,
            config,
            wav,
            next: std::time::Instant::now(),
            #[cfg(feature = "audio")]
            _stream: stream,
        })
    }
    pub fn poll(&mut self, mut send: impl FnMut(Packet) -> bool) -> usize {
        let blocked = self.controls.recording_message.load(Ordering::Relaxed)
            || self.controls.muted.load(Ordering::Relaxed)
            || self.controls.disabled.load(Ordering::Relaxed);
        let mut frames: Vec<_> = self.frames.try_iter().collect();
        if let Some((wav, at)) = self.wav.as_mut() {
            let now = std::time::Instant::now();
            if now >= self.next {
                frames.push(
                    (0..960)
                        .map(|_| {
                            let s = wav[*at];
                            *at = (*at + 1) % wav.len();
                            s
                        })
                        .collect(),
                );
                // Do not catch up in a burst after a stalled network thread.
                self.next = now + std::time::Duration::from_millis(20);
            }
        }
        let held = self.controls.held.load(Ordering::Relaxed);
        let mut count = 0;
        if blocked || (!held && !self.config.activation && self.wav.is_none()) {
            self.end(&mut send);
        }
        for frame in frames {
            let active = !blocked
                && (self.wav.is_some()
                    || if self.config.activation {
                        self.gate.active(&frame, self.config.threshold)
                    } else {
                        held
                    });
            if !active {
                self.end(&mut send);
                continue;
            }
            let mut opus = vec![0; self.mtu];
            if let Ok(n) = self.encoder.encode(&frame, 960, &mut opus) {
                opus.truncate(n);
                if send(self.packet(opus)) {
                    self.seq = self.seq.wrapping_add(1);
                    self.burst = true;
                    count += 1;
                }
            }
        }
        self.controls
            .transmitting
            .store(self.burst, Ordering::Relaxed);
        count
    }
    fn packet(&self, opus: Vec<u8>) -> Packet {
        Packet::Mic {
            opus,
            sequence: self.seq,
            whispering: false,
        }
    }
    fn end(&mut self, send: &mut impl FnMut(Packet) -> bool) {
        if self.burst && send(self.packet(Vec::new())) {
            self.seq = self.seq.wrapping_add(1);
            self.burst = false;
        }
    }
}
#[cfg(feature = "audio")]
pub type CaptureStream = cpal::Stream;
#[cfg(feature = "audio")]
pub fn capture(
    name: &str,
    tx: SyncSender<Vec<f32>>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    capture_with_errors(name, tx, None)
}
#[cfg(feature = "audio")]
pub fn capture_with_errors(
    name: &str,
    tx: SyncSender<Vec<f32>>,
    errors: Option<SyncSender<String>>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = if name.is_empty() {
        host.default_input_device()
    } else {
        host.input_devices()?
            .find(|d| d.name().is_ok_and(|n| n == name))
    }
    .ok_or("input device unavailable")?;
    let config = device.default_input_config()?;
    macro_rules! build { ($($format:ident => $ty:ty),*) => { match config.sample_format() { $(cpal::SampleFormat::$format => input::<$ty>(&device, &config, tx, errors),)* _ => Err("unsupported input format".into()) } }; }
    let stream = build!(I8=>i8,I16=>i16,I32=>i32,I64=>i64,U8=>u8,U16=>u16,U32=>u32,U64=>u64,F32=>f32,F64=>f64)?;
    stream.play()?;
    Ok(stream)
}
#[cfg(feature = "audio")]
fn input<T: cpal::SizedSample>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    tx: SyncSender<Vec<f32>>,
    errors: Option<SyncSender<String>>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>>
where
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    let mut convert = Convert::new(config.sample_rate().0, config.channels() as usize);
    let mut logged = false;
    Ok(device.build_input_stream(
        &config.config(),
        move |data: &[T], _| {
            let samples: Vec<f32> = data
                .iter()
                .map(|s| cpal::Sample::to_sample::<f32>(*s))
                .collect();
            convert.push(&samples, |frame| {
                let _ = tx.try_send(frame);
            });
        },
        move |e| {
            if !logged {
                if let Some(tx) = &errors {
                    let _ = tx.try_send(e.to_string());
                }
                log::warn!("voicechat: microphone: {e}");
                logged = true;
            }
        },
        None,
    )?)
}
pub fn read_wav(path: &std::path::Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV".into());
    }
    let mut at = 12;
    let mut format = None;
    let mut data = None;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into()?) as usize;
        let chunk = bytes.get(at + 8..at + 8 + size).ok_or("truncated WAV")?;
        match &bytes[at..at + 4] {
            b"fmt " if size >= 16 => {
                format = Some((
                    u16::from_le_bytes(chunk[..2].try_into()?),
                    u16::from_le_bytes(chunk[2..4].try_into()?),
                    u32::from_le_bytes(chunk[4..8].try_into()?),
                    u16::from_le_bytes(chunk[14..16].try_into()?),
                ))
            }
            b"data" => data = Some(chunk),
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    let (kind, channels, rate, bits) = format.ok_or("missing WAV format")?;
    if channels == 0 || rate == 0 {
        return Err("invalid WAV rate/channels".into());
    }
    let data = data.ok_or("missing WAV data")?;
    let samples: Vec<f32> = match (kind, bits) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes(s.try_into().unwrap()) as f32 / 32768.0)
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
            .collect(),
        _ => return Err("WAV must be PCM16 or float32".into()),
    };
    let mut out = Vec::new();
    let mut convert = Convert::new(rate, channels as usize);
    convert.push(&samples, |frame| out.extend(frame));
    out.extend(convert.frame);
    if out.is_empty() {
        return Err("empty WAV".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sine_downmix_resample_and_frame() {
        for rate in [24000, 44100, 48000, 96000] {
            let input: Vec<_> = (0..=rate)
                .flat_map(|i| {
                    let s = (i as f32 * std::f32::consts::TAU * 440.0 / rate as f32).sin();
                    [s * 0.6, s * 0.2]
                })
                .collect();
            let mut c = Convert::new(rate, 2);
            let mut output = Vec::new();
            for chunk in input.chunks(317) {
                c.push(chunk, |f| {
                    assert_eq!(f.len(), 960);
                    output.extend(f);
                });
            }
            assert_eq!(output.len(), 48000);
            let rms = (output.iter().map(|s| s * s).sum::<f32>() / 48000.0).sqrt();
            assert!((rms - 0.4 / 2f32.sqrt()).abs() < 0.002);
            assert_eq!(
                output
                    .windows(2)
                    .filter(|p| p[0] <= 0.0 && p[1] > 0.0)
                    .count(),
                440
            );
        }
    }
    #[test]
    fn activation_hang() {
        let mut gate = Gate::default();
        assert!(!gate.active(&[0.0; 960], -50.0));
        assert!(gate.active(&[0.01; 960], -50.0));
        for _ in 0..13 {
            assert!(gate.active(&[0.0; 960], -50.0));
        }
        assert!(!gate.active(&[0.0; 960], -50.0));
    }
    #[test]
    fn sequence_and_end_burst() {
        let controls = Arc::new(Controls::default());
        controls.held.store(true, Ordering::Relaxed);
        let mut sender = Sender::new(
            0,
            1275,
            controls.clone(),
            Config {
                device: "nonexistent test device".into(),
                activation: false,
                threshold: -50.0,
            },
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(3);
        sender.frames = rx;
        tx.send(vec![0.1; 960]).unwrap();
        tx.send(vec![0.2; 960]).unwrap();
        let mut packets = Vec::new();
        sender.poll(|p| {
            packets.push(p);
            true
        });
        controls.held.store(false, Ordering::Relaxed);
        sender.poll(|p| {
            packets.push(p);
            true
        });
        sender.poll(|_| panic!("duplicate end"));
        assert_eq!(packets.len(), 3);
        for (i, p) in packets.iter().enumerate() {
            assert_eq!(Packet::decode(&p.encode()).unwrap(), *p);
            assert!(matches!(p,Packet::Mic{sequence,whispering:false,..} if *sequence == i as i64));
        }
        assert_eq!(packets[2].encode(), vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0]);
    }
}
