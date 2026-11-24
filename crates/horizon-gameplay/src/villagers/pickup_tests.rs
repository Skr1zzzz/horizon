//! A villager's pickup whose thrower's advancement reward runs a function in
//! the middle of it (`AdvancementRewards.grant`, from
//! `LivingEntity.onItemPickup`), on a level holding only what
//! [`villager_pickups`] reads.
use super::*;
use crate::{item_areas::tests::item_at, mob_level::MobParts};
use horizon_client::{
    blocks::BlockFacts,
    brewing::Brewing,
    fast_hash::FxHashSet,
    join_seq::JoinList,
    merchant::Trades,
    mobs::{
        Anim, Brain, MobType, MobWorld, Part, Seen,
        equip::{Facts, Slot},
    },
    stations::StationData,
    world::World,
};
use horizon_world::{level::LevelEvent, registry::BlockStateId};
use std::{collections::HashSet, sync::Arc};

/// Villagers 1, 2 and 3 in a row, of which 2 and 3 tick and may pick up
/// loot, and items 11 to 14: an egg far off, bread a player threw at
/// villager 2's feet, a carrot far off the other way, and bread nobody threw
/// at villager 3's feet.
struct Level {
    mobs: JoinList<Mob>,
    dropped: JoinList<ItemEntity>,
    ticked: FxHashSet<u32>,
    items: Items,
    stations: StationData,
    facts: BlockFacts,
    world: World,
    /// How many pickups ran the thrower's reward.
    rewards: u32,
    /// Each pickup shown (`LivingEntity.take`): the collector, then the item.
    taken: Vec<(u32, u32)>,
}

impl Level {
    fn new() -> Self {
        let kind = villager_type();
        let mob = |id: u32, x: f64| Mob::new(id, Arc::clone(&kind), Vec3::new(x, 64.0, 0.5), 0.0);
        let mut picker = mob(2, 0.5);
        picker.extra.pick_up_loot = true;
        let mut last = mob(3, -10.5);
        last.extra.pick_up_loot = true;
        let item = |id: u32, x: f64, stack: ItemStack| {
            let mut e = item_at(x, 64.0, 0.5);
            e.id = id;
            e.stack = stack;
            e
        };
        let mut bread = item(12, 0.5, ItemStack::new("minecraft:bread", 3));
        bread.thrower = Some([0, 0, 0, 1]);
        let professions = Arc::new(villager::ResolvedProfessions::vanilla());
        let stations = StationData {
            enchantments: Default::default(),
            recipes: Default::default(),
            brewing: Brewing {
                recipes: Vec::new(),
                reagents: Default::default(),
                inputs: Default::default(),
            },
            trades: Trades {
                professions: Arc::clone(&professions),
                trades: Default::default(),
                tags: Default::default(),
                sets: Default::default(),
                potion_tags: Default::default(),
            },
            professions,
            banners: Default::default(),
            recipe_index: Default::default(),
        };
        Self {
            mobs: [mob(1, 10.5), picker, last].into_iter().collect(),
            dropped: [
                item(11, 20.5, ItemStack::new("minecraft:egg", 1)),
                bread,
                item(13, -20.5, ItemStack::new("minecraft:carrot", 1)),
                item(14, -10.5, ItemStack::new("minecraft:bread", 2)),
            ]
            .into_iter()
            .collect(),
            ticked: [2, 3].into_iter().collect(),
            items: Items::empty(),
            stations,
            facts: BlockFacts {
                states: Vec::new(),
                air: BlockStateId(0),
                woods: Default::default(),
                sign_texts: Default::default(),
            },
            world: World::new(BlockStateId(0), 0),
            rewards: 0,
            taken: Vec::new(),
        }
    }
}

