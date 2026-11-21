//! Where the HUD's layers drew, read by client mods.
#![allow(unused_variables)]

/// The client host's reads of the HUD's layers, implemented by the game and reached through [`crate::ClientHost`].
#[horizon_mod_api_macros::host_area]
pub trait HudLayersClientHost {
    /// Side: client.
    /// Where HUD layer `layer` drew in the last frame, for a widget that keeps clear of it: the
    /// smallest `[x, y, w, h]` rectangle in GUI pixels (as `DrawContext::fill` takes one) holding
    /// everything the layer drew, its pose and clip applied. `layer` is an id `gui_layer_pre`
    /// lists (`minecraft:subtitle_overlay`, `minecraft:boss_overlay`, `minecraft:hotbar`,
    /// `minecraft:effects`, `minecraft:chat`...) or a layer a mod registered
    /// (`register_gui_layers`). Only the layer's own drawing counts, not what mods draw in its
    /// `gui_layer_pre` and `gui_layer_post`. The box is what the layer drew, so a sprite's
    /// transparent edges and text shadows are in it: the subtitles' box is their background,
    /// the boss bars' their bars and names, the hotbar's its slots and selection, with the
    /// off-hand slot and the attack indicator while they show.
    /// None when the layer drew nothing that frame: a mod hid it, F1 or a canceled
    /// `render_gui_pre` hid the HUD, it had nothing to show (no boss bar, subtitle or effect, no
    /// recent chat line), or no layer has that id. A frame reads the frame before it, so a box
    /// lags its layer by one frame. Sends nothing and costs a lookup.
    #[access(ClientSetup)]
    fn hud_layer_bounds(&self, layer: &str) -> Option<[f32; 4]>;
}
