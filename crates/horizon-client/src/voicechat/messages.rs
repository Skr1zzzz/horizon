//! Voice Messages implemented from a written interoperability description.
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};
use rusty_opus::{Application, OpusDecoder, OpusEncoder};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub const PREFIX: &str = "voicemessages:";
pub const RECEIVERS: &[u8] = b"voicemessages:config_v0\0voicemessages:targets\0voicemessages:voice_message_chunk_s2c\0voicemessages:voice_message_end_s2c";
#[derive(Debug, PartialEq)]
pub enum Payload {
    Version(String),
    Config(i32),
    Targets(Vec<String>),
    Chunk(Option<u128>, Vec<Vec<u8>>),
    End(Option<u128>, String),
}
impl Payload {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        match self {
            Self::Version(s) => string(&mut w, s, 128)?,
            Self::Config(ms) => w.varint(*ms),
            Self::Targets(list) => {
                w.varint(list.len() as i32);
                for s in list {
                    string(&mut w, s, 64)?;
                }
            }
            Self::Chunk(id, frames) => {
                if let Some(id) = id {
                    w.uuid(*id);
                }
                if frames.len() > 15000 {
                    return Err(invalid());
                }
                w.varint(frames.len() as i32);
                for f in frames {
                    if f.len() > 256 {
                        return Err(invalid());
                    }
                    w.byte_array(f);
                }
            }
            Self::End(id, s) => {
                if let Some(id) = id {
                    w.uuid(*id);
                }
                string(&mut w, s, 64)?;
            }
        }
        Ok(w.data)
    }
    pub fn decode(channel: &str, bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let p = match channel.strip_prefix(PREFIX).unwrap_or(channel) {
            "version" => Self::Version(r.string(128)?),
            "config_v0" => Self::Config(r.varint()?),
            "targets" => {
                let n = r.count(bytes.len())?;
                let mut v = Vec::new();
                for _ in 0..n {
                    v.push(r.string(64)?);
                }
                Self::Targets(v)
            }
            c @ ("voice_message_chunk_c2s" | "voice_message_chunk_s2c") => {
                let id = if c.ends_with("s2c") {
                    Some(r.uuid()?)
                } else {
                    None
                };
                let n = r.count(15000)?;
                let mut frames = Vec::new();
                for _ in 0..n {
                    frames.push(r.byte_array(256)?);
                }
                Self::Chunk(id, frames)
            }
            c @ ("voice_message_end_c2s" | "voice_message_end_s2c") => {
                let id = if c.ends_with("s2c") {
                    Some(r.uuid()?)
                } else {
                    None
                };
                Self::End(id, r.string(64)?)
            }
            _ => return Err(invalid()),
        };
        if r.remaining() != 0 {
            return Err(invalid());
        }
        Ok(p)
    }
}
fn invalid() -> Error {
    Error::new("Invalid voice message payload")
}
fn string(w: &mut Writer, s: &str, max: usize) -> Result<()> {
    if s.encode_utf16().count() > max || s.len() > 3 * max {
        return Err(invalid());
    }
    w.string(s);
    Ok(())
}
pub fn encode_audio(pcm: &[f32], limit: usize) -> Result<Vec<Vec<u8>>> {
    if pcm.is_empty() || pcm.len().div_ceil(960) > limit {
        return Err(invalid());
    }
    let mut encoder = OpusEncoder::new(48000, 1, Application::Voip).map_err(|_| invalid())?;
    let mut out = Vec::new();
    for f in pcm.chunks(960) {
        let mut frame = [0.0; 960];
        frame[..f.len()].copy_from_slice(f);
        let mut packet = vec![0; 256];
        let n = encoder
            .encode(&frame, 960, &mut packet)
            .map_err(|_| invalid())?;
        packet.truncate(n);
        out.push(packet);
    }
    Ok(out)
}
pub fn upload(frames: Vec<Vec<u8>>, target: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut chunk = Vec::new();
    let mut size = 0;
    for f in frames {
        if f.len() > 256 {
            return Err(invalid());
        }
        if size + f.len() + 2 > 31743 || chunk.len() == 15000 {
            out.push((
                format!("{PREFIX}voice_message_chunk_c2s"),
                Payload::Chunk(None, std::mem::take(&mut chunk)).encode()?,
            ));
            size = 0;
        }
        size += f.len() + 2;
        chunk.push(f);
    }
    if chunk.is_empty() && out.is_empty() {
        return Err(invalid());
    }
    if !chunk.is_empty() {
        out.push((
            format!("{PREFIX}voice_message_chunk_c2s"),
            Payload::Chunk(None, chunk).encode()?,
        ));
    }
    out.push((
        format!("{PREFIX}voice_message_end_c2s"),
        Payload::End(None, target.into()).encode()?,
    ));
    Ok(out)
}
struct Pending {
    start: Instant,
    frames: Vec<Vec<u8>>,
}
#[derive(Default)]
pub struct State {
    pub announced: bool,
    pub available: bool,
    pub max_frames: Option<usize>,
    pub targets: Option<Vec<String>>,
    pub wav_sent: bool,
    pending: HashMap<u128, Pending>,
}
impl State {
    pub fn ready(&self) -> bool {
        self.available && self.announced && self.max_frames.is_some() && self.targets.is_some()
    }
    pub fn receive(
        &mut self,
        p: Payload,
        now: Instant,
    ) -> Result<Option<(u128, String, Vec<f32>, usize)>> {
        self.pending
            .retain(|_, v| now.duration_since(v.start) <= Duration::from_secs(5));
        match p {
            Payload::Config(ms) if (3000..=300000).contains(&ms) => {
                self.max_frames = Some(ms as usize / 20)
            }
            Payload::Targets(t) => self.targets = Some(t),
            Payload::Chunk(Some(id), frames) => {
                if !self.pending.contains_key(&id) && self.pending.len() >= 128 {
                    return Err(invalid());
                }
                let v = self.pending.entry(id).or_insert(Pending {
                    start: now,
                    frames: Vec::new(),
                });
                if v.frames.len() + frames.len() > self.max_frames.unwrap_or(15000) {
                    self.pending.remove(&id);
                    return Err(invalid());
                }
                v.frames.extend(frames);
            }
            Payload::End(Some(id), target) => {
                if let Some(v) = self.pending.remove(&id).filter(|v| !v.frames.is_empty()) {
                    let mut decoder = OpusDecoder::new(48000, 1).map_err(|_| invalid())?;
                    let mut pcm = Vec::new();
                    for f in &v.frames {
                        let mut frame = vec![0.0; 960];
                        decoder.decode(f, 960, &mut frame).map_err(|_| invalid())?;
                        pcm.extend(frame);
                    }
                    return Ok(Some((id, target, pcm, v.frames.len())));
                }
            }
            _ => return Err(invalid()),
        }
        Ok(None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_message_wire_vectors() {
        let cases = [
            (
                "version",
                Payload::Version("1.0.12-1.21.11".into()),
                b"\x0e1.0.12-1.21.11".to_vec(),
            ),
            ("config_v0", Payload::Config(90000), vec![0x90, 0xbf, 0x05]),
            (
                "targets",
                Payload::Targets(vec!["all".into(), "team".into()]),
                b"\x02\x03all\x04team".to_vec(),
            ),
            (
                "voice_message_chunk_c2s",
                Payload::Chunk(None, vec![vec![1, 2], vec![3]]),
                vec![2, 2, 1, 2, 1, 3],
            ),
            (
                "voice_message_end_c2s",
                Payload::End(None, "all".into()),
                b"\x03all".to_vec(),
            ),
            (
                "voice_message_chunk_s2c",
                Payload::Chunk(Some(1), vec![vec![7]]),
                [1u128.to_be_bytes().to_vec(), vec![1, 1, 7]].concat(),
            ),
            (
                "voice_message_end_s2c",
                Payload::End(Some(1), "all".into()),
                [1u128.to_be_bytes().to_vec(), b"\x03all".to_vec()].concat(),
            ),
        ];
        for (c, p, b) in cases {
            assert_eq!(p.encode().unwrap(), b);
            assert_eq!(Payload::decode(c, &b).unwrap(), p);
        }
        assert!(Payload::Chunk(None, vec![vec![0; 257]]).encode().is_err());
        assert!(Payload::Chunk(None, vec![vec![]; 15001]).encode().is_err());
    }
    #[test]
    fn voice_message_chunk_limits_and_duration() {
        let frames = vec![vec![1; 256]; 4500];
        let packets = upload(frames.clone(), "all").unwrap();
        let mut joined = Vec::new();
        for (c, b) in &packets[..packets.len() - 1] {
            assert!(b.len() < 32767);
            if let Payload::Chunk(None, f) = Payload::decode(c, b).unwrap() {
                assert!(f.iter().map(|f| f.len() + 2).sum::<usize>() <= 31743);
                joined.extend(f);
            }
        }
        assert_eq!(joined, frames);
        assert_eq!(encode_audio(&vec![0.0; 960 * 150], 150).unwrap().len(), 150);
        assert!(encode_audio(&vec![0.0; 960 * 150 + 1], 150).is_err());
    }
    #[test]
    fn voice_message_assembly_and_expiry() {
        let now = Instant::now();
        let mut s = State::default();
        let f = encode_audio(&vec![0.1; 1920], 4500).unwrap();
        for id in [1, 2] {
            for frame in &f {
                s.receive(Payload::Chunk(Some(id), vec![frame.clone()]), now)
                    .unwrap();
            }
        }
        let (_, _, pcm, n) = s
            .receive(Payload::End(Some(1), "all".into()), now)
            .unwrap()
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(pcm.len(), 1920);
        assert!(
            s.receive(
                Payload::End(Some(2), "all".into()),
                now + Duration::from_secs(6)
            )
            .unwrap()
            .is_none()
        );
    }
}
