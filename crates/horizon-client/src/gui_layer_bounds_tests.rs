//! [`LayerBounds`]: a layer's box is what it drew, read in the frame after. The subtitle layer's
//! box is the one vanilla's `SubtitleOverlay` fills.
use super::LayerBounds;
use crate::hud::{HudAtlas, HudBatch};
use crate::hud_layout::{paint, width};
use crate::hud_overlays::{SubtitleFrame, SubtitleOverlay};
use crate::text::Text;

/// A HUD atlas without game assets: its font draws every character as the missing glyph.
fn atlas() -> HudAtlas {
    HudAtlas::build(&crate::resource_source::ResourceSource::directory(
        "/nonexistent/horizon-test-assets".into(),
    ))
    .unwrap()
}

/// `SubtitleOverlay.extractRenderState` centres row `i` at `width - halfWidth - 2` across and
/// `height - 35 - i * 10` down and fills `-halfWidth - 1, -5` to `halfWidth + 1, 5` round it,
/// `halfWidth` being half the widest subtitle with "< " and "> ". The arrows and the text, their
/// shadows too, stay inside the fills, so the layer's box is the box of every row's fill.
#[test]
fn the_subtitle_layers_box_is_the_one_vanilla_fills() {
    let atlas = atlas();
    let font = &atlas.font;
    // Ahead of a listener at the origin facing south (no arrow), then behind it with "<", then
    // behind it with ">".
    let sounds = [
        ("subtitles.block.chest.open", [0.0, 0.0, 2.0]),
        ("subtitles.entity.zombie.ambient", [3.0, 0.0, -1.0]),
        ("subtitles.entity.player.hurt", [-3.0, 0.0, -1.0]),
    ];
    for rows in 1..=sounds.len() {
        let mut overlay = SubtitleOverlay::default();
        for &(key, at) in &sounds[..rows] {
            overlay.on_play(Some(key), at, 16.0, false, 1000);
        }
        let mut batch = HudBatch::scratch([854, 480]);
        let (w, h) = (batch.width as i32, batch.height as i32);
        let frame = SubtitleFrame {
            show: true,
            position: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            multiplier: 1.0,
            background: 0xCC00_0000,
            width: w,
            height: h,
        };
        paint(&mut batch, &atlas, &overlay.extract(&frame, font, 1000));
        let mut bounds = LayerBounds::default();
        bounds.record("minecraft:subtitle_overlay", &batch.vertices);
        bounds.next_frame();

        let widest = (sounds[..rows].iter())
            .map(|(key, _)| width(font, &Text::translate(key, &[])))
            .max()
            .unwrap();
        let glyphs = |s: &str| font.width(s) as i32;
        let half = (widest + glyphs("<") + glyphs(" ") + glyphs(">") + glyphs(" ")) / 2;
        let n = rows as i32;
        let vanilla = [w - 2 * half - 3, h - 30 - 10 * n, 2 * half + 2, 10 * n].map(|v| v as f32);
        assert_eq!(
            bounds.get("minecraft:subtitle_overlay"),
            Some(vanilla),
            "{rows} rows"
        );
    }
}

/// What a layer drew in a frame is read through the next frame only, both draws of a layer drawn
/// twice count, and a layer that drew nothing, or nothing with an area, reads none.
#[test]
fn a_frames_boxes_are_read_in_the_next_frame_only() {
    let atlas = atlas();
    let mut batch = HudBatch::scratch([854, 480]);
    batch.fill(&atlas, 10.0, 200.0, 182.0, 22.0, 0xFF00_0000);
    let hotbar = batch.vertices.len();
    batch.fill(&atlas, 2.0, 150.0, 100.0, 9.0, 0x8000_0000);
    batch.fill(&atlas, 2.0, 170.0, 120.0, 9.0, 0x8000_0000);
    let chat = batch.vertices.len();
    // A line with no height.
    batch.fill(&atlas, 300.0, 4.0, 40.0, 0.0, 0xFF00_0000);
    let mut bounds = LayerBounds::default();
    bounds.record("minecraft:hotbar", &batch.vertices[..hotbar]);
    bounds.record("minecraft:chat", &batch.vertices[hotbar..hotbar + 6]);
    bounds.record("minecraft:effects", &[]);
    bounds.record("minecraft:chat", &batch.vertices[hotbar + 6..chat]);
    bounds.record("minecraft:title", &batch.vertices[chat..]);
    assert_eq!(
        bounds.get("minecraft:hotbar"),
        None,
        "not before the frame ends"
    );

    bounds.next_frame();
    assert_eq!(
        bounds.get("minecraft:hotbar"),
        Some([10.0, 200.0, 182.0, 22.0])
    );
    assert_eq!(
        bounds.get("minecraft:chat"),
        Some([2.0, 150.0, 120.0, 29.0])
    );
    assert_eq!(bounds.get("minecraft:effects"), None);
    assert_eq!(bounds.get("minecraft:title"), None);
    assert_eq!(bounds.get("minecraft:no_such_layer"), None);

    // The next frame draws only the hotbar, elsewhere.
    bounds.record("minecraft:hotbar", &batch.vertices[hotbar..hotbar + 6]);
    bounds.next_frame();
    assert_eq!(
        bounds.get("minecraft:hotbar"),
        Some([2.0, 150.0, 100.0, 9.0])
    );
    assert_eq!(bounds.get("minecraft:chat"), None, "it drew nothing");
    bounds.next_frame();
    assert_eq!(bounds.get("minecraft:hotbar"), None);
}

/// A frame reads the last HUD pass's boxes from its start, and a frame that draws no HUD (a
/// loading frame) keeps them.
#[test]
fn a_frame_without_a_hud_keeps_the_last_hud_passs_boxes() {
    let atlas = atlas();
    let mut batch = HudBatch::scratch([854, 480]);
    batch.fill(&atlas, 10.0, 200.0, 182.0, 22.0, 0xFF00_0000);
    let mut bounds = LayerBounds::default();
    bounds.frame_start();
    bounds.begin_hud();
    bounds.record("minecraft:hotbar", &batch.vertices);
    assert_eq!(bounds.get("minecraft:hotbar"), None, "the frame it draws");
    let hotbar = Some([10.0, 200.0, 182.0, 22.0]);
    bounds.frame_start();
    assert_eq!(bounds.get("minecraft:hotbar"), hotbar);
    bounds.frame_start();
    assert_eq!(bounds.get("minecraft:hotbar"), hotbar, "no HUD drew");
    bounds.begin_hud();
    bounds.frame_start();
    assert_eq!(bounds.get("minecraft:hotbar"), None, "a HUD that drew no hotbar");
}
