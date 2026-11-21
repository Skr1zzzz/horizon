//! A player's cooldowns for native server mods, through a `PlayerRef`: item
//! cooldowns (`Player.getCooldowns()`, vanilla's `ItemCooldowns`), the
//! experience orb pickup delay (`Player.takeXpDelay`) and the warden warning
//! cooldown (`WardenSpawnTracker.cooldownTicks`). A cooldown group
//! is an item id unless the item's `use_cooldown` component names another
//! (`minecraft:ender_pearl`, a mod's `mymod:wands`...);
//! `item_cooldown_group` gives a stack's. Reads return None for a stale
//! handle or a player who isn't online here; inside another player's turn
//! they still read a resting guest. Writes return false for those, for a
//! dead player, and for a player whose turn another player's turn is
//! running inside.
use super::Game;
use horizon_client::cooldowns::Cooldowns;
use horizon_client::items::ItemStack;
use mod_api::{ItemCooldown, PlayerRef};
use std::sync::Arc;

impl Game<'_> {
    /// The cooldowns of `player`, where they are kept now: a guest's own
    /// outside their turn, the game's while theirs is swapped in (as
    /// `player` finds the player).
    fn native_cooldowns_of(&self, player: PlayerRef) -> Option<&Cooldowns> {
        match self.player_handle_read(player)? {
            super::native_server::PlayerRead::Acting { .. } => Some(&self.cooldowns),
            super::native_server::PlayerRead::Guest(g) => Some(
                self.lan
                    .as_ref()?
                    .play
                    .get(&g.conn)
                    .map_or(&EMPTY, |p| &p.cooldowns),
            ),
        }
    }

    /// `ItemCooldowns.getCooldownGroup(stack)`: the group `stack`'s
    /// cooldown is filed under, the group its `use_cooldown` component names
    /// (its own, else its item's default), else its item id.
    pub(super) fn item_cooldown_group(&self, stack: &ItemStack) -> String {
        let default = self
            .items()
            .get(&stack.item)
            .and_then(|info| info.use_cooldown.as_ref());
        horizon_client::cooldowns::group(stack, default).to_string()
    }
    /// Every group the player waits on and its ticks left, ordered by
    /// group.
    pub(super) fn player_item_cooldowns(&self, player: PlayerRef) -> Option<Vec<ItemCooldown>> {
        let mut out: Vec<ItemCooldown> = self
            .native_cooldowns_of(player)?
            .remaining()
            .filter(|(_, left)| *left > 0)
            .map(|(group, left)| ItemCooldown {
                group: group.to_string(),
                ticks_left: left,
            })
            .collect();
        out.sort_by(|a, b| a.group.cmp(&b.group));
        Some(out)
    }
    /// `ItemCooldowns.isOnCooldown`: whether `group` still has time to run.
    pub(super) fn player_has_item_cooldown(&self, player: PlayerRef, group: &str) -> Option<bool> {
        Some(self.native_cooldowns_of(player)?.is_on_cooldown(group))
    }
    /// `ItemCooldowns.getCooldownPercent(group, 0)`: what is left of the
    /// group's cooldown, from 1 (just started) to 0 (none).
    pub(super) fn player_item_cooldown_percent(
        &self,
        player: PlayerRef,
        group: &str,
    ) -> Option<f32> {
        Some(self.native_cooldowns_of(player)?.percent(group, 0.0))
    }
    /// `ItemCooldowns.addCooldown(group, ticks)`: starts (or restarts) the
    /// group's cooldown, and the player's client draws it on every item of the
    /// group (`ServerItemCooldowns.onCooldownStarted`).
    pub(super) fn player_set_item_cooldown(
        &mut self,
        player: PlayerRef,
        group: &str,
        ticks: i32,
    ) -> bool {
        let group: Arc<str> = group.into();
        self.native_as_player_handle(player, |game| game.add_cooldown(group, ticks))
    }
    /// `ItemCooldowns.removeCooldown(group)`: ends the group's cooldown now,
    /// on the client too (`ServerItemCooldowns.onCooldownEnded`).
    pub(super) fn player_remove_item_cooldown(&mut self, player: PlayerRef, group: &str) -> bool {
        let group: Arc<str> = group.into();
        self.native_as_player_handle(player, |game| {
            game.cooldowns.remove(&group);
            if game.lan_acting.is_some() {
                game.guest_cooldowns.push((group, 0));
            }
        })
    }
    /// `Player.takeXpDelay`: the ticks before the player can take the next
    /// experience orb.
    pub(super) fn player_exp_cooldown(&self, player: PlayerRef) -> Option<u32> {
        match self.player_handle_read(player)? {
            super::native_server::PlayerRead::Acting { .. } => Some(self.xp_delay),
            super::native_server::PlayerRead::Guest(g) => Some(
                self.lan
                    .as_ref()?
                    .play
                    .get(&g.conn)
                    .map_or(0, |p| p.xp_delay),
            ),
        }
    }
    /// Sets `Player.takeXpDelay`: no orb is taken for `ticks` ticks. The
    /// change is not reported to `player_exp_cooldown_change`.
    pub(super) fn player_set_exp_cooldown(&mut self, player: PlayerRef, ticks: u32) -> bool {
        self.native_as_player_handle(player, |game| game.xp_delay = ticks)
    }
    /// Sets the player's `WardenSpawnTracker.cooldownTicks`: sculk shriekers
    /// they set off, or set off near them, don't warn for `ticks` ticks. Each
    /// player has a tracker of their own, in place for their turn.
    pub(super) fn player_set_warden_warning_cooldown(
        &mut self,
        player: PlayerRef,
        ticks: u32,
    ) -> bool {
        self.native_as_player_handle(player, |game| {
            // Loaded first, or the next tick's load would overwrite the game's own.
            game.load_warden_tracker();
            game.sculk.tracker.cooldown_ticks = i32::try_from(ticks).unwrap_or(i32::MAX);
        })
    }
}

