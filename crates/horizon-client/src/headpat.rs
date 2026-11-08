//! Adapted from headpats 1.0.6 (MIT, enjarai), PettingComponent / PetRendering.
//! CCA envelope adapted from Cardinal Components API 7.3.0 (MIT, Ladysnake).
//! Copyright (c) 2019 Ladysnake. See THIRD_PARTY_NOTICES.md for the MIT notice.
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};
pub const CHANNEL: &str = "cardinal-components:entity_sync";
pub const PETTING: &str = "headpats:petting";

pub fn diagnostic(message: impl std::fmt::Display) {
    if std::env::var_os("HORIZON_HEADPATS_LOG").is_some_and(|v| v == "1") {
        log::info!("headpats: {message}");
    }
}

/// Server declarations list C2S receivers, not its S2C senders.
pub fn available<'a>(channels: impl IntoIterator<Item = &'a str>) -> bool {
    channels.into_iter().any(|c| c == PETTING)
}

#[derive(Clone, Debug, Default)]
pub struct Component {
    pub target: Option<u128>,
    pub incoming: i32,
    pub petting: Clock,
    pub petted: Clock,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    pub ticks: i32,
    pub previous_ticks: i32,
    pub multiplier: f32,
    pub previous_multiplier: f32,
}
impl Clock {
    fn tick(&mut self, active: bool) {
        self.previous_ticks = self.ticks;
        self.previous_multiplier = self.multiplier;
        if active {
            self.ticks = self.ticks.wrapping_add(1);
            self.multiplier += (1.0 - self.multiplier) * 0.3;
        } else {
            self.multiplier -= self.multiplier * 0.3;
            if self.multiplier < 0.01 {
                self.multiplier = 0.0;
                self.ticks = 0;
            }
        }
    }
    pub fn at(self, partial: f32) -> (f32, f32) {
        (
            self.previous_ticks as f32 + (self.ticks as f32 - self.previous_ticks as f32) * partial,
            self.previous_multiplier + (self.multiplier - self.previous_multiplier) * partial,
        )
    }
}
impl Component {
    pub fn tick(&mut self) {
        self.petting.tick(self.target.is_some());
        self.petted.tick(self.incoming > 0);
    }
}

/// CCA length-delimits component data, so unrelated components are safely ignored.
pub fn decode(bytes: &[u8]) -> Result<Option<(i32, Option<u128>, i32)>> {
    let mut r = Reader::new(bytes);
    let entity = r.varint()?;
    let _required = r.bool()?;
    let key = r.string(256)?;
    let len = r.count(1 << 20)?;
    let data = r.bytes(len)?;
    if r.remaining() != 0 {
        return Err(Error::new("Trailing CCA bytes"));
    }
    if key != PETTING {
        return Ok(None);
    }
    let mut r = Reader::new(data);
    let target = if r.bool()? { Some(r.uuid()?) } else { None };
    let incoming = r.i32()?;
    if r.remaining() != 0 {
        return Err(Error::new("Trailing headpats bytes"));
    }
    Ok(Some((entity, target, incoming)))
}
// PettingC2SPacket uses PacketCodecs.INTEGER, unlike CCA's VarInt entity ID.
pub fn encode(entity: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(entity);
    w.data
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headpat_registration_is_directional() {
        use horizon_protocol::{packets::configuration as cfg, registry::fabric::Handshake};
        assert!(!available([CHANNEL]));
        assert!(available([PETTING]));
        let mut h = Handshake::new(true);
        let reply = h
            .receive(&cfg::Clientbound::CustomPayload(cfg::CustomPayload {
                channel: "minecraft:register".into(),
                data: PETTING.as_bytes().to_vec(),
            }))
            .unwrap();
        let Some(cfg::Serverbound::CustomPayload(reply)) = reply else {
            panic!("registration reply");
        };
        assert!(
            reply
                .data
                .split(|b| *b == 0)
                .any(|c| c == CHANNEL.as_bytes())
        );
        assert!(available(h.headpat_channels().iter().map(String::as_str)));
        let mut h = Handshake::new(true);
        let reply = h
            .receive(&cfg::Clientbound::CustomPayload(cfg::CustomPayload {
                channel: "c:register".into(),
                data: b"\x01\x04play\x01\x10headpats:petting".to_vec(),
            }))
            .unwrap();
        let Some(cfg::Serverbound::CustomPayload(reply)) = reply else {
            panic!("common registration reply");
        };
        assert_eq!(reply.channel, "c:register");
        assert_eq!(
            reply.data,
            b"\x01\x04play\x01\x1fcardinal-components:entity_sync"
        );
        assert!(available(h.headpat_channels().iter().map(String::as_str)));
        assert!(!available(
            Handshake::new(false)
                .headpat_channels()
                .iter()
                .map(String::as_str)
        ));
    }
    #[test]
    fn headpat_java_wire_vectors() {
        // target VarInt, required Boolean, Identifier, VarInt buffer size, Boolean + Int.
        let stopped = b"\xac\x02\x00\x10headpats:petting\x05\x00\x00\x00\x00\x02";
        assert_eq!(decode(stopped).unwrap(), Some((300, None, 2)));
        let active = b"\x01\x00\x10headpats:petting\x15\x01\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\xff\xff\xff\xff";
        assert_eq!(
            decode(active).unwrap(),
            Some((1, Some(0x000102030405060708090a0b0c0d0e0f), -1))
        );
        for end in 0..active.len() {
            assert!(decode(&active[..end]).is_err());
        }
        assert_eq!(encode(198), [0x00, 0x00, 0x00, 0xc6]);
        assert_eq!(encode(300), [0x00, 0x00, 0x01, 0x2c]);
        assert_eq!(encode(-1), [0xff, 0xff, 0xff, 0xff]);
    }
    #[test]
    fn headpat_clocks_ease_and_reset_like_java() {
        let mut c = Component {
            target: Some(1),
            incoming: 2,
            ..Default::default()
        };
        c.tick();
        assert_eq!(c.petting.at(0.5), (0.5, 0.15));
        c.tick();
        assert!((c.petting.multiplier - 0.51).abs() < 1e-6);
        assert_eq!(c.petted.ticks, 2);
        c.target = None;
        c.incoming = 0;
        c.tick();
        assert_eq!(c.petting.ticks, 2);
        assert!((c.petting.multiplier - 0.357).abs() < 1e-6);
        for _ in 0..10 {
            c.tick();
        }
        assert!(c.petting.multiplier > 0.0);
        c.tick();
        assert_eq!((c.petting.ticks, c.petting.multiplier), (0, 0.0));
    }
}
