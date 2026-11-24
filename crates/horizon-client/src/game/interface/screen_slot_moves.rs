//! `ClientHost::set_screen_slot` and `ClientHost::screen_slot`: a client mod moves or
//! hides a slot of the open container screen, as a Java screen that lays its slots out
//! again sets `Slot.x`/`Slot.y` or deactivates a slot. Only the screen changes: the menu,
//! its slot indices and the server keep every slot. A mod's positions are `Slot.x`/`Slot.y`,
//! from the window's `leftPos`/`topPos`; `slot_position` measures from `inventory_origin`,
//! which the merchant's and the beacon's windows start left of (`left_pos_shift`).
use super::{Game, Screen};
use horizon_client::fast_hash::FxHashMap;
use mod_api::ScreenSlotRefusal;
use std::cell::{Cell, RefCell};

/// A slot's place from `leftPos`/`topPos` (`Slot.x`, `Slot.y`), or None for a hidden one.
type Place = Option<[f32; 2]>;

/// The largest coordinate a mod may give, either way (`BadPosition` past it). Java's
/// `Slot.x` is an int; this keeps every place's pixel arithmetic far inside `i32`.
const MAX_COORD: f32 = 1_000_000.0;

/// What client mods did to the open container screen's slots, by game slot id. A move
/// waits in `pending` until the frame's build takes it (`commit`), so one frame draws and
/// hit-tests one layout. The moves are one screen's (`owner`) and go with it, when it
/// closes or another menu replaces it.
#[derive(Default)]
pub(super) struct ScreenSlotMoves {
    /// The screen the moves are for: its client menu's generation.
    owner: Cell<Option<u64>>,
    /// Each moved slot's place, as the last build took it.
    committed: RefCell<FxHashMap<usize, Place>>,
    /// The moves made since, for the next build.
    pending: RefCell<FxHashMap<usize, Place>>,
    /// How many committed slots are hidden, so `slot_count` lists nothing.
    hidden: Cell<usize>,
    /// Counts the builds that took a change: a slot input of the container layout.
    revision: Cell<u64>,
}

impl ScreenSlotMoves {
    /// Nothing moved as of the last build: the frame's slots are the screen's own.
    fn is_empty(&self) -> bool {
        self.committed.borrow().is_empty()
    }
    /// Slot `slot`'s place on the screen of `owner` as of the last build: Some once a mod
    /// moved or hid it, None where the screen lays it out.
    fn committed(&self, owner: u64, slot: usize) -> Option<Place> {
        if self.owner.get() != Some(owner) {
            return None;
        }
        self.committed.borrow().get(&slot).copied()
    }
    /// How many slots of the screen of `owner` are hidden as of the last build.
    fn hidden(&self, owner: u64) -> usize {
        if self.owner.get() == Some(owner) {
            self.hidden.get()
        } else {
            0
        }
    }
    /// Slot `slot`'s last move on the screen of `owner`, pending or committed.
    fn latest_move(&self, owner: u64, slot: usize) -> Option<Place> {
        if self.owner.get() != Some(owner) {
            return None;
        }
        let pending = self.pending.borrow().get(&slot).copied();
        pending.or_else(|| self.committed.borrow().get(&slot).copied())
    }
    /// Where slot `slot` of the screen of `owner` is from the next build: its last move,
    /// else `shown`, where the last build put it (None when it isn't shown).
    fn latest(&self, owner: u64, slot: usize, shown: Place) -> Place {
        self.latest_move(owner, slot).unwrap_or(shown)
    }
    /// Moves slot `slot` of the screen of `owner`, which the last build put at `shown`
    /// (None when it isn't shown), to `place` from the next build, or hides it with None.
    /// A slot the screen hid when it opened (not shown and never moved) stays hidden. Each
    /// refusal changes nothing, and the slot's current place again changes nothing either.
    fn set(
        &self,
        owner: u64,
        slot: usize,
        shown: Place,
        place: Place,
    ) -> Result<(), ScreenSlotRefusal> {
        let moved = self.latest_move(owner, slot);
        if moved.is_none() && shown.is_none() {
            return match place {
                Some(_) => Err(ScreenSlotRefusal::HiddenAtOpen),
                None => Ok(()),
            };
        }
        if place.is_some_and(|[x, y]| !(x.abs() <= MAX_COORD && y.abs() <= MAX_COORD)) {
            return Err(ScreenSlotRefusal::BadPosition);
        }
        if moved.unwrap_or(shown) != place {
            self.adopt(Some(owner));
            self.pending.borrow_mut().insert(slot, place);
        }
        Ok(())
    }
    /// Makes the moves the screen of `owner`'s, dropping another screen's.
    fn adopt(&self, owner: Option<u64>) {
        if self.owner.replace(owner) == owner {
            return;
        }
        self.pending.borrow_mut().clear();
        let mut committed = self.committed.borrow_mut();
        if !committed.is_empty() {
            committed.clear();
            self.hidden.set(0);
            self.revision.set(self.revision.get().wrapping_add(1));
        }
    }
    /// The build's moment: the pending moves of the screen of `owner` (None with no
    /// container screen open) become what the frame draws and hit-tests, and another
    /// screen's moves go.
    fn commit(&self, owner: Option<u64>) {
        self.adopt(owner);
        let mut pending = self.pending.borrow_mut();
        if pending.is_empty() {
            return;
        }
        let mut committed = self.committed.borrow_mut();
        committed.extend(pending.drain());
        self.hidden
            .set(committed.values().filter(|place| place.is_none()).count());
        self.revision.set(self.revision.get().wrapping_add(1));
    }
}

