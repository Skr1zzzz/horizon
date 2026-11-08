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
            if !matches!(self.screen, Screen::Mod | Screen::Inventory)
                || !self.armor_stands.iter().any(|s| s.id as i32 == id)
            {
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
                    let layout = horizon_client::net::pod::Menu {
                        sync_id: sync,
                        menu: "armorstands:inventory".into(),
                        title: String::new(),
                        slots: armorstand::inventory_slots(),
                        data_slots: 0,
                    };
                    let r = self.remote.as_mut().unwrap();
                    r.armorstand.open(sync, entity, page);
                    r.menu = Some((sync, layout.menu.clone()));
                    r.pending_pod = None;
                    self.open_pod_screen(&layout);
                    self.remote.as_mut().unwrap().pod_menu = Some(layout);
                    self.screen = Screen::Inventory;
                    self.capture(false);
                    return;
                }
                self.remote
                    .as_mut()
                    .unwrap()
                    .armorstand
                    .open(sync, entity, page);
                self.armorstand_takeover(sync, "armorstands open payload");
                // Only committed previews send pings; opening cannot leave an older pong in flight.
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
            Incoming::Pong(_) => {
                let session = &mut self.remote.as_mut().unwrap().armorstand;
                if session.preview.as_ref().is_some_and(|p| p.waiting) {
                    session.preview = None;
                }
            }
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
        if r.armorstand.active.is_some_and(|s| s.2 == 3) {
            for key in [
                "head",
                "right_arm",
                "body",
                "left_arm",
                "right_leg",
                "left_leg",
                "range",
                "mirror",
            ] {
                let command = if key == "mirror" {
                    let pose = std::array::from_fn(|i| s.pose[armorstand::PART_INDEX[i]]);
                    Some(Command::FullPose(armorstand::mirror(pose)))
                } else {
                    None
                };
                let text = if key == "range" {
                    format!(
                        "{}: {}",
                        label(key),
                        label(["full", "half", "tight"][r.armorstand.range])
                    )
                } else {
                    label(key)
                };
                b.push((text, command));
            }
            for p in 0..6 {
                b.push((
                    label(
                        [
                            "utilities",
                            "move",
                            "rotate",
                            "pose",
                            "presets",
                            "inventory",
                        ][p],
                    ),
                    Some(Command::Request(id, p as i32)),
                ));
            }
            b.push((horizon_client::lang::get("gui.done"), None));
            return b;
        }
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
        for p in 0..6 {
            b.push((
                label(
                    [
                        "utilities",
                        "move",
                        "rotate",
                        "pose",
                        "presets",
                        "inventory",
                    ][p],
                ),
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
        let page = r.armorstand.active.unwrap().2;
        b.into_iter()
            .enumerate()
            .filter_map(|(i, button)| {
                let show = match page {
                    0 => i < 26,
                    1 => (8..10).contains(&i) || (16..21).contains(&i) || (24..46).contains(&i),
                    2 => (21..26).contains(&i) || (46..58).contains(&i),
                    4 => i >= 64,
                    5 => i == 1 || i == 7,
                    _ => false,
                };
                (show || matches!(button.1, Some(Command::Request(_, _))) || i == 72)
                    .then_some(button)
            })
            .collect()
    }
    pub(super) fn armorstand_wheel(&mut self, delta: f32) -> bool {
        let Some(s) = self.remote.as_ref().map(|r| &r.armorstand) else {
            return false;
        };
        if !s.active.is_some_and(|s| s.2 == 3) {
            return false;
        }
        let size = self
            .renderer
            .as_ref()
            .map_or([1, 1], |r| [r.config.width, r.config.height]);
        let batch = HudBatch::new(size);
        for axis in 0..4 {
            if ui::hovered(
                self.cursor,
                batch.width - 228.,
                batch.height.max(240.) - 102. + axis as f32 * 22.,
                132.,
                20.,
            ) {
                if s.preview.as_ref().is_some_and(|p| p.waiting || p.dragging) {
                    return true;
                }
                let server = if axis == 3 {
                    s.scale
                } else {
                    self.armor_stands
                        .iter()
                        .find(|p| p.id as i32 == s.active.unwrap().1)
                        .unwrap()
                        .pose[armorstand::PART_INDEX[s.part]][axis]
                };
                let value = s
                    .preview
                    .as_ref()
                    .filter(|p| p.axis == axis)
                    .map_or(server, |p| p.reflected(server));
                let step = if axis == 3 {
                    if value < 2. { 0.1 } else { 0.5 }
                } else {
                    1.
                };
                self.armorstand_preview(axis, value + delta.signum() * step, false, true);
                return true;
            }
        }
        false
    }
    fn armorstand_preview(&mut self, axis: usize, value: f32, dragging: bool, wheel: bool) {
        let s = &mut self.remote.as_mut().unwrap().armorstand;
        if s.preview.as_ref().is_some_and(|p| p.waiting) {
            return;
        }
        let limit = [180., 90., 35.][s.range];
        s.preview = Some(armorstand::Preview {
            part: s.part,
            axis,
            value: if axis == 3 {
                value.clamp(0.01, 10.)
            } else {
                value.clamp(-limit, limit)
            },
            dragging,
            wheel: wheel.then(std::time::Instant::now),
            waiting: false,
        });
    }
    pub(super) fn armorstand_commit(&mut self) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let Some(p) = r.armorstand.preview.as_mut() else {
            return;
        };
        if p.waiting {
            return;
        }
        p.dragging = false;
        p.wheel = None;
        p.waiting = true;
        let command = if p.axis == 3 {
            armorstand::diagnostic(format_args!("scale value={}", p.value));
            r.armorstand.scale = p.value;
            Command::Scale(p.value)
        } else {
            armorstand::diagnostic(format_args!(
                "pose part={} axis={} value={}",
                p.part, p.axis, p.value
            ));
            Command::Pose(p.part as i32, p.axis as i32, p.value)
        };
        self.armorstand_send(command);
        let uuid = self
            .remote_player_uuid(self.remote.as_ref().unwrap().entity_id as u32)
            .unwrap_or(0);
        // Queue the barrier after the edit so older metadata cannot complete this preview.
        self.armorstand_send(Command::Ping(uuid));
    }
    fn armorstand_slider_click(&mut self, batch: &HudBatch) -> bool {
        let s = &self.remote.as_ref().unwrap().armorstand;
        if !s.active.is_some_and(|s| s.2 == 3) {
            return false;
        }
        for axis in 0..4 {
            let y = batch.height.max(240.) - 102. + axis as f32 * 22.;
            let x = batch.width - 228.;
            if ui::hovered(self.cursor, x, y, 220., 20.) {
                let value = if axis == 3 {
                    s.scale
                } else {
                    let id = s.active.unwrap().1;
                    self.armor_stands
                        .iter()
                        .find(|p| p.id as i32 == id)
                        .unwrap()
                        .pose[armorstand::PART_INDEX[s.part]][axis]
                };
                if self.cursor.0 < x + 132. {
                    let position = ui::slider_value(x, 132., self.cursor.0);
                    let value = if axis == 3 {
                        armorstand::scale_value(position)
                    } else {
                        armorstand::pose_value(position, s.range)
                    };
                    self.armorstand_preview(axis, value, true, false);
                } else {
                    let up = self.cursor.0 >= x + 160.;
                    let step = if axis != 3 {
                        1.
                    } else if value < 2. || (!up && (value - 2.).abs() < 0.00001) {
                        0.1
                    } else {
                        0.5
                    };
                    let v = if self.cursor.0 < x + 160. {
                        ((value / step).ceil() - 1.) * step
                    } else if self.cursor.0 < x + 188. {
                        ((value / step).floor() + 1.) * step
                    } else {
                        if axis == 3 { 1. } else { 0. }
                    };
                    self.armorstand_preview(axis, v, false, false);
                    self.armorstand_commit();
                }
                return true;
            }
        }
        false
    }
    fn armorstand_preview_tick(&mut self, batch: &mut HudBatch) {
        let Some(r) = self.remote.as_ref() else {
            return;
        };
        let s = &r.armorstand;
        if !s.active.is_some_and(|s| s.2 == 3) {
            return;
        }
        let id = s.active.unwrap().1;
        if let Some(p) = &s.preview {
            if p.wheel.is_some_and(|t| t.elapsed().as_millis() >= 500) {
                self.armorstand_commit();
            }
        }
        let s = &mut self.remote.as_mut().unwrap().armorstand;
        if let Some(p) = &mut s.preview {
            if p.dragging {
                let pos = ui::slider_value(batch.width - 228., 132., self.cursor.0);
                p.value = if p.axis == 3 {
                    armorstand::scale_value(pos)
                } else {
                    armorstand::pose_value(pos, s.range)
                };
            }
            if let Some(stand) = self.armor_stands.iter_mut().find(|p| p.id as i32 == id) {
                if p.axis != 3 {
                    let value = &mut stand.pose[armorstand::PART_INDEX[p.part]][p.axis];
                    *value = p.reflected(*value);
                } else {
                    s.scale = p.value;
                }
            }
        }
        let Some(stand) = self.armor_stands.iter().find(|p| p.id as i32 == id) else {
            return;
        };
        for axis in 0..4 {
            let value = if axis == 3 {
                s.scale
            } else {
                stand.pose[armorstand::PART_INDEX[s.part]][axis]
            };
            let name = horizon_client::lang::get(&format!(
                "horizon.armorstands.{}",
                ["pitch", "yaw", "roll", "scale"][axis]
            ));
            let y = batch.height.max(240.) - 102. + axis as f32 * 22.;
            let x = batch.width - 228.;
            ui::slider(
                batch,
                &self.gui.atlas,
                x,
                y,
                132.,
                &Text::literal(format!("{name}: {value:.2}")),
                if axis == 3 {
                    armorstand::scale_position(value)
                } else {
                    armorstand::pose_position(value, s.range)
                },
                ui::hovered(self.cursor, x, y, 132., 20.),
                true,
                s.preview
                    .as_ref()
                    .is_some_and(|p| p.axis == axis && p.dragging),
            );
            for (i, suffix) in ["-", "+", "0"].iter().enumerate() {
                let bx = x + 132. + i as f32 * 28.;
                ui::button(
                    batch,
                    &self.gui.atlas,
                    bx,
                    y,
                    28.,
                    20.,
                    &Text::literal(""),
                    true,
                    ui::hovered(self.cursor, bx, y, 28., 20.),
                );
                horizon_client::widgets::record(
                    &format!("{name} {suffix}"),
                    [bx, y, 28., 20.],
                    true,
                );
                batch.text(&self.gui.atlas, suffix, bx + 11., y + 6., u32::MAX, true);
            }
        }
    }
    fn armorstand_layout(
        &self,
        batch: &HudBatch,
        buttons: &[(String, Option<Command>)],
    ) -> Vec<(f32, f32, f32)> {
        let tab_width = buttons
            .iter()
            .filter(|b| matches!(b.1, Some(Command::Request(_, _))))
            .map(|(s, _)| HudBatch::text_width(&self.gui.atlas, s) + 16.)
            .fold(100., f32::max);
        let mut tab = 0;
        let mut control = 0;
        // Reserve the upper-right tabs and the lower-right pose rows independently.
        let pose = self
            .remote
            .as_ref()
            .is_some_and(|r| r.armorstand.active.is_some_and(|s| s.2 == 3));
        let available = if pose {
            batch.width - 244.
        } else {
            batch.width - tab_width - 20.
        };
        let max_rows = ((batch.height - 16.) / 21.).floor().max(1.) as usize;
        let count = buttons
            .iter()
            .filter(|b| !matches!(b.1, Some(Command::Request(_, _))))
            .count();
        let cols = count.div_ceil(max_rows).max(1);
        let rows = count.div_ceil(cols);
        let mut widths = vec![0f32; cols];
        for (i, (label, _)) in buttons
            .iter()
            .filter(|b| !matches!(b.1, Some(Command::Request(_, _))))
            .enumerate()
        {
            widths[i / rows] =
                widths[i / rows].max(HudBatch::text_width(&self.gui.atlas, label) + 12.);
        }
        debug_assert!(
            widths.iter().sum::<f32>() <= available,
            "armor stand controls exceed reserved area"
        );
        buttons
            .iter()
            .map(|(_, command)| {
                if matches!(command, Some(Command::Request(_, _))) {
                    let result = (
                        batch.width - tab_width - 8.,
                        8. + tab as f32 * 21.,
                        tab_width,
                    );
                    tab += 1;
                    result
                } else {
                    let result = (
                        8. + widths[..control / rows].iter().sum::<f32>(),
                        (batch.height - rows as f32 * 21. - 8.).max(8.)
                            + (control % rows) as f32 * 21.,
                        widths[control / rows],
                    );
                    control += 1;
                    result
                }
            })
            .collect()
    }
    pub(super) fn draw_armorstand(&mut self, batch: &mut HudBatch) {
        self.armorstand_preview_tick(batch);
        let buttons = self.armorstand_buttons();
        let layout = self.armorstand_layout(&batch, &buttons);
        for (i, (label, _)) in buttons.iter().enumerate() {
            let (x, y, bw) = layout[i];
            ui::button(
                batch,
                &self.gui.atlas,
                x,
                y,
                bw - 3.,
                20.,
                &Text::literal(label),
                true,
                ui::hovered(self.cursor, x, y, bw - 3., 20.)
                    || self.remote.as_ref().is_some_and(|r| {
                        r.armorstand.active.is_some_and(|s| s.2 == 3)
                            && i < 6
                            && [0, 2, 1, 3, 4, 5][i] == r.armorstand.part
                    }),
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
        if self.armorstand_slider_click(&batch) {
            return;
        }
        let buttons = self.armorstand_buttons();
        let layout = self.armorstand_layout(&batch, &buttons);
        for (i, (_, command)) in buttons.iter().enumerate() {
            if ui::hovered(self.cursor, layout[i].0, layout[i].1, layout[i].2 - 3., 20.) {
                if let Some(c) = command {
                    self.armorstand_send(c.clone());
                } else if self.remote.as_ref().unwrap().armorstand.active.unwrap().2 == 3 && i < 8 {
                    match i {
                        0..=5 => {
                            self.remote.as_mut().unwrap().armorstand.part = [0, 2, 1, 3, 4, 5][i]
                        }
                        6 => {
                            let s = &mut self.remote.as_mut().unwrap().armorstand;
                            s.range = (s.range + 1) % 3;
                        }
                        _ => {}
                    }
                } else if buttons[i]
                    .0
                    .starts_with(&horizon_client::lang::get("horizon.armorstands.mode"))
                {
                    let s = &mut self.remote.as_mut().unwrap().armorstand;
                    s.mode = (s.mode + 1) % 3;
                    s.units = if s.mode == 0 { 0 } else { 1 };
                } else if buttons[i]
                    .0
                    .starts_with(&horizon_client::lang::get("horizon.armorstands.units"))
                {
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