/// A villager's type as mob code reads it: 0.6 by 1.95, with no model.
fn villager_type() -> Arc<MobType> {
    Arc::new(MobType {
        shadow: None,
        id: "minecraft:villager".into(),
        species: Species::Villager,
        width: 0.6,
        height: 1.95,
        eye_height: 1.62,
        category: "misc".into(),
        max_health: 20.0,
        speed: 0.5,
        flying_speed: 0.4,
        attack_damage: 0.0,
        attack_knockback: 0.0,
        follow_range: 48.0,
        step_height: 0.6,
        armor: 0.0,
        armor_toughness: 0.0,
        knockback_resistance: 0.0,
        jump_strength: 0.42,
        attributes: Arc::default(),
        item_stack_limits: Arc::default(),
        attribute_bounds: Default::default(),
        gravity: 0.08,
        safe_fall_distance: 3.0,
        fall_damage_multiplier: 1.0,
        water_efficiency: 0.0,
        water_slowdown: 0.8,
        air_speed: 0.02,
        baby_scale: 0.5,
        head_rot_speed: 10.0,
        max_head_x_rot: 40.0,
        max_head_y_rot: 75.0,
        ambient_interval: 80,
        flags: Default::default(),
        model: Arc::new(Part {
            name: "root".into(),
            pose: [0.0; 9],
            polygons: Vec::new(),
            children: Vec::new(),
        }),
        layers: Default::default(),
        texture: String::new(),
        brain: Brain::Passive,
        anim: Anim::Humanoid { zombie_arms: false },
        sounds: ["entity.villager.ambient"; 3],
        sound_overrides: Default::default(),
        food: Arc::default(),
        tempt: Arc::default(),
        passenger_attachments: Vec::new(),
    })
}

impl MobLevel for Level {
    type Equip = ();
    fn parts(&mut self) -> MobParts<'_> {
        MobParts {
            mobs: &mut self.mobs,
            dropped: &mut self.dropped,
            next_entity: 20,
            ticked: &self.ticked,
            items: &self.items,
            stations: &self.stations,
            facts: &self.facts,
            world: &mut self.world,
        }
    }
    fn mobs(&self) -> &[Mob] {
        &self.mobs
    }
    fn dropped(&self) -> &[ItemEntity] {
        &self.dropped
    }
    fn mob_world(&self) -> MobWorld<'_> {
        unreachable!()
    }
    fn dimension(&self) -> &'static str {
        unreachable!()
    }
    fn host_uuid(&self) -> Option<[i32; 4]> {
        unreachable!()
    }
    fn player_by_uuid(&self, _: [i32; 4]) -> Option<(Vec3, [f64; 3])> {
        unreachable!()
    }
    fn rule(&self, _: &str) -> bool {
        unreachable!()
    }
    fn mob_griefing(&self) -> bool {
        true
    }
    fn entity_grief(&self, _: u32, rule: bool) -> bool {
        rule
    }
    fn grief_hooked(&self) -> bool {
        false
    }
    fn spawn_item_own(&mut self, _: ItemStack, _: [f64; 3], _: u32) -> Option<u32> {
        unreachable!()
    }
    fn spawn_item(&mut self, _: ItemStack, _: [f64; 3], _: [f64; 3], _: u32) -> Option<u32> {
        unreachable!()
    }
    fn item_entity_mut(&mut self, _: u32) -> Option<&mut ItemEntity> {
        unreachable!()
    }
    fn mob_uuid(&mut self, _: u32) -> [i32; 4] {
        unreachable!()
    }
    fn play_sound(&mut self, _: &str, _: [f64; 3], _: f32, _: f32) {
        unreachable!()
    }
    fn play_mob_sound(&mut self, _: usize, _: &str, _: usize, _: f32, _: f32, _: bool) {
        unreachable!()
    }
    fn trigger_block_for(&mut self, _: [i32; 4], _: &str, _: [i32; 3], _: &ItemStack) {
        unreachable!()
    }
    /// The thrower's advancement is done, and its reward function sends the
    /// first mob to another dimension and kills the first item.
    fn on_item_pickup(&mut self, _: usize, item: &ItemEntity) {
        if item.thrower.is_none() {
            return;
        }
        self.rewards += 1;
        self.mobs.remove(0);
        self.dropped.remove(0);
    }
    fn take(&mut self, collector: u32, item: ItemEntity) {
        self.taken.push((collector, item.id));
    }
    fn discard_item(&mut self, index: usize) -> ItemEntity {
        self.dropped.remove(index)
    }
    fn on_equip_item(&mut self, _: u32, _: [f64; 3], _: bool) {
        unreachable!()
    }
    fn equip_prepare(&self, _: usize, _: &ItemStack) -> Self::Equip {
        unreachable!()
    }
    fn equip_allows(&self, _: &Self::Equip, _: usize, _: &str, _: bool) -> bool {
        unreachable!()
    }
    fn equip_can_apply(&self, _: &Self::Equip, _: u32) -> bool {
        unreachable!()
    }
    fn equip_item_if_possible(&mut self, _: usize, _: &ItemStack) -> Option<u32> {
        unreachable!()
    }
    fn stack_facts(&self, _: &Mob, _: &ItemStack, _: Slot) -> Facts {
        unreachable!()
    }
}

