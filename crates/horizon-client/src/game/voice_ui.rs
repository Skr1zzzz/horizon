//! Voice settings live inside the pause shell; widgets register the same rectangles used for hits.
use super::{Game, Screen};
use horizon_client::{
    hud::HudBatch,
    text::Text,
    ui,
    voicechat::{groups, players, sender},
    widgets,
};
use horizon_protocol::packets::{configuration::CustomPayload, play::Serverbound};
use std::{path::Path, sync::atomic::Ordering::Relaxed};
use winit::keyboard::KeyCode;
pub(super) struct State {
    pub open: bool,
    from_play: bool,
    pub groups: groups::State,
    pub local: players::Local,
    devices: Vec<String>,
    page: usize,
    group_page: usize,
    name: String,
    password: String,
    field: usize,
    kind: i16,
    join: Option<u128>,
    creating: bool,
    invalid: bool,
    dragging: Option<f32>,
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        State {
            open: true,
            from_play: false,
            groups: Default::default(),
            local: Default::default(),
            devices: vec![],
            page: 0,
            group_page: 0,
            name: String::new(),
            password: String::new(),
            field: 0,
            kind: 0,
            join: None,
            creating: false,
            invalid: false,
            dragging: None,
        }
    }
    #[test]
    fn closing_resumes_only_when_opened_from_play() {
        for from_play in [false, true] {
            let mut s = state();
            s.from_play = from_play;
            s.creating = true;
            s.password = "secret".into();
            s.dragging = Some(74.0);
            assert_eq!(s.close(), from_play);
            assert!(!s.open && !s.creating && s.password.is_empty() && s.dragging.is_none());
        }
    }
    #[test]
    fn player_pages_and_slider_targets_exclude_local_uuid() {
        let mut s = state();
        for id in 1..=4 {
            s.groups.players.insert(
                id,
                groups::Player {
                    id,
                    name: format!("player{id}"),
                    disabled: false,
                    disconnected: false,
                    group: None,
                },
            );
        }
        assert_eq!(s.players(2).map(|p| p.id).collect::<Vec<_>>(), [1, 3, 4]);
        assert_eq!(s.page_count(2), 1);
        assert_eq!(s.page_count(99), 2);
        s.groups.players.clear();
        assert_eq!(s.page_count(2), 0);
    }
}
impl State {
    fn close(&mut self) -> bool {
        self.open = false;
        self.close_prompt();
        self.dragging = None;
        self.from_play
    }
    fn players(&self, local: u128) -> impl Iterator<Item = &groups::Player> {
        self.groups.players.values().filter(move |p| p.id != local)
    }
    fn page_count(&self, local: u128) -> usize {
        if self.group_page == 0 {
            self.players(local).count().div_ceil(3)
        } else {
            self.groups
                .groups
                .values()
                .filter(|g| !g.hidden && Some(g.id) != self.groups.current)
                .count()
                .div_ceil(2)
        }
    }
    pub fn close_prompt(&mut self) {
        self.creating = false;
        self.join = None;
        self.password.clear();
        self.invalid = false;
    }
    pub fn new(path: &Path) -> Self {
        Self {
            open: false,
            from_play: false,
            groups: Default::default(),
            local: players::Local::load(path),
            devices: vec![],
            page: 0,
            group_page: 0,
            name: String::new(),
            password: String::new(),
            field: 0,
            kind: 0,
            join: None,
            creating: false,
            invalid: false,
            dragging: None,
        }
    }
}
fn tr(key: &str) -> String {
    horizon_client::lang::get(&format!("horizon.voice.{key}"))
}
fn button(
    b: &mut HudBatch,
    a: &horizon_client::hud::HudAtlas,
    x: f32,
    y: f32,
    w: f32,
    label: String,
) {
    ui::button(b, a, x, y, w, 20.0, &Text::literal(label), true, false);
}
impl Game<'_> {
    pub(super) fn toggle_voice(&mut self, mute: bool) {
        let Some(v) = self.remote.as_ref().and_then(|r| r.voice.as_ref()) else {
            return;
        };
        let (value, bit, key) = if mute {
            let value = !v.controls.muted.fetch_xor(true, Relaxed);
            (
                value,
                if value { 1 } else { 2 },
                if value {
                    "horizon.voice.muted"
                } else {
                    "horizon.voice.unmuted"
                },
            )
        } else {
            let value = !v.controls.disabled.fetch_xor(true, Relaxed);
            (
                value,
                if value { 4 } else { 8 },
                if value {
                    "horizon.voice.disabled"
                } else {
                    "horizon.voice.enabled"
                },
            )
        };
        let notify = v.controls.notices.fetch_or(bit, Relaxed) & bit == 0;
        if notify {
            self.send_overlay_message(Text::translate(key, &[]));
        }
        if !mute {
            self.send(Serverbound::CustomPayload(CustomPayload {
                channel: "voicechat:update_state".into(),
                data: vec![u8::from(value)],
            }));
        }
    }
    fn close_voice(&mut self) {
        let resume = if let Some(r) = self.remote.as_mut() {
            r.voice_ui.close()
        } else {
            false
        };
        if resume {
            self.pause_menu = false;
            // Use the pause menu's Back to game action, including pointer capture.
            let _ = self.execute_pause_action(0);
        }
    }
    pub(super) fn voice_ready(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.voice.is_some())
    }
    pub(super) fn voice_open(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.voice_ui.open)
    }
    pub(super) fn open_voice(&mut self) {
        if !self.voice_ready() {
            return;
        }
        if let Some(r) = self.remote.as_mut() {
            r.voice_ui.from_play = self.screen == Screen::Playing;
            r.voice_ui.open = true;
            r.voice_ui.devices = sender::input_devices();
        }
        self.screen = Screen::Paused;
        self.pause_menu = true;
        self.capture(false);
    }
    pub(super) fn voice_key(&mut self, key: KeyCode, text: Option<&str>) {
        if key == KeyCode::Escape {
            self.close_voice();
            return;
        }
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let s = &mut r.voice_ui;
        if !s.creating && s.join.is_none() {
            return;
        }
        if key == KeyCode::Tab {
            s.field = 1 - s.field;
            return;
        }
        let target = if s.field == 0 && s.creating {
            &mut s.name
        } else {
            &mut s.password
        };
        if key == KeyCode::Backspace {
            target.pop();
        } else if let Some(t) = text {
            for c in t.chars().filter(|c| !c.is_control()) {
                if target.chars().count() < 24 {
                    target.push(c);
                }
            }
        }
    }
    pub(super) fn draw_voice(&self, b: &mut HudBatch) {
        let Some(r) = self.remote.as_ref() else {
            return;
        };
        let Some(v) = r.voice.as_ref() else {
            return;
        };
        let s = &r.voice_ui;
        let a = &self.gui.atlas;
        let w = b.width;
        let h = b.height;
        ui::menu_background(b, a, 0.0, 0.0, w, h, true);
        widgets::begin_screen(|| widgets::ScreenInfo::new("voicechat", tr("title")));
        let x = (w - 300.0) / 2.0;
        ui::centered_text(b, a, &Text::literal(tr("title")), x, 8.0, 300.0, u32::MAX);
        if s.creating || s.join.is_some() {
            button(b, a, x, 30.0, 300.0, format!("{}: {}", tr("name"), s.name));
            button(
                b,
                a,
                x,
                54.0,
                300.0,
                format!(
                    "{}: {}",
                    tr("password"),
                    "•".repeat(s.password.chars().count())
                ),
            );
            if s.creating {
                button(
                    b,
                    a,
                    x,
                    78.0,
                    300.0,
                    tr(["normal", "open", "isolated"][s.kind as usize]),
                );
                ui::centered_text(
                    b,
                    a,
                    &Text::literal(tr(
                        ["normal_help", "open_help", "isolated_help"][s.kind as usize]
                    )),
                    x,
                    102.0,
                    300.0,
                    u32::MAX,
                );
            }
            button(b, a, x, 126.0, 146.0, tr("confirm"));
            button(b, a, x + 154.0, 126.0, 146.0, tr("cancel"));
            if s.invalid || s.groups.wrong_password {
                ui::centered_text(
                    b,
                    a,
                    &Text::literal(tr(if s.groups.wrong_password {
                        "wrong_password"
                    } else {
                        "invalid"
                    })),
                    x,
                    154.0,
                    300.0,
                    0xffff5555,
                );
            }
            return;
        }
        button(
            b,
            a,
            x,
            26.0,
            146.0,
            tr(if v.controls.muted.load(Relaxed) {
                "unmute"
            } else {
                "mute"
            }),
        );
        button(
            b,
            a,
            x + 154.0,
            26.0,
            146.0,
            tr(if v.controls.disabled.load(Relaxed) {
                "enable"
            } else {
                "disable"
            }),
        );
        button(
            b,
            a,
            x,
            50.0,
            300.0,
            tr(if self.settings.file.bool("voicechatActivation") {
                "activation"
            } else {
                "ptt"
            }),
        );
        let threshold = self.settings.file.int("voicechatThreshold");
        ui::slider(
            b,
            a,
            x,
            74.0,
            300.0,
            &Text::literal(format!("{}: {threshold} dB", tr("threshold"))),
            (threshold + 100) as f32 / 100.0,
            false,
            true,
            false,
        );
        let level = f32::from_bits(v.controls.level.load(Relaxed));
        b.fill(a, x, 96.0, 300.0, 3.0, 0xff333333);
        let meter = ((20.0 * level.max(0.00001).log10() + 100.0) / 100.0).clamp(0.0, 1.0);
        b.fill(a, x, 96.0, 300.0 * meter, 3.0, 0xff55ff55);
        let device = self.settings.file.string("voicechatInputDevice");
        button(
            b,
            a,
            x,
            102.0,
            300.0,
            format!(
                "{}: {}",
                tr("device"),
                if device.is_empty() {
                    tr("default")
                } else {
                    device
                }
            ),
        );
        let volume = self.settings.file.double("soundCategory_voice") as f32;
        ui::slider(
            b,
            a,
            x,
            126.0,
            300.0,
            &Text::literal(format!("{}: {:.0}%", tr("volume"), volume * 100.0)),
            volume,
            false,
            true,
            false,
        );
        button(b, a, x, 150.0, 146.0, tr("players"));
        if s.groups.enabled {
            button(b, a, x + 154.0, 150.0, 146.0, tr("groups"));
        }
        if s.group_page == 0 {
            for (i, p) in s.players(r.profile).skip(s.page * 3).take(3).enumerate() {
                let status = if p.disabled {
                    tr("disabled")
                } else if p.disconnected {
                    tr("disconnected")
                } else {
                    tr("connected")
                };
                let gain = s.local.gain(p.id);
                let label = format!("{} — {}: {:.0}%", p.name, status, gain * 100.0);
                ui::slider(
                    b,
                    a,
                    x,
                    176.0 + i as f32 * 24.0,
                    300.0,
                    &Text::literal(label),
                    gain / 2.0,
                    false,
                    true,
                    false,
                );
            }
        } else {
            let current = s
                .groups
                .current
                .and_then(|id| s.groups.groups.get(&id))
                .map(|g| g.name.clone())
                .unwrap_or_else(|| tr("none"));
            ui::centered_text(
                b,
                a,
                &Text::literal(format!("{}: {current}", tr("current"))),
                x,
                174.0,
                300.0,
                u32::MAX,
            );
            let members = s
                .groups
                .players
                .values()
                .filter(|p| p.group.is_some() && p.group == s.groups.current)
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ui::centered_text(b, a, &Text::literal(members), x, 186.0, 300.0, u32::MAX);
            button(b, a, x, 200.0, 146.0, tr("create"));
            button(b, a, x + 154.0, 200.0, 146.0, tr("leave"));
            for (i, g) in s
                .groups
                .groups
                .values()
                .filter(|g| !g.hidden && Some(g.id) != s.groups.current)
                .skip(s.page * 2)
                .take(2)
                .enumerate()
            {
                button(
                    b,
                    a,
                    x,
                    224.0 + i as f32 * 24.0,
                    300.0,
                    format!("{}: {}", tr("join"), g.name),
                );
            }
        }
        let y = h - 24.0;
        ui::button(
            b,
            a,
            x,
            y,
            94.0,
            20.0,
            &Text::literal(tr("previous")),
            s.page > 0,
            false,
        );
        ui::button(
            b,
            a,
            x + 102.0,
            y,
            94.0,
            20.0,
            &Text::literal(tr("next")),
            s.page + 1 < s.page_count(r.profile),
            false,
        );
        button(b, a, x + 204.0, y, 96.0, tr("done"));
    }
    fn voice_option(&mut self, key: &str, value: horizon_client::options_file::V) {
        if key == "soundCategory_voice" {
            if let horizon_client::options_file::V::Double(volume) = &value {
                if let Some(index) = horizon_client::settings::SOUND_SOURCES
                    .iter()
                    .position(|s| *s == "voice")
                {
                    self.settings.volumes[index] = *volume as f32;
                }
            }
        }
        self.settings.file.set(key, value);
        self.settings.save(&Self::options_path());
    }
    fn voice_command(&self, command: groups::Command<'_>) {
        if let Ok((channel, data)) = command.encode() {
            self.send(Serverbound::CustomPayload(CustomPayload {
                channel: channel.into(),
                data,
            }));
        }
    }
    pub(super) fn voice_pointer(&mut self, pressed: bool) {
        if !pressed {
            if let Some(r) = self.remote.as_mut() {
                r.voice_ui.dragging = None;
            }
            return;
        }
        let (_, cy) = self.cursor;
        let row = self.remote.as_ref().and_then(|r| {
            let s = &r.voice_ui;
            if s.creating || s.join.is_some() {
                return None;
            }
            [74.0, 126.0]
                .into_iter()
                .chain(
                    (0..3)
                        .filter(|_| s.group_page == 0)
                        .map(|i| 176.0 + i as f32 * 24.0),
                )
                .find(|y| cy >= *y && cy < *y + 20.0)
        });
        if let Some(r) = self.remote.as_mut() {
            r.voice_ui.dragging = row;
        }
        self.voice_click();
    }
    pub(super) fn voice_drag(&mut self) {
        let row = self.remote.as_ref().and_then(|r| r.voice_ui.dragging);
        if let Some(row) = row {
            let previous = self.cursor;
            let (w, _) = self.gui_size();
            self.cursor = (
                (previous.0).clamp((w - 300.0) / 2.0, (w + 300.0) / 2.0 - 0.01),
                row + 10.0,
            );
            self.voice_click();
            self.cursor = previous;
        }
    }
    pub(super) fn voice_click(&mut self) {
        let (w, h) = self.gui_size();
        let x = (w - 300.0) / 2.0;
        let (cx, cy) = self.cursor;
        let hit = |y: f32| cy >= y && cy < y + 20.0 && cx >= x && cx < x + 300.0;
        let Some(r) = self.remote.as_ref() else {
            return;
        };
        if cx < x || cx >= x + 300.0 {
            return;
        }
        let s = &r.voice_ui;
        if s.creating || s.join.is_some() {
            if hit(30.0) {
                self.remote.as_mut().unwrap().voice_ui.field = 0;
            } else if hit(54.0) {
                self.remote.as_mut().unwrap().voice_ui.field = 1;
            } else if hit(78.0) && s.creating {
                let kind = (s.kind + 1) % 3;
                self.remote.as_mut().unwrap().voice_ui.kind = kind;
            } else if hit(126.0) {
                if cx >= x + 154.0 {
                    let s = &mut self.remote.as_mut().unwrap().voice_ui;
                    s.creating = false;
                    s.join = None;
                    s.password.clear();
                } else {
                    let valid = (!s.creating || groups::valid(&s.name))
                        && (s.password.is_empty() || groups::valid(&s.password));
                    if valid {
                        if s.creating {
                            self.voice_command(groups::Command::Create(
                                &s.name,
                                &s.password,
                                s.kind,
                            ));
                        } else if let Some(id) = s.join {
                            self.voice_command(groups::Command::Join(id, &s.password));
                        }
                        // Keep password prompts open until an acknowledgement, including a wrong-password response.
                        let s = &mut self.remote.as_mut().unwrap().voice_ui;
                        s.invalid = false;
                        s.groups.wrong_password = false;
                    } else {
                        self.remote.as_mut().unwrap().voice_ui.invalid = true;
                    }
                }
            }
            return;
        }
        if hit(26.0) {
            self.toggle_voice(cx < x + 150.0);
        } else if hit(50.0) {
            let active = !self.settings.file.bool("voicechatActivation");
            self.voice_option(
                "voicechatActivation",
                horizon_client::options_file::V::Bool(active),
            );
            if let Some(v) = self.remote.as_ref().and_then(|r| r.voice.as_ref()) {
                v.controls.activation.store(active, Relaxed);
            }
        } else if hit(74.0) {
            let value = (ui::slider_value(x, 300.0, cx) * 100.0).round() as i32 - 100;
            self.voice_option(
                "voicechatThreshold",
                horizon_client::options_file::V::Int(value),
            );
            if let Some(v) = self.remote.as_ref().and_then(|r| r.voice.as_ref()) {
                v.controls.threshold.store(value, Relaxed);
            }
        } else if hit(102.0) {
            let current = self.settings.file.string("voicechatInputDevice");
            let next = s
                .devices
                .iter()
                .position(|d| d == &current)
                .map_or(0, |i| i + 1);
            let device = s.devices.get(next).cloned().unwrap_or_default();
            self.voice_option(
                "voicechatInputDevice",
                horizon_client::options_file::V::Str(device.clone()),
            );
            if let Some(v) = self.remote.as_ref().and_then(|r| r.voice.as_ref()) {
                if let Ok(mut d) = v.controls.device.lock() {
                    *d = Some(device);
                }
            }
        } else if hit(126.0) {
            let value = ui::slider_value(x, 300.0, cx);
            self.voice_option(
                "soundCategory_voice",
                horizon_client::options_file::V::Double(value as f64),
            );
        } else if hit(150.0) {
            let s = &mut self.remote.as_mut().unwrap().voice_ui;
            s.group_page = usize::from(cx >= x + 154.0 && s.groups.enabled);
            s.page = 0;
        } else if hit(h - 24.0) {
            let local = r.profile;
            let s = &mut self.remote.as_mut().unwrap().voice_ui;
            if cx < x + 94.0 {
                s.page = s.page.saturating_sub(1);
            } else if cx < x + 196.0 {
                let count = s.page_count(local);
                s.page = (s.page + 1).min(count.saturating_sub(1));
            } else {
                self.close_voice();
            }
        } else if s.group_page == 0 {
            let i = ((cy - 176.0) / 24.0).floor() as usize;
            // Resolved before the mutable borrow below: the player iterator borrows `remote`.
            let target = (176.0..248.0)
                .contains(&cy)
                .then(|| s.players(r.profile).nth(s.page * 3 + i).map(|p| p.id))
                .flatten();
            if let Some(id) = target {
                let gain = ui::slider_value(x, 300.0, cx) * 2.0;
                let s = &mut self.remote.as_mut().unwrap().voice_ui;
                s.local.volumes.insert(id, gain);
                if let Err(e) = s
                    .local
                    .save(&Self::options_path().with_file_name("voice-player-volumes.txt"))
                {
                    log::warn!("voicechat: volume save: {e}");
                }
            }
        } else if hit(200.0) {
            if cx < x + 150.0 {
                let s = &mut self.remote.as_mut().unwrap().voice_ui;
                s.creating = true;
                s.name.clear();
                s.password.clear();
                s.field = 0;
                s.invalid = false;
            } else {
                self.voice_command(groups::Command::Leave);
            }
        } else if cy >= 224.0 && cy < 268.0 {
            let i = ((cy - 224.0) / 24.0).floor() as usize;
            if let Some(g) = s
                .groups
                .groups
                .values()
                .filter(|g| !g.hidden && Some(g.id) != s.groups.current)
                .nth(s.page * 2 + i)
            {
                if g.password {
                    let id = g.id;
                    let s = &mut self.remote.as_mut().unwrap().voice_ui;
                    s.join = Some(id);
                    s.password.clear();
                    s.field = 1;
                    s.invalid = false;
                    s.groups.wrong_password = false;
                } else {
                    self.voice_command(groups::Command::Join(g.id, ""));
                }
            }
        }
    }
}
