//! Adapted from laser-eyes (MIT); state belongs to the remote session, never a world save.
use super::{Game, Screen};
use glam::{DVec3, Mat4, Quat, Vec3};
use horizon_client::{
    laser,
    mesher::Vertex,
    sound_engine::{SoundEntity, SoundInstance, Sounds},
};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct State {
    pub available: bool,
    pub config: laser::Config,
    pub firing: HashMap<u128, u32>,
    held: bool,
    registered: bool,
    loops: HashMap<u128, SoundEntity>,
}
impl Drop for State {
    fn drop(&mut self) {
        for entity in self.loops.values() {
            let mut s = entity.state();
            s.removed = true;
            entity.update(s);
        }
    }
}
impl Game<'_> {
    pub(super) fn laser_payload(&mut self, channel: &str, bytes: &[u8]) {
        let Some(r) = self.remote.as_mut().filter(|r| r.laser.available) else {
            return;
        };
        let result = match channel {
            "laser-eyes:sync" => laser::sync(bytes).map(|(uuid, active)| {
                if active {
                    r.laser.firing.entry(uuid).or_insert(0);
                } else {
                    r.laser.firing.remove(&uuid);
                }
            }),
            "laser-eyes:config_sync" => laser::Config::decode(bytes).map(|c| r.laser.config = c),
            _ => return,
        };
        if let Err(e) = result {
            log::warn!("Ignoring malformed {channel}: {e}");
        }
    }
    // Keep the same pose inputs for the bound sound and the rendered beam.
    fn laser_pose(&self, uuid: u128, alpha: f64) -> Option<(DVec3, f64, f64, f64, f32, f32, u32)> {
        let r = self.remote.as_ref()?;
        if uuid == r.profile() {
            let p = self.player.as_ref()?;
            if p.health.dead_or_dying() {
                return None;
            }
            let feet = DVec3::new(p.position.x, p.position.y, p.position.z);
            let eye = self.eye_position()?;
            return Some((
                feet,
                eye[1] - feet.y,
                f64::from(p.dimensions.height),
                f64::from(p.dimensions.width),
                self.yaw,
                self.pitch,
                0,
            ));
        }
        let id = r.level_players.iter().find(|(_, u)| **u == uuid)?.0;
        let m = self
            .mobs
            .iter()
            .find(|m| m.id == *id as u32 && m.health > 0.0)?;
        let previous = DVec3::new(m.previous.x, m.previous.y, m.previous.z);
        let p = previous.lerp(DVec3::new(m.position.x, m.position.y, m.position.z), alpha);
        let yaw = m.previous_head_yaw
            + ((m.head_yaw - m.previous_head_yaw + 180.0).rem_euclid(360.0) - 180.0) * alpha as f32;
        Some((
            DVec3::new(p.x, p.y, p.z),
            m.kind.eye_height,
            m.kind.height,
            m.kind.width,
            yaw,
            m.previous_head_pitch + (m.head_pitch - m.previous_head_pitch) * alpha as f32,
            m.id,
        ))
    }
    pub(super) fn release_laser(&mut self) {
        if let Some(r) = self.remote.as_mut()
            && r.laser.held
        {
            r.laser.held = false;
            self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                horizon_protocol::packets::configuration::CustomPayload {
                    channel: "laser-eyes:fire".into(),
                    data: vec![0],
                },
            ));
        }
    }
    pub(super) fn tick_laser(&mut self) {
        let held = self.screen == Screen::Playing
            && self.focused
            && self.keys.held(horizon_client::actions::Action::LaserFire);
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let register = r.laser.available && !r.laser.registered && r.logged_in;
        if register {
            r.laser.registered = true;
        }
        let held = held && r.laser.available && r.logged_in;
        let changed = held != r.laser.held;
        r.laser.held = held;
        if register {
            self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                horizon_protocol::packets::configuration::CustomPayload {
                    channel: "minecraft:register".into(),
                    data: b"laser-eyes:sync\0laser-eyes:config_sync".to_vec(),
                },
            ));
        }
        if changed {
            self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                horizon_protocol::packets::configuration::CustomPayload {
                    channel: "laser-eyes:fire".into(),
                    data: vec![u8::from(held)],
                },
            ));
        }
        let ids: Vec<_> = self
            .remote
            .as_ref()
            .unwrap()
            .laser
            .firing
            .keys()
            .copied()
            .collect();
        let poses: HashMap<_, _> = ids
            .into_iter()
            .filter_map(|u| self.laser_pose(u, 1.0).map(|p| (u, p)))
            .collect();
        let r = self.remote.as_ref().unwrap();
        let glow = |uuid: u128| {
            r.laser
                .available
                .then(|| r.laser.firing.get(&uuid))
                .flatten()
                .map(|age| {
                    let alpha = ((*age as f32 / 60.0).min(1.0) * 255.0) as u32;
                    (
                        alpha << 24 | (r.laser.config.color & 0xffffff),
                        (14i32.saturating_sub(r.laser.config.eye_offset)).clamp(10, 15),
                    )
                })
        };
        for m in self.mobs.iter_mut() {
            m.laser_glow = r.level_players.get(&(m.id as i32)).and_then(|u| glow(*u));
        }
        if let Some(m) = self.player_model.as_mut() {
            m.laser_glow = glow(r.profile());
        }
        let state = &mut self.remote.as_mut().unwrap().laser;
        state.loops.retain(|uuid, entity| {
            let keep =
                state.available && state.firing.contains_key(uuid) && poses.contains_key(uuid);
            if !keep {
                let mut s = entity.state();
                s.removed = true;
                entity.update(s);
            }
            keep
        });
        if !state.available {
            state.firing.clear();
            return;
        }
        for (uuid, (feet, eye, _, _, _, _, id)) in poses {
            let age = state.firing.get_mut(&uuid).unwrap();
            *age = age.saturating_add(1);
            if *age < 60 {
                continue;
            }
            let at = (feet + DVec3::Y * eye).to_array();
            let entity = state.loops.entry(uuid).or_insert_with(|| {
                let e = SoundEntity::new(id, at, false);
                let mut sound = SoundInstance::entity_bound(
                    "laser-eyes:laser_eye",
                    horizon_client::sound_engine::PLAYERS,
                    1.0,
                    1.0,
                    horizon_core::random::LegacyRandom::new(0),
                    e.clone(),
                );
                sound.looping = true;
                self.audio.play_instance(sound, false);
                e
            });
            let mut s = entity.state();
            s.position = at;
            entity.update(s);
        }
    }
    fn laser_hit(&self, origin: DVec3, dir: DVec3) -> Option<DVec3> {
        let mut scene = horizon_client::collisions::SceneCollisions {
            world: &self.world,
            binding: self.collision,
            subject: None,
        };
        let mut cell = origin.floor().as_ivec3().to_array();
        let o = origin.to_array();
        let d = dir.to_array();
        let step = d.map(|v| if v > 0.0 { 1 } else { -1 });
        let delta = d.map(|v| {
            if v == 0.0 {
                f64::INFINITY
            } else {
                v.recip().abs()
            }
        });
        let mut next: [f64; 3] = std::array::from_fn(|i| {
            if d[i] == 0.0 {
                f64::INFINITY
            } else {
                (f64::from(cell[i] + i32::from(step[i] > 0)) - o[i]) / d[i]
            }
        });
        let mut t = 0.0;
        while t <= 96.0 {
            let state = self.world.block(cell[0], cell[1], cell[2]);
            if let Ok(shape) = self.collision.collision_shape(&mut scene, state, cell) {
                let mut nearest = 97.0f64;
                for b in shape.boxes() {
                    let lo = b.min_position();
                    let hi = b.max_position();
                    let min = [lo.x, lo.y, lo.z];
                    let max = [hi.x, hi.y, hi.z];
                    let (mut near, mut far) = (0.0f64, 96.0f64);
                    for i in 0..3 {
                        let low = min[i] + f64::from(cell[i]);
                        let high = max[i] + f64::from(cell[i]);
                        if d[i] == 0.0 {
                            if o[i] < low || o[i] > high {
                                far = -1.0;
                            }
                        } else {
                            let a = (low - o[i]) / d[i];
                            let b = (high - o[i]) / d[i];
                            near = near.max(a.min(b));
                            far = far.min(a.max(b));
                        }
                    }
                    if near <= far {
                        nearest = nearest.min(near);
                    }
                }
                if nearest <= 96.0 {
                    return Some(origin + dir * nearest);
                }
            }
            let axis = if next[0] < next[1] {
                if next[0] < next[2] { 0 } else { 2 }
            } else if next[1] < next[2] {
                1
            } else {
                2
            };
            t = next[axis];
            cell[axis] += step[axis];
            next[axis] += delta[axis];
        }
        None
    }
    pub(super) fn laser_geometry(&self, camera: DVec3, alpha: f64, out: &mut Vec<Vertex>) {
        let Some(r) = self.remote.as_ref().filter(|r| r.laser.available) else {
            return;
        };
        let c = &r.laser.config;
        let sprite = if c.custom {
            self.gui.atlas.sprites.get("laser-eyes:entity/laser_beam")
        } else {
            None
        }
        .or_else(|| self.gui.atlas.sprites.get("entity/beacon/beacon_beam"));
        let Some(&sprite) = sprite else {
            return;
        };
        let time = self.world.game_time.rem_euclid(1_000_000) as f32 + alpha as f32;
        for (&uuid, &ticks) in &r.laser.firing {
            if ticks < 60 {
                continue;
            }
            let Some((feet, eh, h, w, yaw, pitch, _)) = self.laser_pose(uuid, alpha) else {
                continue;
            };
            let look = DVec3::from_array(super::view_vector(yaw, pitch));
            let eye = laser::eye(feet, eh, h, w, look, c.eye_offset);
            let Some(segments) =
                laser::segments(eye, look, f64::from(yaw), w, self.laser_hit(eye, look))
            else {
                continue;
            };
            let fp = uuid == r.profile()
                && self.camera_type == super::perspective::CameraType::FirstPerson;
            let pulse = 0.85 + 0.15 * (time * 0.4).sin();
            let core = [16, 8, 0].into_iter().fold(0, |v, shift| {
                let channel = (c.color >> shift) & 255;
                v | ((channel as f32 + (255 - channel) as f32 * 0.75) as u32) << shift
            });
            let layers = if fp {
                vec![(0.0, 0.02, 0x24, c.color, 1.0)]
            } else {
                vec![
                    (0.12 * pulse, 0.30 * pulse, 0x14, c.color, 0.6),
                    (0.06 * pulse, 0.17 * pulse, 0x30, c.color, 1.0),
                    (0.03, 0.09 * pulse, 0x88, c.color, -1.3),
                    (
                        0.0,
                        0.032,
                        (240.0 * (0.9 + 0.1 * (time * 1.7).sin())) as u32,
                        core,
                        1.9,
                    ),
                ]
            };
            for (from, to) in segments {
                let dir = (to - from).as_vec3();
                let len = dir.length();
                let height = len.ceil().max(1.0);
                let base = Mat4::from_translation((from - camera).as_vec3())
                    * Mat4::from_quat(Quat::from_rotation_arc(Vec3::Y, dir / len))
                    * Mat4::from_scale(Vec3::new(1.0, len / height, 1.0))
                    * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5));
                for &(inner, outer, a, color, speed) in &layers {
                    let start = out.len();
                    horizon_client::block_entity_render::beam(
                        base,
                        horizon_client::block_entity_render::BeamShape {
                            scale: 1.0,
                            time: ((time * 6.0) % 360.0) * speed / 2.25,
                            start: 0,
                            height: height as i32,
                            color,
                            solid_radius: inner.max(0.00001),
                            glow_radius: outer,
                        },
                        sprite,
                        out,
                    );
                    for (i, v) in out[start..].iter_mut().enumerate() {
                        // A zero-radius core has no solid surface, and Java scales glow alpha by 1/8.
                        let opacity = if i < 16 {
                            if inner == 0.0 { 0 } else { a }
                        } else {
                            a / 8
                        };
                        v.color = (v.color & 0xffffff) | opacity << 24;
                        v.layer |= u32::from(horizon_client::mesher::BRIGHT) << 16;
                    }
                }
            }
        }
    }
}
