//! `prepare_block_craft` through the blocks that ask it: a furnace, blast furnace and smoker
//! finishing a cook (`tick_furnace`), a campfire finishing an item (`tick_campfire`) and a
//! crafter crafting (`crafter_dispense`). A mod's stack is made instead, `None` makes nothing,
//! and with no listener every block makes the recipe's result as before.
use super::{BlockEntity, Game};
use crate::game::native_entity_refs::tests::with_game;
use horizon_client::{
    chunk::Chunk,
    containers::{Campfire, Crafter, Furnace, FurnaceKind},
    items::ItemStack,
};
use horizon_core::position::BlockPos;
use mod_api::{
    BlockCookEvent, CrafterCraftEvent, FurnaceSmeltEvent, Hooks, ItemKey, ModEntry, Mods,
    PrepareBlockCraftEvent, ServerHost,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Where each block stands.
const POS: (i32, i32, i32) = (8, 64, 8);

/// Each furnace with the input it cooks, its result and the recipe that makes it.
const FURNACES: [(&str, FurnaceKind, &str, &str, &str); 3] = [
    (
        "minecraft:furnace",
        FurnaceKind::Furnace,
        "minecraft:raw_iron",
        "minecraft:iron_ingot",
        "minecraft:iron_ingot_from_smelting_raw_iron",
    ),
    (
        "minecraft:blast_furnace",
        FurnaceKind::BlastFurnace,
        "minecraft:raw_iron",
        "minecraft:iron_ingot",
        "minecraft:iron_ingot_from_blasting_raw_iron",
    ),
    (
        "minecraft:smoker",
        FurnaceKind::Smoker,
        "minecraft:beef",
        "minecraft:cooked_beef",
        "minecraft:cooked_beef_from_smoking",
    ),
];

/// What the probe does with the result it is asked about.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Answer {
    /// Leave it.
    #[default]
    Keep,
    /// Three times as many.
    Triple,
    /// Take it away.
    Cancel,
}

/// One question the probe was asked: the position, block and recipe, and the result's item
/// and count as it arrived.
type Asked = ([i32; 3], String, Option<String>, Option<(ItemKey, u32)>);

#[derive(Default)]
struct Heard {
    answer: Mutex<Answer>,
    asked: Mutex<Vec<Asked>>,
    /// What `furnace_smelt`, `block_cook` and `crafter_craft` told.
    made: Mutex<Vec<String>>,
}
impl Heard {
    fn reset(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
        self.asked.lock().unwrap().clear();
        self.made.lock().unwrap().clear();
    }
    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
    fn made(&self) -> Vec<String> {
        self.made.lock().unwrap().clone()
    }
    fn told(&self, line: String) {
        self.made.lock().unwrap().push(line);
    }
}

/// Answers `prepare_block_craft` as `Heard::answer` says, and hears what was made.
struct Probe(Arc<Heard>);
#[mod_api::__subscriptions]
impl Hooks for Probe {
    fn prepare_block_craft(&self, _: &mut dyn ServerHost, event: &mut PrepareBlockCraftEvent<'_>) {
        self.0.asked.lock().unwrap().push((
            event.pos,
            event.kind.to_owned(),
            event.recipe.map(str::to_owned),
            event.result.as_ref().map(|s| (s.item(), s.count())),
        ));
        match *self.0.answer.lock().unwrap() {
            Answer::Keep => {}
            Answer::Triple => {
                if let Some(stack) = event.result.as_mut() {
                    stack.set_count(stack.count() * 3).unwrap();
                }
            }
            Answer::Cancel => *event.result = None,
        }
    }
    fn furnace_smelt(&self, _: &mut dyn ServerHost, event: &mut FurnaceSmeltEvent<'_>) {
        self.0
            .told(format!("furnace_smelt {} {}", event.item, event.result));
    }
    fn block_cook(&self, _: &mut dyn ServerHost, event: &mut BlockCookEvent<'_>) {
        self.0
            .told(format!("block_cook {} {}", event.item, event.result));
    }
    fn crafter_craft(&self, _: &mut dyn ServerHost, _: &mut CrafterCraftEvent<'_>) {
        self.0.told("crafter_craft".to_owned());
    }
}

/// Hears what was made, and does not listen to `prepare_block_craft`.
struct Watcher(Arc<Heard>);
#[mod_api::__subscriptions]
impl Hooks for Watcher {
    fn furnace_smelt(&self, _: &mut dyn ServerHost, event: &mut FurnaceSmeltEvent<'_>) {
        self.0
            .told(format!("furnace_smelt {} {}", event.item, event.result));
    }
    fn block_cook(&self, _: &mut dyn ServerHost, event: &mut BlockCookEvent<'_>) {
        self.0
            .told(format!("block_cook {} {}", event.item, event.result));
    }
    fn crafter_craft(&self, _: &mut dyn ServerHost, _: &mut CrafterCraftEvent<'_>) {
        self.0.told("crafter_craft".to_owned());
    }
}

