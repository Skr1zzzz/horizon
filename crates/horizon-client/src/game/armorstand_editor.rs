//! Combined server-authoritative Armor Stands panel; no local edit history or prediction.
use super::{Game, Screen};
use horizon_client::{
    armorstand::{self, Command, Incoming},
    hud::HudBatch,
    text::Text,
    ui,
};
use horizon_protocol::packets::{
    configuration::CustomPayload,
    play::{self, Serverbound},
};

impl Game<'_> {
    pub(super) fn armorstand_send(&self, command: Command) {
        let Some(r) = self.remote.as_ref() else {
            return;
        };
        let (channel, data) = command.encode();
        if r.armorstand.active.is_some() && r.headpats.channels.contains(&channel) {
            armorstand::diagnostic(format_args!("send {channel} {command:?}"));
            self.send(Serverbound::CustomPayload(CustomPayload { channel, data }));
        }
    }
    pub(super) fn armorstand_close(&mut self, notify: bool) {
        if let Some(id) = self.remote.as_mut().and_then(|r| r.armorstand.close()) {
            if notify {
                self.send(Serverbound::ContainerClose(play::ContainerClose {
                    container: id,
                }));
            }
            if self.screen == Screen::Mod {
                self.screen = Screen::Playing;
                self.capture(true);
            }
        }
    }
    pub(super) fn tick_armorstand(&mut self) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let available = r.headpats.channels.contains("armorstands:ping_c2s");
        if available && !r.armorstand_registered {
            r.armorstand_registered = true;
            armorstand::diagnostic(format_args!(
                "send minecraft:register channels={:?}",
                armorstand::RECEIVE
            ));
            self.send(Serverbound::CustomPayload(CustomPayload {
                channel: "minecraft:register".into(),
                data: armorstand::RECEIVE.as_bytes().to_vec(),
            }));
        } else if !available {
            r.armorstand_registered = false;
            self.armorstand_close(false);
        }
        if let Some((_, id, _)) = self.remote.as_ref().and_then(|r| r.armorstand.active) {
            if self.screen != Screen::Mod || !self.armor_stands.iter().any(|s| s.id as i32 == id) {
                self.armorstand_close(true);
            }
        }
    }
    pub(super) fn armorstand_receive(&mut self, channel: &str, data: &[u8]) {
        let Some(r) = self.remote.as_ref().filter(|r| r.armorstand_registered) else {
            return;
        };
        let active = r.armorstand.active;
        let packet = match armorstand::decode(channel, data) {
            Ok(p) => p,
            Err(e) => {
                log::warn!(
                    "Ignoring malformed {channel}: {e}; len={} first64={:02x?}",
                    data.len(),
                    &data[..data.len().min(64)]
                );
                return;
            }
        };
        match packet {
            Incoming::Open { sync, entity, page } => {
                // Inventory needs its own 42-slot layout; never consume it as a slotless panel.
                if !self.armor_stands.iter().any(|s| s.id as i32 == entity) {
                    self.armorstand_close(false);
                    self.send(Serverbound::ContainerClose(play::ContainerClose {
                        container: sync,
                    }));
                    return;
                }
                if page == 5 {
                    self.armorstand_close(false);
                    self.send(Serverbound::ContainerClose(play::ContainerClose {
                        container: sync,
                    }));
                    return;
                }
                self.remote
                    .as_mut()
                    .unwrap()
                    .armorstand
                    .open(sync, entity, page);
                self.armorstand_takeover(sync, "armorstands open payload");
                // The server authenticates the ping identity; no local prediction waits on this pong.
                let uuid = self
                    .remote_player_uuid(self.remote.as_ref().unwrap().entity_id as u32)
                    .unwrap_or(0);
                self.armorstand_send(Command::Ping(uuid));
            }
            Incoming::Update {
                position,
                yaw,
                pitch,
                invulnerable,
                slots,
            } => {
                if let Some((_, id, _)) = active {
                    if position.iter().all(|v| v.is_finite())
                        && yaw.is_finite()
                        && pitch.is_finite()
                    {
                        if let Some(s) = self.armor_stands.iter_mut().find(|s| s.id as i32 == id) {
                            s.position = horizon_core::geometry::Vec3::new(
                                position[0],
                                position[1],
                                position[2],
                            );
                            s.yaw = (yaw + 180.).rem_euclid(360.) - 180.;
                            s.pitch = (pitch + 180.).rem_euclid(360.) - 180.;
                            s.disabled_slots = slots;
                        }
                        self.remote.as_mut().unwrap().armorstand.invulnerable = invulnerable;
                    }
                }
            }
            Incoming::Message {
                translated,
                message,
                styled,
                color,
            } => {
                if active.is_some() {
                    let message = if translated {
                        horizon_client::lang::get(&message)
                    } else {
                        message
                    };
                    self.remote.as_mut().unwrap().armorstand.message = Some((
                        message,
                        if styled {
                            0xff000000 | color as u32 & 0xffffff
                        } else {
                            u32::MAX
                        },
                        std::time::Instant::now(),
                    ));
                }
            }
            Incoming::Pong(_) => {}
        }
    }
    fn armorstand_buttons(&self) -> Vec<(String, Option<Command>)> {
        let Some(r) = self.remote.as_ref() else {
            return vec![];
        };
        let Some((_, id, _)) = r.armorstand.active else {
            return vec![];
        };
        let Some(s) = self.armor_stands.iter().find(|s| s.id as i32 == id) else {
            return vec![];
        };
        let label = |s: &str| horizon_client::lang::get(&format!("horizon.armorstands.{s}"));
        let mut b = vec![];
        for (i, value) in [
            s.no_base_plate,
            s.show_arms,
            s.small,
            s.no_gravity,
            s.invisible,
            s.name_visible,
            r.armorstand.invulnerable,
            s.disabled_slots == 4144959,
        ]
        .into_iter()
        .enumerate()
        {
            let key = [
                "base_plate",
                "arms",
                "small",
                "gravity",
                "invisible",
                "name",
                "invulnerable",
                "lock",
            ][i];
            let displayed = if i == 0 || i == 3 { !value } else { value };
            b.push((
                format!(
                    "{}: {}",
                    label(key),
                    horizon_client::lang::get(if displayed { "gui.yes" } else { "gui.no" })
                ),
                Some(Command::Flag(i as i32 + 1, !value)),
            ));
        }
        for action in [
            "copy",
            "paste",
            "prepare",
            "tool_rack",
            "upright_item",
            "flat_item",
            "block",
            "tool",
            "snap_corner",
            "snap_center",
            "snap_standing",
            "snap_sitting",
            "snap_player",
            "face_toward",
            "face_away",
            "face_with",
        ] {
            let wire = match (action, s.small) {
                ("upright_item", true) => "upright_item_small",
                ("flat_item", true) => "flat_item_small",
                ("block", true) => "block_small",
                ("tool", true) => "tool_small",
                _ => action,
            };
            b.push((label(action), Some(Command::Utility(wire))));
        }
        b.push((label("undo"), Some(Command::Undo(false))));
        b.push((label("redo"), Some(Command::Undo(true))));
        b.push((
            format!(
                "{}: {}",
                label("mode"),
                label(["world", "stand", "player"][r.armorstand.mode as usize])
            ),
            None,
        ));
        b.push((
            format!(
                "{}: {}",
                label("units"),
                label(if r.armorstand.units == 0 {
                    "pixels"
                } else {
                    "blocks"
                })
            ),
            None,
        ));
        for d in 0..6 {
            for a in 1..=3 {
                let distance = if r.armorstand.units == 0 {
                    ["1/16", "3/16", "1/2"]
                } else {
                    ["1/4", "1/3", "1"]
                };
                b.push((
                    format!(
                        "{} {}",
                        label(["down", "up", "north", "south", "west", "east"][d]),
                        distance[a - 1]
                    ),
                    Some(Command::Position(
                        d as i32,
                        a as i32,
                        r.armorstand.mode,
                        r.armorstand.units,
                    )),
                ));
            }
        }
        for v in [-45, -15, -5, -1, 1, 5, 15, 45] {
            b.push((format!("{} {v:+}°", label("yaw")), Some(Command::Yaw(v))));
        }
        for v in [0., 90., -180., -90.] {
            b.push((
                format!("{} {v}°", label("yaw")),
                Some(Command::AbsoluteYaw(v)),
            ));
        }
        for p in 0..5 {
            b.push((
                label(["utilities", "move", "rotate", "pose", "presets"][p]),
                Some(Command::Request(id, p as i32)),
            ));
        }
        for &p in armorstand::PRESETS
            .iter()
            .skip(r.armorstand.preset_page * 6)
            .take(6)
        {
            b.push((label(p), Some(Command::Preset(p))));
        }
        b.push((label("previous"), None));
        b.push((label("next"), None));
        b.push((horizon_client::lang::get("gui.done"), None));
        b
    }
    pub(super) fn draw_armorstand(&mut self, batch: &mut HudBatch) {
        let buttons = self.armorstand_buttons();
        let rows = ((batch.height - 26.) / 21.).max(1.) as usize;
        let cols = buttons.len().div_ceil(rows).max(4);
        let bw = (batch.width - 16.) / cols as f32;
        for (i, (label, _)) in buttons.iter().enumerate() {
            let x = 8. + (i % cols) as f32 * bw;
            let y = 8. + (i / cols) as f32 * 21.;
            ui::button(
                batch,
                &self.gui.atlas,
                x,
                y,
                bw - 3.,
                20.,
                &Text::literal(label),
                true,
                ui::hovered(self.cursor, x, y, bw - 3., 20.),
            );
        }
        if let Some((text, color, time)) = self
            .remote
            .as_ref()
            .and_then(|r| r.armorstand.message.as_ref())
        {
            if time.elapsed().as_secs() < 6 {
                batch.text_styled(
                    &self.gui.atlas,
                    &Text::literal(text),
                    8.,
                    batch.height - 12.,
                    *color,
                    true,
                );
            }
        }
    }
    pub(super) fn armorstand_click(&mut self) {
        let size = self
            .renderer
            .as_ref()
            .map_or([1, 1], |r| [r.config.width, r.config.height]);
        let batch = HudBatch::new(size);
        let buttons = self.armorstand_buttons();
        let rows = ((batch.height - 26.) / 21.).max(1.) as usize;
        let cols = buttons.len().div_ceil(rows).max(4);
        let bw = (batch.width - 16.) / cols as f32;
        for (i, (_, command)) in buttons.iter().enumerate() {
            if ui::hovered(
                self.cursor,
                8. + (i % cols) as f32 * bw,
                8. + (i / cols) as f32 * 21.,
                bw - 3.,
                20.,
            ) {
                if let Some(c) = command {
                    self.armorstand_send(c.clone());
                } else if i == 26 {
                    let s = &mut self.remote.as_mut().unwrap().armorstand;
                    s.mode = (s.mode + 1) % 3;
                    s.units = if s.mode == 0 { 0 } else { 1 };
                } else if i == 27 {
                    let s = &mut self.remote.as_mut().unwrap().armorstand;
                    s.units = 1 - s.units;
                } else if i == buttons.len() - 1 {
                    self.armorstand_close(true);
                } else {
                    let s = &mut self.remote.as_mut().unwrap().armorstand;
                    if i == buttons.len() - 3 {
                        s.preset_page = s.preset_page.saturating_sub(1);
                    } else {
                        s.preset_page = (s.preset_page + 1).min(5);
                    }
                }
                return;
            }
        }
    }
}
