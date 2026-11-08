//! Protocol implemented from a written interoperability description, not SVC source.
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};

#[derive(Clone, Debug, PartialEq)]
pub struct Secret {
    pub key: [u8; 16],
    pub port: u16,
    pub player: u128,
    pub codec: u8,
    pub mtu: i32,
    pub distance: f64,
    pub keep_alive: u64,
    pub groups: bool,
    pub host: String,
    pub recording: bool,
}
fn invalid() -> Error {
    Error::new("Invalid voicechat packet")
}
fn finish(r: &Reader<'_>) -> Result<()> {
    if r.remaining() == 0 {
        Ok(())
    } else {
        Err(invalid())
    }
}
impl Secret {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let key = r.bytes(16)?.try_into().unwrap();
        let port = u16::try_from(r.i32()?)
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(invalid)?;
        let player = r.uuid()?;
        let codec = r.u8()?;
        let mtu = r.i32()?;
        let distance = r.f64()?;
        let keep_alive = r.i32()?;
        if codec > 2
            || !(1..=1275).contains(&mtu)
            || !distance.is_finite()
            || distance <= 0.0
            || keep_alive <= 0
        {
            return Err(invalid());
        }
        let s = Self {
            key,
            port,
            player,
            codec,
            mtu,
            distance,
            keep_alive: keep_alive as u64,
            groups: r.bool()?,
            host: r.string(32767)?,
            recording: r.bool()?,
        };
        finish(&r)?;
        Ok(s)
    }
    pub fn address(&self, server: &str) -> Result<(String, u16)> {
        voice_host(&self.host, server, self.port)
    }
}
pub fn voice_host(host: &str, server: &str, default: u16) -> Result<(String, u16)> {
    if host.is_empty() {
        return Ok((server.into(), default));
    }
    if let Ok(port) = host.parse::<u16>() {
        return if port > 0 {
            Ok((server.into(), port))
        } else {
            Err(invalid())
        };
    }
    let (name, port) = if let Some(rest) = host.strip_prefix('[') {
        let (name, suffix) = rest.split_once(']').ok_or_else(invalid)?;
        let port = if suffix.is_empty() {
            default
        } else {
            suffix
                .strip_prefix(':')
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?
        };
        (name, port)
    } else if let Some((name, port)) = host.split_once(':') {
        (name, port.parse().map_err(|_| invalid())?)
    } else {
        (host, default)
    };
    if name.is_empty() || port == 0 || name.contains(['/', '@', ' ', ':']) && !host.starts_with('[')
    {
        return Err(invalid());
    }
    Ok((name.into(), port))
}
#[derive(Clone, Debug, PartialEq)]
pub enum Placement {
    Player,
    Group,
    Location([f64; 3]),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub channel: u128,
    pub sender: u128,
    pub placement: Placement,
    pub opus: Vec<u8>,
    pub sequence: i64,
    pub distance: f32,
    pub flags: u8,
    pub category: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Mic {
        opus: Vec<u8>,
        sequence: i64,
        whispering: bool,
    },
    Sound(Sound),
    Authenticate {
        player: u128,
        key: [u8; 16],
    },
    AuthenticateAck,
    Ping {
        id: u128,
        timestamp: i64,
    },
    KeepAlive,
    ConnectionCheck,
    ConnectionCheckAck,
}
impl Packet {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let p = match r.u8()? {
            1 => Self::Mic {
                opus: r.byte_array(1275)?,
                sequence: r.i64()?,
                whispering: r.bool()?,
            },
            id @ 2..=4 => {
                let channel = r.uuid()?;
                let sender = r.uuid()?;
                let placement = match id {
                    2 => Placement::Player,
                    3 => Placement::Group,
                    _ => Placement::Location([r.f64()?, r.f64()?, r.f64()?]),
                };
                let opus = r.byte_array(1275)?;
                let sequence = r.i64()?;
                let distance = if id == 3 { 0.0 } else { r.f32()? };
                let flags = r.u8()?;
                let category = if flags & 2 != 0 {
                    Some(r.string(16)?)
                } else {
                    None
                };
                if !distance.is_finite()
                    || distance < 0.0
                    || matches!(&placement,Placement::Location(at) if at.iter().any(|x| !x.is_finite()))
                {
                    return Err(invalid());
                }
                Self::Sound(Sound {
                    channel,
                    sender,
                    placement,
                    opus,
                    sequence,
                    distance,
                    flags,
                    category,
                })
            }
            5 => Self::Authenticate {
                player: r.uuid()?,
                key: r.bytes(16)?.try_into().unwrap(),
            },
            6 => Self::AuthenticateAck,
            7 => Self::Ping {
                id: r.uuid()?,
                timestamp: r.i64()?,
            },
            8 => Self::KeepAlive,
            9 => Self::ConnectionCheck,
            10 => Self::ConnectionCheckAck,
            _ => return Err(invalid()),
        };
        finish(&r)?;
        Ok(p)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Mic {
                opus,
                sequence,
                whispering,
            } => {
                w.u8(1);
                w.byte_array(opus);
                w.i64(*sequence);
                w.bool(*whispering)
            }
            Self::Sound(s) => {
                w.u8(match s.placement {
                    Placement::Player => 2,
                    Placement::Group => 3,
                    Placement::Location(_) => 4,
                });
                w.uuid(s.channel);
                w.uuid(s.sender);
                if let Placement::Location(at) = s.placement {
                    for v in at {
                        w.f64(v)
                    }
                }
                w.byte_array(&s.opus);
                w.i64(s.sequence);
                if s.placement != Placement::Group {
                    w.f32(s.distance)
                }
                w.u8(s.flags);
                if s.flags & 2 != 0 {
                    w.string(s.category.as_deref().unwrap_or(""))
                }
            }
            Self::Authenticate { player, key } => {
                w.u8(5);
                w.uuid(*player);
                w.data.extend_from_slice(key)
            }
            Self::AuthenticateAck => w.u8(6),
            Self::Ping { id, timestamp } => {
                w.u8(7);
                w.uuid(*id);
                w.i64(*timestamp)
            }
            Self::KeepAlive => w.u8(8),
            Self::ConnectionCheck => w.u8(9),
            Self::ConnectionCheckAck => w.u8(10),
        }
        w.data
    }
}
pub struct Crypto(LessSafeKey);
impl Crypto {
    pub fn new(key: &[u8; 16]) -> Self {
        Self(LessSafeKey::new(
            UnboundKey::new(&aead::AES_128_GCM, key).unwrap(),
        ))
    }
    pub fn seal(&self, iv: [u8; 12], plain: &[u8]) -> Option<Vec<u8>> {
        let mut data = plain.to_vec();
        self.0
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(iv), Aad::empty(), &mut data)
            .ok()?;
        let mut out = iv.to_vec();
        out.extend(data);
        Some(out)
    }
    pub fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
        let iv = data.get(..12)?.try_into().ok()?;
        let mut body = data[12..].to_vec();
        Some(
            self.0
                .open_in_place(Nonce::assume_unique_for_key(iv), Aad::empty(), &mut body)
                .ok()?
                .to_vec(),
        )
    }
}
/// UI state is parsed but not retained until there is an icon surface.
pub fn state(channel: &str, data: &[u8]) -> Result<()> {
    let mut r = Reader::new(data);
    if channel == "voicechat:remove_state" {
        r.uuid()?;
    } else {
        let count = if channel == "voicechat:states" {
            usize::try_from(r.i32()?)
                .ok()
                .filter(|n| *n <= 4096)
                .ok_or_else(invalid)?
        } else {
            1
        };
        for _ in 0..count {
            r.bool()?;
            r.bool()?;
            r.uuid()?;
            r.string(32767)?;
            if r.bool()? {
                r.uuid()?;
            }
        }
    }
    finish(&r)
}