/// A server game with a resident column at [`POS`], and the probe installed (`answering`) or
/// the watcher.
fn with_mod(answering: bool, test: impl FnOnce(&mut Game<'_>, &Heard)) {
    let heard = Arc::new(Heard::default());
    let entry = if answering {
        ModEntry::new("block_craft_probe", Box::new(Probe(heard.clone())))
    } else {
        ModEntry::new("block_craft_watcher", Box::new(Watcher(heard.clone())))
    };
    let mods: Mods = vec![entry].into();
    crate::game::api_values::install_parts(&mods).unwrap();
    mod_platform::testing::with_mods(mods, || {
        with_game(|game| {
            game.world
                .chunks
                .insert((0, 0), Arc::new(Chunk::empty(0, 0)));
            test(game, &heard);
        });
    });
}

/// `block` (its default state) at [`POS`] with `entity`, and nothing on the ground.
fn place(game: &mut Game<'_>, block: &str, entity: BlockEntity) {
    let state = game.pack.assets.registry.default_state(block).unwrap();
    game.world.set(POS.0, POS.1, POS.2, state).unwrap();
    game.world.block_entities.insert(POS, entity);
    game.dropped.clear();
}

/// The item entities on the ground, counted by item.
fn dropped(game: &Game<'_>) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for entity in game.dropped.iter() {
        *counts.entry(entity.stack.item.to_string()).or_insert(0) += entity.stack.count;
    }
    counts
}

/// One tick of a lit `block` whose cook of two `input`s finishes in it; the furnace after.
fn smelt(game: &mut Game<'_>, block: &str, kind: FurnaceKind, input: &str) -> Furnace {
    let furnace = Furnace {
        kind,
        items: [
            Some(ItemStack::new(input, 2)),
            Some(ItemStack::new("minecraft:coal", 1)),
            None,
        ],
        lit_remaining: 5,
        lit_total: 200,
        cooking_timer: 199,
        cooking_total: 200,
        speed: 1.0,
        recipes_used: HashMap::new(),
    };
    place(game, block, BlockEntity::Furnace(furnace));
    game.tick_furnace(POS).unwrap();
    let Some(BlockEntity::Furnace(furnace)) = game.world.block_entities.get(&POS) else {
        panic!("no {block}");
    };
    furnace.clone()
}

/// `ticks` ticks of a lit campfire whose one beef's time is up in the first; the campfire
/// after and what it dropped.
fn cook(game: &mut Game<'_>, ticks: usize) -> (Campfire, HashMap<String, u32>) {
    let mut campfire = Campfire::default();
    campfire.items[0] = Some(ItemStack::new("minecraft:beef", 1));
    campfire.progress[0] = 599;
    campfire.total[0] = 600;
    place(
        game,
        "minecraft:campfire",
        BlockEntity::Campfire(Box::new(campfire)),
    );
    for _ in 0..ticks {
        game.tick_campfire(POS).unwrap();
    }
    let Some(BlockEntity::Campfire(campfire)) = game.world.block_entities.get(&POS) else {
        panic!("no campfire");
    };
    ((**campfire).clone(), dropped(game))
}

/// A crafter holding two oak logs crafts once (facing north, out into the air); the crafter
/// after and what it dispensed.
fn craft(game: &mut Game<'_>) -> (Crafter, HashMap<String, u32>) {
    let mut crafter = Crafter::default();
    crafter.items[0] = Some(ItemStack::new("minecraft:oak_log", 2));
    place(game, "minecraft:crafter", BlockEntity::Crafter(crafter));
    let state = game.world.block(POS.0, POS.1, POS.2);
    game.crafter_dispense(BlockPos::new(POS.0, POS.1, POS.2), state)
        .unwrap();
    let Some(BlockEntity::Crafter(crafter)) = game.world.block_entities.get(&POS) else {
        panic!("no crafter");
    };
    (crafter.clone(), dropped(game))
}

/// The furnace made `count` of `output` from one of its two inputs and counted `recipe`.
fn assert_smelted(furnace: &Furnace, output: &str, count: u32, recipe: &str) {
    assert_eq!(furnace.items[0].as_ref().map(|s| s.count), Some(1));
    assert_eq!(
        furnace.items[2].as_ref().map(|s| (&*s.item, s.count)),
        Some((output, count))
    );
    assert_eq!(furnace.recipes_used.get(recipe), Some(&1));
}

