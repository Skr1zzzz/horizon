//! Clean-room protocol implemented from a written interoperability description.
pub mod sender;
pub mod wire;
use horizon_protocol::buf::{Reader, Writer};
use rusty_opus::OpusDecoder;
use std::sync::{Arc, atomic::Ordering};
use std::{
    collections::{BTreeMap, HashMap},
    net::{ToSocketAddrs, UdpSocket},
    sync::mpsc::{self, Receiver, SyncSender},
    time::{Duration, Instant},
};
use wire::{Packet, Secret, Sound};

pub enum Event {
    Connected,
    Pcm(Sound, Vec<f32>),
}
pub struct Connection {
    stop: SyncSender<()>,
    pub events: Receiver<Event>,
    pub controls: Arc<sender::Controls>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
    }
}
impl Connection {
    pub fn start(secret: Secret, server: String, config: sender::Config) -> Self {
        let (stop, halt) = mpsc::sync_channel(1);
        let (tx, events) = mpsc::sync_channel(100);
        let controls = Arc::new(sender::Controls::default());
        let shared = controls.clone();
        std::thread::Builder::new()
            .name("voicechat UDP".into())
            .spawn(move || {
                if let Err(e) = run(secret, &server, halt, tx, shared, config) {
                    log::warn!("voicechat: {e}");
                }
            })
            .expect("voicechat thread");
        Self {
            stop,
            events,
            controls,
        }
    }
}
fn diagnostic(message: &str) {
    if std::env::var_os("HORIZON_VOICECHAT_LOG").is_some_and(|v| v == "1") {
        log::info!("voicechat: {message}");
    }
}
#[derive(Default)]
pub struct Reorder {
    next: Option<i64>,
    pending: BTreeMap<i64, Sound>,
}
impl Reorder {
    fn push(&mut self, s: Sound) -> Vec<Option<Sound>> {
        let next = self.next.get_or_insert(s.sequence);
        if s.sequence < *next {
            return Vec::new();
        }
        self.pending.entry(s.sequence).or_insert(s);
        self.drain(false)
    }
    fn drain(&mut self, expired: bool) -> Vec<Option<Sound>> {
        let mut out = Vec::new();
        let Some(mut n) = self.next else { return out };
        while let Some((&first, _)) = self.pending.first_key_value() {
            if first != n && self.pending.len() <= 3 && !expired {
                break;
            }
            // Bound PLC work for discontinuities instead of decoding billions of lost frames.
            if first.saturating_sub(n) > 50 {
                n = first;
            }
            if let Some(s) = self.pending.remove(&n) {
                out.push(Some(s));
            } else {
                out.push(None)
            }
            n = n.saturating_add(1);
        }
        self.next = Some(n);
        out
    }
}
struct Channel {
    reorder: Reorder,
    decoder: OpusDecoder,
    last: Sound,
    arrival: Instant,
}
fn run(
    secret: Secret,
    server: &str,
    halt: Receiver<()>,
    tx: SyncSender<Event>,
    controls: Arc<sender::Controls>,
    config: sender::Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let (host, port) = secret.address(server)?;
    diagnostic(&format!("secret port={port} host={host}"));
    let address = (host.as_str(), port)
        .to_socket_addrs()?
        .next()
        .ok_or("voice host has no address")?;
    let socket = UdpSocket::bind(if address.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })?;
    socket.connect(address)?;
    socket.set_read_timeout(Some(Duration::from_millis(10)))?;
    let crypto = wire::Crypto::new(&secret.key);
    let send = |packet: Packet| -> Option<()> {
        let mut iv = [0; 12];
        getrandom::fill(&mut iv).ok()?;
        let encrypted = crypto.seal(iv, &packet.encode())?;
        let mut w = Writer::new();
        w.u8(255);
        w.uuid(secret.player);
        w.byte_array(&encrypted);
        socket.send(&w.data).ok()?;
        Some(())
    };
    let mut authenticated = false;
    let mut connected = false;
    let mut retry = Instant::now() - Duration::from_secs(1);
    let mut alive = Instant::now();
    let mut channels: HashMap<u128, Channel> = HashMap::new();
    let mut buffer = [0; 65536];
    let mut stats: HashMap<u128, (usize, f32)> = HashMap::new();
    let mut report = Instant::now();
    let mut dump = Dump::new();
    let mut config = Some(config);
    let mut sender = None;
    let mut sent = 0;
    loop {
        match halt.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {}
            _ => break,
        }
        if !connected && retry.elapsed() >= Duration::from_secs(1) {
            send(if authenticated {
                Packet::ConnectionCheck
            } else {
                Packet::Authenticate {
                    player: secret.player,
                    key: secret.key,
                }
            });
            retry = Instant::now();
        }
        if alive.elapsed() > Duration::from_millis(secret.keep_alive.saturating_mul(10)) {
            break;
        }
        if let Some(s) = sender.as_mut() {
            sent += sender::Sender::poll(s, |p| send(p).is_some());
        }
        if let Ok(n) = socket.recv(&mut buffer) {
            let mut r = Reader::new(&buffer[..n]);
            let packet = (|| {
                if r.u8().ok()? != 255 {
                    return None;
                }
                let encrypted = r.byte_array(65507).ok()?;
                if r.remaining() != 0 {
                    return None;
                }
                Packet::decode(&crypto.open(&encrypted)?).ok()
            })();
            match packet {
                Some(Packet::AuthenticateAck) if !authenticated => {
                    authenticated = true;
                    retry = Instant::now() - Duration::from_secs(1);
                    diagnostic("authenticated");
                }
                Some(Packet::ConnectionCheckAck) if authenticated && !connected => {
                    connected = true;
                    alive = Instant::now();
                    diagnostic("connected");
                    match sender::Sender::new(
                        secret.codec,
                        secret.mtu as usize,
                        controls.clone(),
                        config.take().unwrap(),
                    ) {
                        Ok(s) => sender = Some(s),
                        Err(e) => log::warn!("voicechat: sender: {e}"),
                    }
                    let _ = tx.try_send(Event::Connected);
                }
                Some(Packet::KeepAlive) => {
                    alive = Instant::now();
                    send(Packet::KeepAlive);
                }
                Some(p @ Packet::Ping { .. }) => {
                    send(p);
                }
                Some(Packet::Sound(s))
                    if connected && !controls.disabled.load(Ordering::Relaxed) =>
                {
                    if channels.len() < 128 || channels.contains_key(&s.channel) {
                        let c = channels.entry(s.channel).or_insert_with(|| Channel {
                            reorder: Reorder::default(),
                            decoder: OpusDecoder::new(48000, 1).expect("valid Opus format"),
                            last: s.clone(),
                            arrival: Instant::now(),
                        });
                        c.arrival = Instant::now();
                        let ready = c.reorder.push(s);
                        decode(c, ready, &tx, &mut stats, &mut dump);
                    }
                }
                _ => {}
            }
        }
        if controls.disabled.load(Ordering::Relaxed) {
            channels.clear();
        }
        for c in channels.values_mut() {
            if c.arrival.elapsed() > Duration::from_millis(60) {
                let ready = c.reorder.drain(true);
                decode(c, ready, &tx, &mut stats, &mut dump);
            }
        }
        channels.retain(|_, c| c.arrival.elapsed() < Duration::from_secs(30));
        dump.flush(false);
        if report.elapsed() >= Duration::from_secs(1) {
            for (id, (frames, peak)) in stats.drain() {
                diagnostic(&format!(
                    "heard {} frames={frames} peak={peak:.5}",
                    uuid(id)
                ));
            }
            diagnostic(&format!("sent {sent} frames"));
            sent = 0;
            report = Instant::now();
        }
    }
    dump.flush(true);
    Ok(())
}
fn uuid(id: u128) -> String {
    let s = format!("{id:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &s[..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..]
    )
}
fn decode(
    c: &mut Channel,
    ready: Vec<Option<Sound>>,
    tx: &SyncSender<Event>,
    stats: &mut HashMap<u128, (usize, f32)>,
    dump: &mut Dump,
) {
    for sound in ready {
        if let Some(s) = sound {
            if s.opus.is_empty() {
                c.decoder = OpusDecoder::new(48000, 1).expect("valid Opus format");
                continue;
            }
            c.last = s;
        } else {
            c.last.opus.clear();
        }
        let mut pcm = vec![0.0; 960];
        if c.decoder.decode(&c.last.opus, 960, &mut pcm).is_err() {
            continue;
        }
        let entry = stats.entry(c.last.sender).or_default();
        entry.0 += 1;
        entry.1 = pcm.iter().fold(entry.1, |p, s| p.max(s.abs()));
        dump.add(c.last.channel, &pcm);
        let _ = tx.try_send(Event::Pcm(c.last.clone(), pcm));
    }
}
// A wall-clock timeline lets overlapping senders sum, rather than concatenate in the WAV.
struct Dump {
    file: Option<std::fs::File>,
    start: Instant,
    last: Instant,
    base: usize,
    samples: Vec<f32>,
    ends: HashMap<u128, usize>,
}
impl Dump {
    fn new() -> Self {
        use std::io::Write;
        let file = std::env::var_os("HORIZON_VOICECHAT_DUMP").and_then(|p| {
            let path = std::path::PathBuf::from(p);
            let root = std::env::current_dir().ok()?;
            let absolute = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            // Diagnostics must not escape the worktree, including through a symlink.
            let parent = absolute.parent()?.canonicalize().ok()?;
            if !parent.starts_with(root.canonicalize().ok()?) || absolute.exists() {
                log::warn!("voicechat: dump must be a new file inside the working directory");
                return None;
            }
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(absolute)
                .ok()?;
            f.write_all(&[0; 44]).ok()?;
            Some(f)
        });
        Self {
            file,
            start: Instant::now(),
            last: Instant::now(),
            base: 0,
            samples: Vec::new(),
            ends: HashMap::new(),
        }
    }
    fn add(&mut self, id: u128, pcm: &[f32]) {
        if self.file.is_none() {
            return;
        }
        let now = (self.start.elapsed().as_secs_f64() * 48000.0) as usize;
        let end = self.ends.entry(id).or_insert(now);
        let at = (*end).max(now.saturating_sub(960)).max(self.base);
        *end = at + pcm.len();
        self.samples
            .resize(self.samples.len().max(*end - self.base), 0.0);
        for (i, s) in pcm.iter().enumerate() {
            self.samples[at - self.base + i] += s;
        }
    }
    fn flush(&mut self, all: bool) {
        use std::io::{Seek, SeekFrom, Write};
        if !all && self.last.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last = Instant::now();
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let count = if all {
            self.samples.len()
        } else {
            self.samples.len().saturating_sub(4800)
        };
        let bytes: Vec<u8> = self
            .samples
            .drain(..count)
            .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
            .collect();
        let result = (|| -> std::io::Result<()> {
            file.seek(SeekFrom::End(0))?;
            file.write_all(&bytes)?;
            self.base += count;
            let size = (self.base * 2).min(u32::MAX as usize - 36) as u32;
            let mut h = Vec::new();
            h.extend(b"RIFF");
            h.extend((size + 36).to_le_bytes());
            h.extend(b"WAVEfmt ");
            h.extend(16u32.to_le_bytes());
            h.extend(1u16.to_le_bytes());
            h.extend(1u16.to_le_bytes());
            h.extend(48000u32.to_le_bytes());
            h.extend(96000u32.to_le_bytes());
            h.extend(2u16.to_le_bytes());
            h.extend(16u16.to_le_bytes());
            h.extend(b"data");
            h.extend(size.to_le_bytes());
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&h)?;
            file.flush()
        })();
        if let Err(e) = result {
            log::warn!("voicechat: dump: {e}");
            self.file = None;
        }
    }
}
#[cfg(test)]
mod tests;