/// What a guest with no play state yet waits on: nothing.
static EMPTY: std::sync::LazyLock<Cooldowns> = std::sync::LazyLock::new(Cooldowns::default);

#[cfg(test)]
mod tests {
    use super::super::native_entity_refs::tests::with_game;
    use horizon_client::items::ItemStack;
    use horizon_core::nbt::Tag;

    #[test]
    #[ignore = "daily"]
    fn a_stack_names_its_cooldown_group() {
        with_game(|game| {
            let pearl = ItemStack::new("minecraft:ender_pearl", 1);
            assert_eq!(game.item_cooldown_group(&pearl), "minecraft:ender_pearl");
            let mut wand = ItemStack::new("minecraft:stick", 1);
            wand.data_mut().set_other(
                "minecraft:use_cooldown",
                Some(Tag::compound([
                    ("seconds", Tag::Float(1.0f32.to_bits())),
                    ("cooldown_group", Tag::String("mymod:wands".into())),
                ])),
            );
            assert_eq!(game.item_cooldown_group(&wand), "mymod:wands");
            let unknown = ItemStack::new("mymod:not_an_item", 1);
            assert_eq!(game.item_cooldown_group(&unknown), "mymod:not_an_item");
        });
    }

    /// `player`'s handle from another world epoch, which names no one.
    fn stale(player: mod_api::PlayerRef) -> mod_api::PlayerRef {
        let entity = player.entity();
        mod_api::__private::game::player_ref(
            mod_api::EntityRef::new(
                mod_api::WorldEpoch::new(entity.epoch().get() + 1),
                entity.id(),
            ),
            player.uuid(),
        )
    }

    #[test]
    #[ignore = "daily"]
    fn exp_cooldowns_reach_their_player() {
        super::super::native_guest_turn_tests::with_guests(|game| {
            let (a, b) = (game.player(12).unwrap(), game.player(13).unwrap());
            assert!(game.player_set_exp_cooldown(a, 5));
            assert_eq!(game.player_exp_cooldown(a), Some(5));
            assert_eq!(game.player_exp_cooldown(b), Some(0));
            // In A's turn, A's own delay is in place and B's is still B's.
            game.as_guest(12, |game| {
                assert_eq!(game.player_exp_cooldown(a), Some(5));
                assert_eq!(game.player_exp_cooldown(b), Some(0));
                assert!(game.player_set_exp_cooldown(a, 9));
                assert!(game.player_set_exp_cooldown(b, 3));
                // A's state is displaced while B's turn runs inside A's.
                game.as_guest(13, |game| {
                    assert_eq!(game.player_exp_cooldown(a), None);
                    assert!(!game.player_set_exp_cooldown(a, 1));
                })
                .unwrap();
            })
            .unwrap();
            assert_eq!(game.player_exp_cooldown(a), Some(9));
            assert_eq!(game.player_exp_cooldown(b), Some(3));

            assert_eq!(game.player_exp_cooldown(stale(a)), None);
            assert!(!game.player_set_exp_cooldown(stale(a), 1));
            assert_eq!(game.player_exp_cooldown(a), Some(9));
        });
    }

    #[test]
    #[ignore = "daily"]
    fn the_warden_warning_cooldown_is_its_trackers_owners() {
        use horizon_client::sculk::WardenSpawnTracker;
        let level = |warning_level: i32| WardenSpawnTracker {
            warning_level,
            ..Default::default()
        };
        let tracker = |game: &super::super::Game<'_>, conn: u32| {
            let guests = &game.lan.as_ref().unwrap().hub.guests;
            guests[guests.connection_index(conn).unwrap()].warden
        };
        super::super::native_guest_turn_tests::with_guests(|game| {
            // Each player's own saved tracker, as `playerdata::apply` loads it.
            game.lan.as_mut().unwrap().hub.guests.update_all(|guest| {
                guest.warden = level(if guest.uuid == 12 { 2 } else { 4 });
            });
            let (a, b) = (game.player(12).unwrap(), game.player(13).unwrap());

            // A write reaches that player's tracker alone, out of their turn
            // or inside another player's.
            assert!(game.player_set_warden_warning_cooldown(b, 40));
            assert_eq!(tracker(game, 13).cooldown_ticks, 40);
            assert_eq!(tracker(game, 12), level(2));
            game.as_guest(12, |game| {
                assert!(game.player_set_warden_warning_cooldown(a, 5));
                assert_eq!(game.sculk.tracker.cooldown_ticks, 5);
                assert!(game.player_set_warden_warning_cooldown(b, 30));
                assert_eq!(game.sculk.tracker.cooldown_ticks, 5);
                // A's state is displaced while B's turn runs inside A's.
                game.as_guest(13, |game| {
                    assert!(!game.player_set_warden_warning_cooldown(a, 1));
                })
                .unwrap();
            })
            .unwrap();
            assert!(!game.player_set_warden_warning_cooldown(stale(a), 40));
            let (a_now, b_now) = (tracker(game, 12), tracker(game, 13));
            assert_eq!((a_now.warning_level, a_now.cooldown_ticks), (2, 5));
            assert_eq!((b_now.warning_level, b_now.cooldown_ticks), (4, 30));
            // The game's own tracker, a server's no one's, is left as it was.
            assert!(!game.sculk.tracker_loaded);
            assert_eq!(game.sculk.tracker, WardenSpawnTracker::default());
        });
    }
}