impl Game<'_> {
    /// The open container screen whose slots a mod may move, as its moves are kept: its
    /// client menu's generation, new for every menu a server opens or closes. None with
    /// another screen or none, the creative inventory, a native mod's own screen for the
    /// menu (it lays its slots out itself), or no logged-in client.
    fn screen_slot_owner(&self) -> Option<u64> {
        if self.screen != Screen::Inventory || self.native_screen_shown() {
            return None;
        }
        let remote = self.remote.as_ref().filter(|remote| remote.logged_in)?;
        Some(remote.menu.generation)
    }
    /// Game slot `slot`'s place as a mod left it on the open container screen, measured
    /// as `slot_position` measures: Some(Some) moved, Some(None) hidden, None where the
    /// screen's own layout puts it. As of the last build: a move waits for the next.
    pub(super) fn moved_slot(&self, slot: usize) -> Option<Option<(f32, f32)>> {
        let moves = &self.screen_state.slot_moves;
        if moves.is_empty() {
            return None;
        }
        let place = moves.committed(self.screen_slot_owner()?, slot)?;
        let shift = self.left_pos_shift();
        Some(place.map(|[x, y]| (x + shift, y)))
    }
    /// Whether a mod hid game slot `slot` of the open container screen.
    pub(super) fn screen_slot_hidden(&self, slot: usize) -> bool {
        matches!(self.moved_slot(slot), Some(None))
    }
    /// How many of the open container screen's slots mods hid.
    pub(super) fn hidden_screen_slots(&self) -> usize {
        let moves = &self.screen_state.slot_moves;
        if moves.is_empty() {
            return 0;
        }
        self.screen_slot_owner()
            .map_or(0, |owner| moves.hidden(owner))
    }
    /// Counts the builds that took a mod's slot change: a slot input of the container
    /// layout, so a move rebuilds it.
    pub(super) fn screen_slot_revision(&self) -> u64 {
        self.screen_state.slot_moves.revision.get()
    }
    /// The build's moment for the mods' slot moves (`ScreenSlotMoves::commit`).
    pub(super) fn commit_screen_slots(&self) {
        self.screen_state
            .slot_moves
            .commit(self.screen_slot_owner());
    }
    /// The open container screen and the game slot of its menu index `slot`
    /// (`inventory_screen_slot`), or why a mod can't address it.
    fn screen_slot_target(&self, slot: usize) -> Result<(u64, usize), ScreenSlotRefusal> {
        if self.is_logical_server() {
            return Err(ScreenSlotRefusal::Unavailable);
        }
        let owner = self
            .screen_slot_owner()
            .ok_or(ScreenSlotRefusal::NoScreen)?;
        let menu = self
            .remote
            .as_ref()
            .and_then(|remote| remote.menu.as_ref())
            .map_or("inventory", |open| open.1.as_str());
        let slot = i32::try_from(slot)
            .ok()
            .and_then(|slot| Self::inventory_screen_slot(menu, slot))
            .ok_or(ScreenSlotRefusal::NoSuchSlot)?;
        Ok((owner, slot))
    }
    /// Where the last build put game slot `slot` of the open container screen, from its
    /// `leftPos`/`topPos`; None when it doesn't show it.
    fn shown_screen_slot(&self, slot: usize) -> Option<[f32; 2]> {
        self.slots().contains(&slot).then(|| {
            let (x, y) = self.slot_position(slot);
            [x - self.left_pos_shift(), y]
        })
    }
    /// `ClientHost::screen_slot`: where the open container screen puts the slot of menu
    /// index `slot` from the next frame, from its `leftPos`/`topPos`; None when hidden.
    pub(in crate::game) fn screen_slot(&self, slot: usize) -> Option<[f32; 2]> {
        let (owner, slot) = self.screen_slot_target(slot).ok()?;
        self.screen_state
            .slot_moves
            .latest(owner, slot, self.shown_screen_slot(slot))
    }
    /// `ClientHost::set_screen_slot`: moves the slot of menu index `slot` of the open
    /// container screen to `pos` (from its `leftPos`/`topPos`), or hides it with None,
    /// from the next frame (`ScreenSlotMoves::set`). Nothing is sent.
    pub(in crate::game) fn set_screen_slot(
        &self,
        slot: usize,
        pos: Option<[f32; 2]>,
    ) -> Result<(), ScreenSlotRefusal> {
        let (owner, slot) = self.screen_slot_target(slot)?;
        self.screen_state
            .slot_moves
            .set(owner, slot, self.shown_screen_slot(slot), pos)
    }
}

