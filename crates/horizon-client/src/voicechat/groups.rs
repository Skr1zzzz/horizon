//! Group payloads from the written compatibility-20 interface description only.
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: u128,
    pub name: String,
    pub password: bool,
    pub persistent: bool,
    pub hidden: bool,
    pub kind: i16,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub disabled: bool,
    pub disconnected: bool,
    pub id: u128,
    pub name: String,
    pub group: Option<u128>,
}
#[derive(Default)]
pub struct State {
    pub enabled: bool,
    pub groups: BTreeMap<u128, Group>,
    pub players: BTreeMap<u128, Player>,
    pub current: Option<u128>,
    pub wrong_password: bool,
}
fn invalid() -> Error {
    Error::new("Invalid voice group payload")
}
fn end(r: &Reader<'_>) -> Result<()> {
    if r.remaining() == 0 {
        Ok(())
    } else {
        Err(invalid())
    }
}
// Reject Unicode format controls as well as the Rust control category; these can disguise names.
pub fn valid(s: &str) -> bool {
    (1..=24).contains(&s.chars().count())
        && !s.starts_with(char::is_whitespace)
        && !s.chars().any(|c| {
            c.is_control()
                || matches!(c as u32,
            0xad | 0x61c | 0x6dd | 0x70f | 0x8e2 | 0x180e | 0xfeff | 0x110bd | 0x110cd | 0xe0001
            | 0x600..=0x605 | 0x890..=0x891 | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064
            | 0x2066..=0x206f | 0xfff9..=0xfffb | 0x13430..=0x13455 | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a | 0xe0020..=0xe007f)
        })
}
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Add(Group),
    Remove(u128),
    Joined(Option<u128>, bool),
}
pub fn decode(channel: &str, data: &[u8]) -> Result<Incoming> {
    let mut r = Reader::new(data);
    let p = match channel {
        "voicechat:add_group" => {
            let id = r.uuid()?;
            let name = r.string(512)?;
            let password = r.bool()?;
            let persistent = r.bool()?;
            let hidden = r.bool()?;
            let kind = r.i16()?;
            Incoming::Add(Group {
                id,
                name,
                password,
                persistent,
                hidden,
                kind: if (0..=2).contains(&kind) { kind } else { 0 },
            })
        }
        "voicechat:remove_group" => Incoming::Remove(r.uuid()?),
        "voicechat:joined_group" => {
            let id = if r.bool()? { Some(r.uuid()?) } else { None };
            Incoming::Joined(id, r.bool()?)
        }
        _ => return Err(invalid()),
    };
    end(&r)?;
    Ok(p)
}
pub enum Command<'a> {
    Create(&'a str, &'a str, i16),
    Join(u128, &'a str),
    Leave,
}
impl Command<'_> {
    pub fn encode(&self) -> Result<(&'static str, Vec<u8>)> {
        let mut w = Writer::new();
        let channel = match *self {
            Self::Create(name, password, kind) => {
                if !valid(name)
                    || (!password.is_empty() && !valid(password))
                    || !(0..=2).contains(&kind)
                {
                    return Err(invalid());
                }
                w.string(name);
                w.bool(!password.is_empty());
                if !password.is_empty() {
                    w.string(password);
                }
                w.i16(kind);
                "voicechat:create_group"
            }
            Self::Join(id, password) => {
                if !password.is_empty() && !valid(password) {
                    return Err(invalid());
                }
                w.uuid(id);
                w.bool(!password.is_empty());
                if !password.is_empty() {
                    w.string(password);
                }
                "voicechat:set_group"
            }
            Self::Leave => "voicechat:leave_group",
        };
        Ok((channel, w.data))
    }
}
impl State {
    pub fn receive(&mut self, channel: &str, data: &[u8]) -> Result<()> {
        if channel.ends_with("_group") {
            match decode(channel, data)? {
                Incoming::Add(g) => {
                    self.groups.insert(g.id, g);
                }
                Incoming::Remove(id) => {
                    self.groups.remove(&id);
                }
                Incoming::Joined(id, wrong) => {
                    self.current = id;
                    self.wrong_password = wrong;
                }
            }
        } else {
            let mut r = Reader::new(data);
            if channel == "voicechat:remove_state" {
                let id = r.uuid()?;
                end(&r)?;
                self.players.remove(&id);
            } else {
                let n = if channel == "voicechat:states" {
                    r.i32()?
                } else {
                    1
                };
                if !(0..=4096).contains(&n) {
                    return Err(invalid());
                }
                let mut players = Vec::new();
                for _ in 0..n {
                    players.push(Player {
                        disabled: r.bool()?,
                        disconnected: r.bool()?,
                        id: r.uuid()?,
                        name: r.string(32767)?,
                        group: if r.bool()? { Some(r.uuid()?) } else { None },
                    });
                }
                end(&r)?;
                for p in players {
                    self.players.insert(p.id, p);
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_payload_vectors() {
        let mut add = vec![0; 16];
        add.extend([1, b'A', 1, 0, 1, 0, 2]);
        assert_eq!(
            decode("voicechat:add_group", &add).unwrap(),
            Incoming::Add(Group {
                id: 0,
                name: "A".into(),
                password: true,
                persistent: false,
                hidden: true,
                kind: 2
            })
        );
        assert_eq!(
            decode("voicechat:remove_group", &[0; 16]).unwrap(),
            Incoming::Remove(0)
        );
        let mut joined = vec![1];
        joined.extend([0; 16]);
        joined.push(0);
        assert_eq!(
            decode("voicechat:joined_group", &joined).unwrap(),
            Incoming::Joined(Some(0), false)
        );
        assert_eq!(
            decode("voicechat:joined_group", &[0, 1]).unwrap(),
            Incoming::Joined(None, true)
        );
        assert_eq!(
            Command::Create("A", "B", 1).encode().unwrap().1,
            [1, b'A', 1, 1, b'B', 0, 1]
        );
        assert_eq!(
            Command::Create("A", "", 0).encode().unwrap().1,
            [1, b'A', 0, 0, 0]
        );
        let mut join = vec![0; 16];
        join.extend([1, 1, b'B']);
        assert_eq!(Command::Join(0, "B").encode().unwrap().1, join);
        assert_eq!(
            Command::Leave.encode().unwrap(),
            ("voicechat:leave_group", Vec::<u8>::new())
        );
        let mut join_empty = vec![0; 16];
        join_empty.push(0);
        assert_eq!(
            Command::Join(0, "").encode().unwrap(),
            ("voicechat:set_group", join_empty)
        );
        assert_eq!(
            Command::Create("A", "", 2).encode().unwrap(),
            ("voicechat:create_group", vec![1, b'A', 0, 0, 2])
        );
        assert_eq!(
            decode("voicechat:joined_group", &[0, 0]).unwrap(),
            Incoming::Joined(None, false)
        );
        let mut unknown = vec![0; 16];
        unknown.extend([1, b'A', 0, 1, 0, 0x7f, 0xff]);
        assert!(matches!(
            decode("voicechat:add_group", &unknown).unwrap(),
            Incoming::Add(Group { kind: 0, .. })
        ));
        for channel in [
            "voicechat:add_group",
            "voicechat:remove_group",
            "voicechat:joined_group",
        ] {
            assert!(decode(channel, &[]).is_err());
        }
        add.push(0);
        assert!(decode("voicechat:add_group", &add).is_err());
    }
    #[test]
    fn player_membership_payload_vectors() {
        let mut state = State::default();
        let mut bytes = vec![0, 0, 0, 1, 0, 1];
        bytes.extend([0; 16]);
        bytes.extend([1, b'P', 1]);
        bytes.extend([7; 16]);
        state.receive("voicechat:states", &bytes).unwrap();
        let player = state.players.get(&0).unwrap();
        assert!(player.disconnected);
        assert!(!player.disabled);
        assert_eq!(player.name, "P");
        assert_eq!(player.group, Some(u128::from_be_bytes([7; 16])));
        state.receive("voicechat:remove_state", &[0; 16]).unwrap();
        assert!(state.players.is_empty());
    }
    #[test]
    fn group_field_validation() {
        for s in ["", " leading", "\tbad", "a\n", "a\u{200b}", "a\u{ad}"] {
            assert!(!valid(s), "{s:?}");
        }
        assert!(valid(&"я".repeat(24)));
        assert!(!valid(&"я".repeat(25)));
        assert!(valid("Two words"));
    }
}