impl VillagerLevel for Level {
    fn items(&self) -> &Items {
        &self.items
    }
    fn stations(&self) -> &StationData {
        &self.stations
    }
    fn villager_picks_up(&self) -> Option<HashSet<String>> {
        Some(HashSet::from(["minecraft:bread".to_owned()]))
    }
    fn mob_type(&self, _: &str) -> Option<Arc<MobType>> {
        unreachable!()
    }
    fn mob_world_seen<'s>(&'s self, _: &'s [Seen]) -> MobWorld<'s> {
        unreachable!()
    }
    fn set_next_entity(&mut self, _: u32) {
        unreachable!()
    }
    fn mob_joined(&mut self, _: usize, _: bool) {
        unreachable!()
    }
    fn biome_facts(&self, _: i32, _: i32, _: i32) -> spawning::BiomeFacts {
        unreachable!()
    }
    fn spawn_mob_raw(&mut self, _: &str, _: [f64; 3], _: bool) -> Option<u32> {
        unreachable!()
    }
    fn mob_event_particles(&mut self, _: u32, _: i8) {
        unreachable!()
    }
    fn bonemeal_grow(&mut self, _: BlockPos, _: &mut ItemStack) -> (bool, Option<bool>) {
        unreachable!()
    }
    fn level_event(&mut self, _: LevelEvent) {
        unreachable!()
    }
    fn composter_level(&self, _: [i32; 3]) -> Option<i32> {
        unreachable!()
    }
    fn compost_items(&mut self, _: usize, _: [i32; 3]) {
        unreachable!()
    }
    fn hurt_by_player_uuid(&self, _: u32) -> Option<[i32; 4]> {
        unreachable!()
    }
    fn player_uuid_of(&self, _: Option<u32>) -> Option<[i32; 4]> {
        unreachable!()
    }
    fn lan_acting(&self) -> Option<u32> {
        unreachable!()
    }
    fn wake_villager(&mut self, _: u32) {
        unreachable!()
    }
    fn villager_index(&self, id: u32) -> Option<usize> {
        super::villager_index(self, id)
    }
    fn item_max_stack(&self, _: &str) -> u32 {
        64
    }
    fn villager_make_bread(&mut self, _: usize) {
        unreachable!()
    }
    fn villager_throw(&mut self, _: Vec3, _: f64, _: Vec3, _: ItemStack) {
        unreachable!()
    }
    fn villager_golem_check(&mut self, _: u32, _: usize) {
        unreachable!()
    }
    fn try_spawn_iron_golem(&mut self, _: [i32; 3]) -> bool {
        unreachable!()
    }
    fn villager_reputation_of(&self, _: u32, _: [i32; 4]) -> i32 {
        unreachable!()
    }
}

/// `Mob.aiStep` loots from a list taken once, passing over the items removed
/// since, and `InventoryCarrier.pickUpItem` goes on with its own villager and
/// stack after `onItemPickup`, whose advancement reward may run a function.
/// One that sends a mob to another dimension and kills an item, both listed
/// before the villager's own, leaves it the bread it reached for and every
/// other mob and stack as they were, and the villager after it, now one
/// place earlier in the list, still loots its own bread.
#[test]
fn a_pickup_reward_that_removes_a_mob_and_an_item_moves_only_the_bread() {
    let mut level = Level::new();
    villager_pickups(&mut level);
    assert_eq!(level.rewards, 1);
    let bread = |id: u32| {
        let m = level.mobs.iter().find(|m| m.id == id).unwrap();
        villager::ext(m).map_or(0, |x| x.count_item("minecraft:bread"))
    };
    assert_eq!((bread(2), bread(3)), (3, 2), "each takes its bread");
    let left: Vec<u32> = level.dropped.iter().map(|e| e.id).collect();
    assert_eq!(left, [13], "the carrot stays");
    assert_eq!(level.taken, [(2, 12), (3, 14)]);
}