/// Each block asks once, right before it makes its result, with its own id, the recipe and
/// the recipe's result. A changed stack is made instead and told by the listen hook after
/// it; `None` makes nothing, each block as vanilla does with an empty result: a furnace keeps
/// its input and starts the cook again, a campfire keeps the item on the fire and asks again
/// next tick, a crafter uses nothing.
#[test]
#[ignore = "daily"]
fn each_block_makes_the_result_prepare_block_craft_leaves() {
    with_mod(true, |game, heard| {
        assert!(mod_api::__private::listened(
            mod_api::HookId::PrepareBlockCraft
        ));
        let key = |id: &str| mod_api::__private::item_key(id).unwrap();
        let at = [POS.0, POS.1, POS.2];
        for answer in [Answer::Keep, Answer::Triple, Answer::Cancel] {
            let count = |n: u32| if answer == Answer::Triple { 3 * n } else { n };
            for (block, kind, input, output, recipe) in FURNACES {
                heard.reset(answer);
                let furnace = smelt(game, block, kind, input);
                let asked = (
                    at,
                    block.to_owned(),
                    Some(recipe.to_owned()),
                    Some((key(output), 1)),
                );
                assert_eq!(heard.asked(), [asked], "{block} {answer:?}");
                if answer == Answer::Cancel {
                    assert_eq!(furnace.items[0].as_ref().map(|s| s.count), Some(2));
                    assert!(furnace.items[2].is_none(), "{block}");
                    assert!(furnace.recipes_used.is_empty(), "{block}");
                    assert_eq!(furnace.cooking_timer, 0, "{block}");
                    assert!(heard.made().is_empty(), "{block}");
                } else {
                    assert_smelted(&furnace, output, count(1), recipe);
                    assert_eq!(heard.made(), [format!("furnace_smelt {input} {output}")]);
                }
            }

            heard.reset(answer);
            let ticks = if answer == Answer::Cancel { 2 } else { 1 };
            let (campfire, on_ground) = cook(game, ticks);
            let asked = (
                at,
                "minecraft:campfire".to_owned(),
                Some("minecraft:cooked_beef_from_campfire_cooking".to_owned()),
                Some((key("minecraft:cooked_beef"), 1)),
            );
            if answer == Answer::Cancel {
                assert_eq!(heard.asked(), [asked.clone(), asked]);
                assert_eq!(
                    campfire.items[0].as_ref().map(|s| (&*s.item, s.count)),
                    Some(("minecraft:beef", 1))
                );
                assert!(on_ground.is_empty());
                assert!(heard.made().is_empty());
            } else {
                assert_eq!(heard.asked(), [asked]);
                assert!(campfire.items.iter().all(Option::is_none));
                assert_eq!(
                    on_ground,
                    HashMap::from([("minecraft:cooked_beef".to_owned(), count(1))])
                );
                assert_eq!(
                    heard.made(),
                    ["block_cook minecraft:beef minecraft:cooked_beef"]
                );
            }

            heard.reset(answer);
            let (crafter, on_ground) = craft(game);
            assert_eq!(
                heard.asked(),
                [(
                    at,
                    "minecraft:crafter".to_owned(),
                    Some("minecraft:oak_planks".to_owned()),
                    Some((key("minecraft:oak_planks"), 4)),
                )]
            );
            if answer == Answer::Cancel {
                assert_eq!(crafter.items[0].as_ref().map(|s| s.count), Some(2));
                assert_eq!(crafter.crafting_ticks, 0);
                assert!(on_ground.is_empty());
                assert!(heard.made().is_empty());
            } else {
                assert_eq!(crafter.items[0].as_ref().map(|s| s.count), Some(1));
                assert_eq!(
                    on_ground,
                    HashMap::from([("minecraft:oak_planks".to_owned(), count(4))])
                );
                assert_eq!(heard.made(), ["crafter_craft"]);
            }
        }
    });
}

/// With no mod listening to `prepare_block_craft`, every block makes the recipe's result, and
/// the listen hooks after it tell it as before.
#[test]
#[ignore = "daily"]
fn without_a_listener_each_block_makes_the_recipes_result() {
    with_mod(false, |game, heard| {
        assert!(!mod_api::__private::listened(
            mod_api::HookId::PrepareBlockCraft
        ));
        for (block, kind, input, output, recipe) in FURNACES {
            heard.reset(Answer::Keep);
            let furnace = smelt(game, block, kind, input);
            assert_smelted(&furnace, output, 1, recipe);
            assert_eq!(heard.made(), [format!("furnace_smelt {input} {output}")]);
        }
        heard.reset(Answer::Keep);
        let (campfire, on_ground) = cook(game, 1);
        assert!(campfire.items.iter().all(Option::is_none));
        assert_eq!(
            on_ground,
            HashMap::from([("minecraft:cooked_beef".to_owned(), 1)])
        );
        assert_eq!(
            heard.made(),
            ["block_cook minecraft:beef minecraft:cooked_beef"]
        );
        heard.reset(Answer::Keep);
        let (crafter, on_ground) = craft(game);
        assert_eq!(crafter.items[0].as_ref().map(|s| s.count), Some(1));
        assert_eq!(
            on_ground,
            HashMap::from([("minecraft:oak_planks".to_owned(), 4)])
        );
        assert_eq!(heard.made(), ["crafter_craft"]);
        assert!(heard.asked().is_empty());
    });
}