#[cfg(test)]
mod tests {
    use super::ScreenSlotMoves;
    use mod_api::ScreenSlotRefusal::*;

    /// The screen the moves are for: a menu generation.
    const SCREEN: u64 = 3;

    /// Where the screen's own layout shows its slots: 0 at (8, 142), 1 at (26, 142),
    /// and 2 hidden when it opened.
    fn shown(slot: usize) -> Option<[f32; 2]> {
        [Some([8.0, 142.0]), Some([26.0, 142.0]), None][slot]
    }

    #[test]
    fn a_move_reads_back_at_once_and_takes_effect_at_the_next_build() {
        let moves = ScreenSlotMoves::default();
        moves.commit(Some(SCREEN));
        assert_eq!(moves.set(SCREEN, 0, shown(0), Some([180.5, -20.0])), Ok(()));
        assert_eq!(moves.latest(SCREEN, 0, shown(0)), Some([180.5, -20.0]));
        assert_eq!(moves.committed(SCREEN, 0), None, "pending until the build");
        moves.commit(Some(SCREEN));
        assert_eq!(moves.committed(SCREEN, 0), Some(Some([180.5, -20.0])));
        assert_eq!(moves.committed(SCREEN, 1), None);
        assert_eq!(moves.revision.get(), 1);
        // A slot's current place again, moved or its own, costs no build.
        assert_eq!(moves.set(SCREEN, 0, shown(0), Some([180.5, -20.0])), Ok(()));
        assert_eq!(moves.set(SCREEN, 1, shown(1), Some([26.0, 142.0])), Ok(()));
        moves.commit(Some(SCREEN));
        assert_eq!(moves.revision.get(), 1);
        assert_eq!(moves.committed(SCREEN, 1), None);
    }

    #[test]
    fn a_hidden_slot_is_counted_and_shows_again_where_a_mod_puts_it() {
        let moves = ScreenSlotMoves::default();
        moves.commit(Some(SCREEN));
        assert_eq!(moves.set(SCREEN, 1, shown(1), None), Ok(()));
        moves.commit(Some(SCREEN));
        assert_eq!(moves.committed(SCREEN, 1), Some(None));
        assert_eq!(moves.hidden(SCREEN), 1);
        // The build no longer shows it, but the screen laid it out when it opened.
        assert_eq!(moves.latest(SCREEN, 1, None), None);
        assert_eq!(moves.set(SCREEN, 1, None, Some([40.0, 60.0])), Ok(()));
        moves.commit(Some(SCREEN));
        assert_eq!(moves.committed(SCREEN, 1), Some(Some([40.0, 60.0])));
        assert_eq!(moves.hidden(SCREEN), 0);
    }

    #[test]
    fn each_refusal_changes_nothing() {
        let moves = ScreenSlotMoves::default();
        moves.commit(Some(SCREEN));
        for (slot, place, refusal) in [
            (2, Some([0.0, 0.0]), HiddenAtOpen),
            (2, Some([f32::NAN, 0.0]), HiddenAtOpen),
            (0, Some([f32::NAN, 0.0]), BadPosition),
            (0, Some([0.0, f32::INFINITY]), BadPosition),
            (1, Some([f32::NEG_INFINITY, 0.0]), BadPosition),
            (0, Some([3.0e9, 0.0]), BadPosition),
            (1, Some([0.0, -1.0e7]), BadPosition),
        ] {
            assert_eq!(
                moves.set(SCREEN, slot, shown(slot), place),
                Err(refusal),
                "{slot} {place:?}"
            );
        }
        // Hiding a slot the screen hid when it opened is already so.
        assert_eq!(moves.set(SCREEN, 2, shown(2), None), Ok(()));
        moves.commit(Some(SCREEN));
        assert!(moves.is_empty());
        assert_eq!(moves.revision.get(), 0);
        assert_eq!(moves.latest(SCREEN, 0, shown(0)), shown(0));
    }

