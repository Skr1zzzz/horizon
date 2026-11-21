//! `HudLayersClientHost::hud_layer_bounds`: where a HUD layer drew in the last frame, for a
//! client mod laying its own widget out clear of vanilla's (#10125).
use super::*;

impl Game<'_> {
    /// Where HUD layer `layer` drew in the last frame, `[x, y, w, h]` in GUI pixels: the box of
    /// the layer's own drawing, between its `native_gui_layer_pre` and `native_gui_layer_post`,
    /// as `gui_layer` and `draw_mod_gui_layers` measured it. None when it drew nothing there.
    pub(in crate::game) fn hud_layer_bounds(&self, layer: &str) -> Option<[f32; 4]> {
        self.hud_layers_drawn.get(layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::{ClientHost, Flow, GuiLayerPostEvent, GuiLayerPreEvent, Hooks, ModEntry};
    use mod_platform::testing::with_mods;
    use std::sync::Mutex;

    /// What `Around` read of the hotbar's box after each chat layer.
    static READ: Mutex<Vec<Option<[f32; 4]>>> = Mutex::new(Vec::new());

    /// Draws in every layer's hooks, hides the effects, and reads the hotbar's box after the chat.
    struct Around;
    #[mod_api::__subscriptions]
    impl Hooks for Around {
        fn gui_layer_pre(&self, _: &dyn ClientHost, event: &mut GuiLayerPreEvent<'_>) -> Flow {
            if event.layer == "minecraft:effects" {
                return Flow::Cancel;
            }
            event.draw.fill([0.0, 0.0, 4.0, 4.0], 0xFF00_FF00);
            Flow::Continue
        }
        fn gui_layer_post(&self, host: &dyn ClientHost, event: &mut GuiLayerPostEvent<'_>) {
            event.draw.fill([400.0, 230.0, 4.0, 4.0], 0xFF00_FF00);
            if event.layer == "minecraft:chat" {
                let hotbar = host.hud_layer_bounds("minecraft:hotbar");
                READ.lock().unwrap().push(hotbar);
            }
        }
    }

    /// A layer's box is its own drawing, not what mods drew in its hooks; a mod reads it in the
    /// next frame; a layer a mod hid, or one that drew nothing, reads none.
    #[test]
    #[ignore = "daily"]
    fn a_layers_box_is_its_own_drawing_read_in_the_next_frame() {
        let mods: mod_api::Mods = vec![ModEntry::new("around", Box::new(Around))].into();
        with_mods(mods, || {
            crate::game::native_entity_refs::tests::with_client_game(|game| {
                let hotbar = [122.0, 218.0, 182.0, 22.0];
                for _ in 0..2 {
                    game.hud_layers_drawn.next_frame();
                    let mut batch = HudBatch::new([854, 480], game.settings.gui_scale_inputs());
                    game.gui_layer("minecraft:hotbar", &mut batch, |g, batch| {
                        let [x, y, w, h] = hotbar;
                        batch.fill(&g.gui.atlas, x, y, w, h, u32::MAX);
                    });
                    game.gui_layer("minecraft:effects", &mut batch, |g, batch| {
                        batch.fill(&g.gui.atlas, 400.0, 1.0, 24.0, 24.0, u32::MAX);
                    });
                    game.gui_layer("minecraft:chat", &mut batch, |_, _| {});
                }
                assert_eq!(
                    *READ.lock().unwrap(),
                    [None, Some(hotbar)],
                    "the first frame reads none drawn before it"
                );
                game.hud_layers_drawn.next_frame();
                assert_eq!(game.hud_layer_bounds("minecraft:hotbar"), Some(hotbar));
                assert_eq!(game.hud_layer_bounds("minecraft:effects"), None, "hidden");
                assert_eq!(
                    game.hud_layer_bounds("minecraft:chat"),
                    None,
                    "drew nothing"
                );
            });
        });
    }
}
