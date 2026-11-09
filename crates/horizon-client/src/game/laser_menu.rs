//! Server-authoritative controls: rejected commands must not change the displayed config.
use super::{Game, Screen};
use horizon_client::{clipboard::Clipboard, hud::HudBatch, laser::Config, text::Text, ui};

const COLORS: [(&str, u32); 9] = [
    ("red", 0xff3030),
    ("orange", 0xff8000),
    ("yellow", 0xffe000),
    ("green", 0x30ff30),
    ("cyan", 0x30ffff),
    ("blue", 0x3060ff),
    ("purple", 0xb030ff),
    ("pink", 0xff60c0),
    ("white", 0xffffff),
];

#[derive(Clone)]
enum Change {
    Mode(&'static str),
    Toggle(&'static str, bool),
    Number(&'static str, f64, bool),
    Color(String),
    Style(bool),
    Paste,
    Done,
}
impl Change {
    fn command(&self) -> Option<String> {
        let value = match self {
            Self::Mode(mode) => return Some(format!("/lasereyes mode {mode}")),
            Self::Toggle(key, value) => format!("{key} {value}"),
            Self::Number(key, value, integer) => {
                let value = if *key == "eyeoffset" {
                    value.clamp(-4.0, 4.0)
                } else {
                    value.max(0.0)
                };
                if *integer {
                    format!("{key} {}", value as i32)
                } else {
                    format!("{key} {value:.1}")
                }
            }
            Self::Color(value) => format!("color {value}"),
            Self::Style(custom) => {
                format!("beamstyle {}", if *custom { "custom" } else { "beacon" })
            }
            Self::Paste | Self::Done => return None,
        };
        Some(format!("/lasereyes config {value}"))
    }
}

fn hex_command(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("/lasereyes config color #{hex}"))
}

struct Button {
    rect: [f32; 4],
    label: Text,
    change: Change,
    swatch: Option<u32>,
    active: bool,
}
fn buttons(c: &Config, w: f32, h: f32) -> Vec<Button> {
    // Independent columns prevent palette targets from covering number controls.
    let width = (w - 12.0).min(900.0);
    let x = (w - width) / 2.0;
    let col = (width - 12.0) / 3.0;
    let step = ((h - 26.0) / 12.0).min(22.0);
    let bh = (step - 2.0).min(20.0);
    let mut out = Vec::new();
    let mut add = |column: usize, row: usize, key: &str, args: &[Text], change, swatch| {
        out.push(Button {
            rect: [
                x + column as f32 * (col + 6.0),
                24.0 + row as f32 * step,
                col,
                bh,
            ],
            label: Text::translate(key, args),
            active: match &change {
                Change::Mode("damage") => c.break_blocks || c.ignite_blocks,
                Change::Mode("burn") => !c.break_blocks || !c.ignite_blocks,
                Change::Color(_) => swatch != Some(c.color & 0xffffff),
                _ => true,
            },
            change,
            swatch,
        });
    };
    add(
        0,
        0,
        "horizon.laser.mode_damage",
        &[],
        Change::Mode("damage"),
        None,
    );
    add(
        1,
        0,
        "horizon.laser.mode_burn",
        &[],
        Change::Mode("burn"),
        None,
    );
    for (row, key, value) in [
        (1, "breakblocks", c.break_blocks),
        (2, "igniteblocks", c.ignite_blocks),
        (3, "igniteentities", c.ignite_entities),
        (4, "stablebeam", c.stable),
    ] {
        add(
            0,
            row,
            &format!("horizon.laser.{key}"),
            &[Text::translate(
                if value { "options.on" } else { "options.off" },
                &[],
            )],
            Change::Toggle(key, !value),
            None,
        );
    }
    add(
        0,
        5,
        "horizon.laser.beamstyle",
        &[Text::translate(
            if c.custom {
                "horizon.laser.custom"
            } else {
                "horizon.laser.beacon"
            },
            &[],
        )],
        Change::Style(!c.custom),
        None,
    );
    // Beam style has a string-valued command, unlike the boolean switches.
    for (row, key, value, delta, integer) in [
        (1, "damage", c.damage, 1.0, false),
        (2, "range", c.range, 8.0, false),
        (3, "igniteseconds", f64::from(c.ignite_seconds), 1.0, true),
        (4, "burnthreshold", f64::from(c.burn_threshold), 2.0, true),
        (5, "eyeoffset", f64::from(c.eye_offset), 1.0, true),
    ] {
        for (column, sign) in [(0, -1.0), (1, 1.0)] {
            add(
                column,
                row + 5,
                &format!(
                    "horizon.laser.{key}_{}",
                    if sign < 0.0 { "minus" } else { "plus" }
                ),
                &[Text::literal(if integer {
                    format!("{value:.0}")
                } else {
                    format!("{value:.1}")
                })],
                Change::Number(key, value + delta * sign, integer),
                None,
            );
        }
    }
    for (i, (name, rgb)) in COLORS.iter().enumerate() {
        add(
            2,
            i + 1,
            &format!("horizon.laser.color_{name}"),
            &[],
            Change::Color((*name).into()),
            Some(*rgb),
        );
    }
    add(
        2,
        10,
        "horizon.laser.hex",
        &[Text::literal(format!("#{:06X}", c.color & 0xffffff))],
        Change::Paste,
        None,
    );
    add(0, 11, "gui.done", &[], Change::Done, None);
    out
}

impl Game<'_> {
    pub(super) fn laser_menu_key(&mut self, raw: winit::keyboard::KeyCode) -> bool {
        if self.screen != Screen::Playing
            || !self.remote.as_ref().is_some_and(|r| r.laser.available)
        {
            return false;
        }
        if !horizon_client::keybinds::key_name(raw)
            .is_some_and(|name| self.settings.keys.get("key.laser-eyes.config") == name)
        {
            return false;
        }
        self.release_laser();
        self.screen = Screen::LaserConfig;
        self.capture(false);
        true
    }
    pub(super) fn close_laser_menu(&mut self) {
        self.screen = Screen::Playing;
        self.capture(true);
    }
    pub(super) fn draw_laser_menu(&mut self, batch: &mut HudBatch) {
        horizon_client::widgets::begin_screen(|| {
            horizon_client::widgets::ScreenInfo::new(
                "laser_config",
                super::screen_info::tr("horizon.laser.title", &[]),
            )
        });
        let Some(r) = self.remote.as_ref().filter(|r| r.laser.available) else {
            return;
        };
        let atlas = &self.gui.atlas;
        ui::menu_background(batch, atlas, 0.0, 0.0, batch.width, batch.height, true);
        batch.text_styled(
            atlas,
            &Text::translate("horizon.laser.title", &[]),
            8.0,
            8.0,
            u32::MAX,
            true,
        );
        for b in buttons(&r.laser.config, batch.width, batch.height) {
            let [x, y, w, h] = b.rect;
            ui::button(
                batch,
                atlas,
                x,
                y,
                w,
                h,
                &b.label,
                b.active,
                ui::hovered(self.cursor, x, y, w, h),
            );
            if let Some(rgb) = b.swatch {
                batch.fill(
                    atlas,
                    x + 3.0,
                    y + 3.0,
                    8.0,
                    h - 6.0,
                    horizon_client::hud::rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255),
                );
            }
        }
    }
    pub(super) fn laser_menu_click(&mut self) {
        let Some(r) = self.remote.as_ref().filter(|r| r.laser.available) else {
            return;
        };
        let (w, h) = self.gui_size();
        let Some(b) = buttons(&r.laser.config, w, h).into_iter().find(|b| {
            let [x, y, w, h] = b.rect;
            b.active && ui::hovered(self.cursor, x, y, w, h)
        }) else {
            return;
        };
        self.audio.play_button_click();
        let command = match b.change {
            Change::Done => {
                self.close_laser_menu();
                return;
            }
            Change::Paste => hex_command(&self.clipboard.get()),
            change => change.command(),
        };
        if let Some(command) = command {
            // LaserConfigScreen sends it as typed (sendChatCommand), not as an unattended
            // click: no admin-command confirmation pops over the menu.
            self.send_command(command.strip_prefix('/').unwrap_or(&command));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_mode_and_color_are_inactive() {
        let mut config = Config::default();
        for (breaking, igniting, selected) in [
            (false, false, Some("damage")),
            (true, true, Some("burn")),
            (true, false, None),
            (false, true, None),
        ] {
            config.break_blocks = breaking;
            config.ignite_blocks = igniting;
            for button in buttons(&config, 900.0, 320.0) {
                if let Change::Mode(mode) = button.change {
                    assert_eq!(!button.active, selected == Some(mode));
                }
                if let Some(rgb) = button.swatch {
                    assert_eq!(!button.active, rgb == config.color);
                }
            }
        }
        config.color = 0x123456;
        assert!(
            buttons(&config, 900.0, 320.0)
                .iter()
                .filter(|b| b.swatch.is_some())
                .all(|b| b.active)
        );
    }
    #[test]
    fn java_menu_commands() {
        for (change, expected) in [
            (Change::Style(true), "/lasereyes config beamstyle custom"),
            (Change::Style(false), "/lasereyes config beamstyle beacon"),
            (Change::Mode("damage"), "/lasereyes mode damage"),
            (Change::Mode("burn"), "/lasereyes mode burn"),
            (
                Change::Toggle("breakblocks", false),
                "/lasereyes config breakblocks false",
            ),
            (
                Change::Toggle("igniteblocks", true),
                "/lasereyes config igniteblocks true",
            ),
            (
                Change::Toggle("igniteentities", false),
                "/lasereyes config igniteentities false",
            ),
            (
                Change::Toggle("stablebeam", true),
                "/lasereyes config stablebeam true",
            ),
            (
                Change::Number("damage", 4.0, false),
                "/lasereyes config damage 4.0",
            ),
            (
                Change::Number("range", 104.0, false),
                "/lasereyes config range 104.0",
            ),
            (
                Change::Number("igniteseconds", 7.0, true),
                "/lasereyes config igniteseconds 7",
            ),
            (
                Change::Number("burnthreshold", 26.0, true),
                "/lasereyes config burnthreshold 26",
            ),
            (
                Change::Number("eyeoffset", -5.0, true),
                "/lasereyes config eyeoffset -4",
            ),
            (
                Change::Number("eyeoffset", 5.0, true),
                "/lasereyes config eyeoffset 4",
            ),
            (
                Change::Number("damage", -1.0, false),
                "/lasereyes config damage 0.0",
            ),
        ] {
            assert_eq!(change.command().as_deref(), Some(expected));
        }
        for (name, _) in COLORS {
            assert_eq!(
                Change::Color(name.into()).command().unwrap(),
                format!("/lasereyes config color {name}")
            );
        }
    }
    #[test]
    fn custom_hex_is_one_safe_command_argument() {
        assert_eq!(
            hex_command(" #12aBcF\n").as_deref(),
            Some("/lasereyes config color #12aBcF")
        );
        for bad in ["12345", "gg0000", "ff0000 extra", "ff0000\n/stop"] {
            assert!(hex_command(bad).is_none());
        }
    }
}