    #[test]
    fn the_moves_go_when_their_screen_closes_or_another_menu_replaces_it() {
        let moves = ScreenSlotMoves::default();
        moves.commit(Some(SCREEN));
        assert_eq!(moves.set(SCREEN, 0, shown(0), None), Ok(()));
        moves.commit(Some(SCREEN));
        // Closed: the moves go and the layout is rebuilt.
        moves.commit(None);
        assert!(moves.is_empty());
        assert_eq!(moves.revision.get(), 2);
        // Opened again with the same menu (the player's inventory keeps its
        // generation), it starts from its own layout.
        moves.commit(Some(SCREEN));
        assert_eq!(moves.hidden(SCREEN), 0);
        assert_eq!(moves.latest(SCREEN, 0, shown(0)), shown(0));
        // Another menu takes the screen before the build: the pending move goes too.
        assert_eq!(moves.set(SCREEN, 1, shown(1), Some([1.0, 2.0])), Ok(()));
        moves.commit(Some(SCREEN + 1));
        assert!(moves.is_empty());
        assert_eq!(moves.latest(SCREEN, 1, shown(1)), shown(1));
        assert_eq!(moves.latest(SCREEN + 1, 1, shown(1)), shown(1));
    }

    /// The game's own slot layout, hit test and screen lifetime: a mod moves the player's
    /// first hotbar slot and hides the first inventory slot, and both go when the screen
    /// closes.
    #[test]
    #[ignore = "daily"]
    fn a_mod_moves_and_hides_inventory_slots_until_the_screen_closes() {
        use crate::game::Screen;
        use crate::game::native_entity_refs::tests::with_client_game;
        use crate::game::remote::tests::connect_menu_test;
        use horizon_client::hud::HudBatch;
        use horizon_protocol::packets::play::{self, Clientbound as C};

        with_client_game(|game| {
            let _server = connect_menu_test(game);
            game.handle_packet(C::Login(play::Login {
                player_id: 7,
                hardcore: false,
                levels: vec!["minecraft:overworld".into()],
                max_players: 20,
                chunk_radius: 8,
                simulation_distance: 8,
                reduced_debug_info: false,
                show_death_screen: true,
                limited_crafting: false,
                spawn: play::SpawnInfo {
                    dimension_type: 0,
                    dimension: "minecraft:overworld".into(),
                    seed: 1,
                    game_type: 0,
                    previous_game_type: None,
                    debug: false,
                    flat: false,
                    last_death: None,
                    portal_cooldown: 0,
                    sea_level: 63,
                },
                online_mode: false,
                enforces_secure_chat: false,
            }))
            .unwrap();
            assert_eq!(game.set_screen_slot(36, Some([0.0, 0.0])), Err(NoScreen));
            assert_eq!(game.screen_slot(36), None);
            game.set_screen(Screen::Inventory);
            let batch = HudBatch::new([800, 600], game.settings.gui_scale_inputs());
            let (ox, oy) = game.inventory_origin(&batch);
            // InventoryMenu: menu slot 36, the first hotbar slot, is at (8, 142);
            // menu slot 9, the first inventory slot, at (8, 84).
            assert_eq!(game.screen_slot(36), Some([8.0, 142.0]));
            assert_eq!(game.slot_at(&batch, ox + 16.0, oy + 150.0), Some(0));

            assert_eq!(game.set_screen_slot(36, Some([180.5, -20.0])), Ok(()));
            assert_eq!(game.screen_slot(36), Some([180.5, -20.0]));
            assert_eq!(
                game.slot_at(&batch, ox + 16.0, oy + 150.0),
                Some(0),
                "drawn and hit where it was until the next frame's build"
            );
            game.build_screen();
            assert_eq!(game.slot_at(&batch, ox + 16.0, oy + 150.0), None);
            assert_eq!(game.slot_at(&batch, ox + 188.5, oy - 12.0), Some(0));

            assert_eq!(game.set_screen_slot(9, None), Ok(()));
            game.build_screen();
            assert_eq!(game.screen_slot(9), None);
            assert!(!game.slots().contains(&9));
            assert_eq!(game.slot_count(), game.slots().len());
            assert_eq!(game.slot_at(&batch, ox + 16.0, oy + 92.0), None);

            assert_eq!(game.set_screen_slot(46, Some([0.0, 0.0])), Err(NoSuchSlot));
            assert_eq!(
                game.set_screen_slot(36, Some([f32::NAN, 0.0])),
                Err(BadPosition)
            );
            assert_eq!(game.screen_slot(36), Some([180.5, -20.0]));

            game.set_screen(Screen::Playing);
            assert_eq!(game.set_screen_slot(36, Some([0.0, 0.0])), Err(NoScreen));
            game.set_screen(Screen::Inventory);
            assert_eq!(game.screen_slot(36), Some([8.0, 142.0]));
            assert_eq!(game.screen_slot(9), Some([8.0, 84.0]));
            assert_eq!(game.slot_at(&batch, ox + 16.0, oy + 150.0), Some(0));
            assert_eq!(game.slot_at(&batch, ox + 16.0, oy + 92.0), Some(9));
        });
    }
}
