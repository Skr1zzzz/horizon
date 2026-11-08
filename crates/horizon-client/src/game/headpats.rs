//! Adapted from headpats (MIT, enjarai); see THIRD_PARTY_NOTICES.md.
use super::{Game, Screen};
use horizon_client::headpat::{self, Component};
use std::collections::HashMap;
#[derive(Default)]
pub(super) struct State {
    pub channels: std::collections::HashSet<String>,
    pub players: HashMap<i32, Component>,
    pub registered: bool,
}
impl State {
    pub fn available(&self) -> bool {
        headpat::available(self.channels.iter().map(String::as_str))
    }
}
impl Game<'_> {
    fn headpat_send(&self, id: i32) {
        headpat::diagnostic(format_args!("send headpats:petting entity={id}"));
        self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
            horizon_protocol::packets::configuration::CustomPayload {
                channel: headpat::PETTING.into(),
                data: headpat::encode(id),
            },
        ));
    }
    pub(super) fn headpat_target(&self, i: usize, location: [f64; 3]) -> bool {
        let m = &self.mobs[i];
        self.held().is_none()
            && self.player.as_ref().is_some_and(|p| {
                let dx = p.position.x - m.position.x;
                let dy = p.position.y - m.position.y;
                let dz = p.position.z - m.position.z;
                dx * dx + dy * dy + dz * dz < 2.25
            })
            && f64::from(location[1])
                > m.kind.height * f64::from(m.extra.model_scale)
                    - 0.5 * f64::from(m.extra.model_scale)
            && self.remote.as_ref().is_some_and(|r| {
                r.headpats.available() && r.level_players.contains_key(&(m.id as i32))
            })
    }
    pub(super) fn start_headpat(&mut self, i: usize, location: [f64; 3]) -> bool {
        let m = &self.mobs[i];
        let r = self.remote.as_ref();
        let distance = self.player.as_ref().map_or(f64::INFINITY, |p| {
            (p.position.x - m.position.x).powi(2)
                + (p.position.y - m.position.y).powi(2)
                + (p.position.z - m.position.z).powi(2)
        });
        let reason = if r.is_none_or(|r| !r.headpats.available()) {
            "not available"
        } else if !r.unwrap().level_players.contains_key(&(m.id as i32)) {
            "not a player"
        } else if self.held().is_some() {
            "held item"
        } else if distance >= 2.25 {
            "distance"
        } else if !self.headpat_target(i, location) {
            "hit height"
        } else if r
            .unwrap()
            .headpats
            .players
            .get(&r.unwrap().entity_id)
            .is_some_and(|c| c.target.is_some())
        {
            "already patting"
        } else {
            "accepted"
        };
        headpat::diagnostic(format_args!(
            "start entity={} reason={reason} distance_squared={distance} hit_y={} height={} scale={}",
            m.id, location[1], m.kind.height, m.extra.model_scale
        ));
        if reason != "accepted" {
            return false;
        }
        let r = self.remote.as_mut().unwrap();
        let id = self.mobs[i].id as i32;
        let target = r.level_players[&id];
        let own = r.entity_id;
        let c = r.headpats.players.entry(own).or_default();
        if c.target.is_some() {
            return false;
        }
        c.target = Some(target);
        r.headpats.players.entry(id).or_default().incoming += 1;
        self.headpat_send(id);
        true
    }
    pub(super) fn tick_headpats(&mut self) {
        let Some(r) = self.remote.as_ref() else {
            return;
        };
        if !r.headpats.available() {
            return;
        }
        if !r.headpats.registered {
            self.remote.as_mut().unwrap().headpats.registered = true;
            self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                horizon_protocol::packets::configuration::CustomPayload {
                    channel: "minecraft:register".into(),
                    data: headpat::CHANNEL.as_bytes().to_vec(),
                },
            ));
        }
        let r = self.remote.as_ref().unwrap();
        let target = r.headpats.players.get(&r.entity_id).and_then(|c| c.target);
        let keep = self.screen == Screen::Playing
            && self.right
            && self.mob_hit().is_some_and(|(i, d, loc)| {
                self.before_block(d)
                    && self.headpat_target(i, loc)
                    && self
                        .remote
                        .as_ref()
                        .unwrap()
                        .level_players
                        .get(&(self.mobs[i].id as i32))
                        .copied()
                        == target
            });
        if target.is_some() && !keep {
            let r = self.remote.as_mut().unwrap();
            r.headpats.players.entry(r.entity_id).or_default().target = None;
            if let Some((&id, _)) = r.level_players.iter().find(|(_, u)| Some(**u) == target) {
                r.headpats.players.entry(id).or_default().incoming -= 1;
            }
            self.headpat_send(-1);
        }
        let r = self.remote.as_mut().unwrap();
        for c in r.headpats.players.values_mut() {
            c.tick();
        }
        for m in self.mobs.iter_mut() {
            m.headpat = r
                .headpats
                .players
                .get(&(m.id as i32))
                .cloned()
                .unwrap_or_default();
            if m.headpat.target.is_some() {
                m.body_yaw = m.yaw;
            }
        }
        if let Some(m) = self.player_model.as_mut() {
            m.headpat = r
                .headpats
                .players
                .get(&r.entity_id)
                .cloned()
                .unwrap_or_default();
        }
    }
}
