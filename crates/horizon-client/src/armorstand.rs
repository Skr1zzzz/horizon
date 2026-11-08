//! Armor Stands 2.2.4 interoperability; codec corrections adapted from armorstands (MIT).
//! Copyright (c) 2021 Evan Steinkerchner (Roundaround). See THIRD_PARTY_NOTICES.md.
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};

pub const RECEIVE: &str = "armorstands:open_screen_s2c\0armorstands:client_update_s2c\0armorstands:message_s2c\0armorstands:pong_s2c";
pub fn diagnostic(message: impl std::fmt::Display) {
    if std::env::var_os("HORIZON_ARMORSTANDS_LOG").is_some_and(|v| v == "1") {
        log::info!("armorstands: {message}");
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Open {
        sync: i32,
        entity: i32,
        page: i32,
    },
    Update {
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        invulnerable: bool,
        slots: i32,
    },
    Message {
        translated: bool,
        message: String,
        styled: bool,
        color: i32,
    },
    Pong(u128),
}
pub fn decode(channel: &str, data: &[u8]) -> Result<Incoming> {
    let mut r = Reader::new(data);
    let p = match channel.strip_prefix("armorstands:") {
        Some("open_screen_s2c") => {
            // PacketCodecs.INTEGER is fixed-width; only the indexed enum is VarInt.
            let (sync, entity, page) = (r.i32()?, r.i32()?, r.varint()?);
            if !(1..=255).contains(&sync) || !(0..=5).contains(&page) || entity < 0 {
                return Err(Error::new("invalid armorstand session"));
            }
            Incoming::Open { sync, entity, page }
        }
        Some("client_update_s2c") => Incoming::Update {
            position: [r.f64()?, r.f64()?, r.f64()?],
            yaw: r.f32()?,
            pitch: r.f32()?,
            invulnerable: r.bool()?,
            slots: r.i32()?,
        },
        Some("message_s2c") => Incoming::Message {
            translated: r.bool()?,
            message: r.string(32767)?,
            styled: r.bool()?,
            color: r.i32()?,
        },
        Some("pong_s2c") => Incoming::Pong(r.uuid()?),
        _ => return Err(Error::new("unknown armorstand payload")),
    };
    if r.remaining() != 0 {
        return Err(Error::new("trailing armorstand bytes"));
    }
    diagnostic(format_args!("receive {channel} {p:?}"));
    Ok(p)
}
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Ping(u128),
    Request(i32, i32),
    Pose(i32, i32, f32),
    FullPose([[f32; 3]; 6]),
    Scale(f32),
    Position(i32, i32, i32, i32),
    Yaw(i32),
    Flag(i32, bool),
    Preset(&'static str),
    AbsoluteYaw(f32),
    Undo(bool),
    Utility(&'static str),
}
impl Command {
    pub fn encode(&self) -> (String, Vec<u8>) {
        let mut w = Writer::new();
        let suffix = match *self {
            Self::Ping(id) => {
                w.uuid(id);
                "ping_c2s"
            }
            Self::Request(e, p) => {
                w.i32(e);
                w.varint(p);
                "request_screen_c2s"
            }
            Self::Pose(p, a, v) => {
                w.varint(p);
                w.varint(a);
                w.f32(v);
                "adjust_pose_c2s"
            }
            Self::FullPose(pose) => {
                for part in pose {
                    for value in part {
                        w.f32(value);
                    }
                }
                "set_pose_c2s"
            }
            Self::Scale(value) => {
                w.f32(value);
                "set_scale_c2s"
            }
            Self::Position(d, a, m, u) => {
                w.varint(d);
                w.i32(a);
                w.varint(m);
                w.varint(u);
                "adjust_pos_c2s"
            }
            Self::Yaw(v) => {
                w.i32(v);
                "adjust_yaw_c2s"
            }
            Self::Flag(f, v) => {
                w.varint(f);
                w.bool(v);
                "set_flag_c2s"
            }
            Self::Preset(s) => {
                w.string(s);
                "set_pose_preset_c2s"
            }
            Self::AbsoluteYaw(v) => {
                w.f32(v);
                "set_yaw_c2s"
            }
            Self::Undo(v) => {
                w.bool(v);
                "undo_c2s"
            }
            Self::Utility(s) => {
                w.string(s);
                "utility_action_c2s"
            }
        };
        let channel = format!("armorstands:{suffix}");
        (channel, w.data)
    }
}
/// A single in-flight edit keeps its target stable even if the selection changes.
#[derive(Debug)]
pub struct Preview {
    pub part: usize,
    pub axis: usize,
    pub value: f32,
    pub dragging: bool,
    pub wheel: Option<std::time::Instant>,
    pub waiting: bool,
}
impl Preview {
    pub fn reflected(&self, server: f32) -> f32 {
        if self.dragging || self.wheel.is_some() || self.waiting {
            self.value
        } else {
            server
        }
    }
}
pub fn inventory_slots() -> Vec<(i32, i32, u8)> {
    (0..42)
        .map(|i| match i {
            0..=8 => (8 + i * 18, 142, 1),
            9..=35 => (8 + (i - 9) % 9 * 18, 84 + (i - 9) / 9 * 18, 1),
            36..=37 => (116, 44 + (i - 36) * 18, 0),
            _ => (44, 8 + (i - 38) * 18, 0),
        })
        .collect()
}
pub const PART_INDEX: [usize; 6] = [0, 1, 3, 2, 5, 4];
pub fn pose_value(position: f32, range: usize) -> f32 {
    (position.clamp(0., 1.) * 2. - 1.) * [180., 90., 35.][range]
}
pub fn pose_position(value: f32, range: usize) -> f32 {
    (value / [180., 90., 35.][range] * 0.5 + 0.5).clamp(0., 1.)
}
pub fn scale_value(position: f32) -> f32 {
    let p = position.clamp(0., 1.);
    if p < 0.5 {
        0.01 + p * 3.98
    } else {
        2. + (p - 0.5) * 16.
    }
}
pub fn scale_position(value: f32) -> f32 {
    if value < 2. {
        (value - 0.01) / 3.98
    } else {
        0.5 + (value - 2.) / 16.
    }
    .clamp(0., 1.)
}
pub fn mirror(mut pose: [[f32; 3]; 6]) -> [[f32; 3]; 6] {
    pose.swap(2, 3);
    pose.swap(4, 5);
    for p in &mut pose {
        p[1] = -p[1];
        p[2] = -p[2];
    }
    pose
}
#[derive(Default, Debug)]
pub struct Session {
    pub active: Option<(i32, i32, i32)>,
    pub invulnerable: bool,
    pub mode: i32,
    pub units: i32,
    pub preset_page: usize,
    pub part: usize,
    pub range: usize,
    pub preview: Option<Preview>,
    pub scale: f32,
    pub message: Option<(String, u32, std::time::Instant)>,
}
impl Session {
    pub fn owns(&self, sync: i32) -> bool {
        self.active.is_some_and(|s| s.0 == sync && s.2 != 5)
    }
    pub fn open(&mut self, sync: i32, entity: i32, page: i32) {
        self.active = Some((sync, entity, page));
        self.preview = None;
        self.scale = 1.;
        self.message = None;
    }
    pub fn close(&mut self) -> Option<i32> {
        self.active.take().map(|s| s.0)
    }
}
pub const PRESETS: &[&str] = &[
    "default",
    "attention",
    "confident",
    "walking",
    "running",
    "lunging",
    "pointing",
    "facepalm",
    "blocking",
    "confused",
    "winning",
    "formal",
    "salute",
    "sad",
    "arabesque",
    "cancan",
    "joyous",
    "cupid",
    "stargazing",
    "guard",
    "brandish",
    "speaking",
    "inspiring",
    "thanking",
    "dabbing",
    "pledging",
    "contemplating",
    "zombie",
    "sitting",
    "lazing",
    "jumpAttack",
    "steward",
    "scarecrow",
    "pretty",
    "drumMajor",
    "fishing",
];
#[cfg(test)]
mod armorstand_tests {
    use super::*;
    #[test]
    fn stale_server_values_do_not_replace_pending_preview() {
        let mut p = Preview {
            part: 0,
            axis: 0,
            value: 42.,
            dragging: true,
            wheel: None,
            waiting: false,
        };
        assert_eq!(p.reflected(-10.), 42.);
        p.dragging = false;
        p.waiting = true;
        assert_eq!(p.reflected(-10.), 42.);
        p.waiting = false;
        assert_eq!(p.reflected(-10.), -10.);
    }
    #[test]
    fn inventory_wire_indices() {
        let menu = crate::net::pod::Menu {
            sync_id: 1,
            menu: "armorstands:inventory".into(),
            title: String::new(),
            slots: inventory_slots(),
            data_slots: 0,
        };
        for i in 0..42 {
            let internal = if i < 36 { i } else { 10000 + i };
            assert_eq!(menu.internal(i), Some(internal));
            assert_eq!(menu.index(internal), Some(i));
        }
        assert_eq!(menu.slots[0], (8, 142, 1));
        assert_eq!(menu.slots[9], (8, 84, 1));
        assert_eq!(menu.slots[41], (44, 62, 0));
        // The handler adds hotbar first, unlike ordinary vanilla containers.
        for (wire, internal, position) in [
            (8, 8, (152, 142, 1)),
            (9, 9, (8, 84, 1)),
            (35, 35, (152, 120, 1)),
            (36, 10036, (116, 44, 0)),
            (37, 10037, (116, 62, 0)),
            (38, 10038, (44, 8, 0)),
        ] {
            assert_eq!(menu.internal(wire), Some(internal));
            assert_eq!(menu.index(internal), Some(wire));
            assert_eq!(menu.slots[wire], position);
        }
        let layout = menu.layout();
        assert_eq!(
            (layout.width, layout.height, layout.inventory_y),
            (176., 166., 84.)
        );
        assert_eq!(menu.internal(42), None);
    }
    #[test]
    fn slider_mapping_and_payloads() {
        for (range, max) in [180., 90., 35.].into_iter().enumerate() {
            assert_eq!(pose_value(0., range), -max);
            assert_eq!(pose_value(0.5, range), 0.);
            assert_eq!(pose_value(1., range), max);
            assert_eq!(pose_position(max, range), 1.);
        }
        for (p, value) in [(0., 0.01), (0.5, 2.), (1., 10.)] {
            assert!((scale_value(p) - value).abs() < 0.00001);
            assert!((scale_position(value) - p).abs() < 0.00001);
        }
        assert_eq!(
            Command::Scale(2.).encode(),
            ("armorstands:set_scale_c2s".into(), vec![0x40, 0, 0, 0])
        );
        let mut pose = [[0.; 3]; 6];
        pose[5][2] = 1.;
        let mut expected = vec![0; 68];
        expected.extend([0x3f, 0x80, 0, 0]);
        assert_eq!(
            Command::FullPose(pose).encode(),
            ("armorstands:set_pose_c2s".into(), expected)
        );
    }
    #[test]
    fn command_vectors() {
        for (c, b) in [
            (Command::Ping(0), vec![0; 16]),
            (Command::Request(300, 4), vec![0, 0, 1, 44, 4]),
            (Command::Pose(2, 1, 90.0), vec![2, 1, 0x42, 0xb4, 0, 0]),
            (Command::Position(5, 3, 2, 1), vec![5, 0, 0, 0, 3, 2, 1]),
            (Command::Yaw(-1), vec![255, 255, 255, 255]),
            (Command::Yaw(45), vec![0, 0, 0, 45]),
            (Command::Flag(8, true), vec![8, 1]),
            (Command::Preset("default"), b"\x07default".to_vec()),
            (Command::AbsoluteYaw(-90.0), vec![0xc2, 0xb4, 0, 0]),
            (Command::Undo(false), vec![0]),
            (Command::Utility("copy"), b"\x04copy".to_vec()),
        ] {
            assert_eq!(c.encode().1, b);
        }
    }
    #[test]
    fn incoming_vectors_and_truncation() {
        let mut update = vec![0; 32];
        update.extend([1, 0, 0x3f, 0x3f, 0x3f]);
        let cases = [
            (
                "open_screen_s2c",
                vec![0, 0, 0, 1, 0, 0, 1, 44, 4],
                Incoming::Open {
                    sync: 1,
                    entity: 300,
                    page: 4,
                },
            ),
            (
                "pong_s2c",
                vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
                Incoming::Pong(0x000102030405060708090a0b0c0d0e0f),
            ),
            (
                "message_s2c",
                vec![0, 1, b'y', 1, 0, 0x12, 0x34, 0x56],
                Incoming::Message {
                    translated: false,
                    message: "y".into(),
                    styled: true,
                    color: 0x123456,
                },
            ),
            (
                "message_s2c",
                vec![1, 1, b'x', 0, 255, 255, 255, 255],
                Incoming::Message {
                    translated: true,
                    message: "x".into(),
                    styled: false,
                    color: -1,
                },
            ),
            (
                "client_update_s2c",
                update,
                Incoming::Update {
                    position: [0.; 3],
                    yaw: 0.,
                    pitch: 0.,
                    invulnerable: true,
                    slots: 4144959,
                },
            ),
        ];
        for (c, b, p) in cases {
            let c = format!("armorstands:{c}");
            assert_eq!(decode(&c, &b).unwrap(), p);
            for n in 0..b.len() {
                assert!(decode(&c, &b[..n]).is_err());
            }
            let mut extra = b;
            extra.push(0);
            assert!(decode(&c, &extra).is_err());
        }
    }
    #[test]
    fn armorstand_container_ownership_tracks_authorized_session() {
        let mut s = Session::default();
        // A vanilla open/layout arriving first must not grant edit capability.
        assert!(!s.owns(1));
        s.open(1, 212, 0);
        assert!(s.owns(1));
        assert!(!s.owns(2));
        // A later open/layout of the same ID belongs to the combined panel.
        assert!(s.owns(1));
        s.open(2, 212, 3);
        assert!(!s.owns(1));
        assert!(s.owns(2));
        s.close();
        assert!(!s.owns(2));
        s.open(3, 212, 5);
        assert!(!s.owns(3));
    }
    #[test]
    fn session_replacement_and_close() {
        let mut s = Session::default();
        assert_eq!(s.close(), None);
        s.open(1, 7, 0);
        s.open(2, 7, 3);
        assert_eq!(s.active, Some((2, 7, 3)));
        assert_eq!(s.close(), Some(2));
        assert_eq!(s.close(), None);
        s.open(3, 9, 1);
        assert_eq!(s.active, Some((3, 9, 1)));
    }
}
