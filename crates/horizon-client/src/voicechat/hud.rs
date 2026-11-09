//! Presentation priority keeps connection failures visible even while muted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Disconnected,
    Disabled,
    Muted,
    Talking,
}
pub fn state(connected: bool, muted: bool, disabled: bool, talking: bool) -> Option<Icon> {
    if !connected {
        Some(Icon::Disconnected)
    } else if disabled {
        Some(Icon::Disabled)
    } else if muted {
        Some(Icon::Muted)
    } else if talking {
        Some(Icon::Talking)
    } else {
        None
    }
}
use crate::hud::{HudAtlas, HudBatch, rgba};

pub fn color(icon: Icon) -> u32 {
    match icon {
        Icon::Disconnected => rgba(255, 170, 0, 255),
        Icon::Disabled => rgba(119, 119, 119, 255),
        Icon::Muted => rgba(255, 85, 85, 255),
        Icon::Talking => rgba(85, 255, 85, 255),
    }
}

const MIC: [&str; 16] = [
    "....####....",
    "...######...",
    "...######...",
    "...######...",
    "...######...",
    "...######...",
    ".#.######.#.",
    ".#..####..#.",
    ".#........#.",
    "..#......#..",
    "...######...",
    ".....##.....",
    ".....##.....",
    ".....##.....",
    "...######...",
    "..########..",
];

/// Original pixel art shared by the world HUD and the recording button.
/// Paint shadows before foreground so adjoining pixels do not obscure each other.
pub fn draw_microphone(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    position: [f32; 2],
    scale: f32,
    tint: u32,
    icon: Icon,
) {
    let [x, y] = position;
    let mut pixels = Vec::new();
    for (row, line) in MIC.iter().enumerate() {
        for (col, pixel) in line.bytes().enumerate() {
            if pixel == b'#' {
                pixels.push((col as f32, row as f32));
            }
        }
    }
    if icon == Icon::Disconnected {
        for row in [2, 3, 4, 5, 6, 7, 10, 11] {
            for col in 14..16 {
                pixels.push((col as f32, row as f32));
            }
        }
    }
    let paint = |batch: &mut HudBatch, pixels: &[(f32, f32)]| {
        for &(px, py) in pixels {
            batch.fill(
                atlas,
                x + (px - 1.0) * scale,
                y + (py - 1.0) * scale,
                3.0 * scale,
                3.0 * scale,
                rgba(0, 0, 0, 160),
            );
        }
        for &(px, py) in pixels {
            batch.fill(atlas, x + px * scale, y + py * scale, scale, scale, tint);
        }
    };
    paint(batch, &pixels);
    if matches!(icon, Icon::Muted | Icon::Disabled) {
        // A separate pass gives the slash its own outline over the capsule.
        let slash: Vec<_> = (0..12)
            .flat_map(|i| [(i as f32, (i + 2) as f32), (i as f32, (i + 3) as f32)])
            .collect();
        paint(batch, &slash);
    }
}

pub fn draw(batch: &mut HudBatch, atlas: &HudAtlas, position: [f32; 2], icon: Icon) {
    draw_microphone(batch, atlas, position, 1.0, color(icon), icon);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icon_colors_use_hud_channel_order() {
        assert_eq!(color(Icon::Muted), 0xff5555ff);
        assert_eq!(color(Icon::Disconnected), 0xff00aaff);
        assert_eq!(color(Icon::Talking), 0xff55ff55);
        assert_eq!(color(Icon::Disabled), 0xff777777);
    }

    #[test]
    fn microphone_bitmap_is_twelve_by_sixteen() {
        assert_eq!(MIC.len(), 16);
        for row in MIC {
            assert_eq!(row.len(), 12);
            assert!(row.bytes().all(|p| matches!(p, b'.' | b'#')));
        }
    }

    #[test]
    fn hud_state_priority() {
        for muted in [false, true] {
            for disabled in [false, true] {
                for talking in [false, true] {
                    assert_eq!(
                        state(false, muted, disabled, talking),
                        Some(Icon::Disconnected)
                    );
                    let expected = if disabled {
                        Some(Icon::Disabled)
                    } else if muted {
                        Some(Icon::Muted)
                    } else if talking {
                        Some(Icon::Talking)
                    } else {
                        None
                    };
                    assert_eq!(state(true, muted, disabled, talking), expected);
                }
            }
        }
    }
}
