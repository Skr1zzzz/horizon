//! The `prepare_block_craft` hook: declarations the mod API generator reads (see `hooks/mod.rs`).

pub trait Hooks {
    /// The block at `pos` is about to make `result`, where vanilla computes
    /// it: a furnace, blast furnace or smoker finishing a cook
    /// (`AbstractFurnaceBlockEntity.burn`), a lit campfire finishing an item
    /// (`CampfireBlockEntity.cookTick`), or a crafter crafting on a redstone
    /// pulse (`CrafterBlock.dispenseFrom`). `kind` is that block's id
    /// (`minecraft:furnace`, `minecraft:blast_furnace`, `minecraft:smoker`,
    /// `minecraft:campfire`, `minecraft:soul_campfire`, `minecraft:crafter`)
    /// and `recipe` the recipe's full id; a campfire item no recipe takes any
    /// more has `None`, and the item itself as its result. `result` arrives as
    /// the recipe made it. Put another stack there to make that instead, or
    /// set it to `None` to make nothing, which each block treats as vanilla
    /// treats an empty result: a furnace keeps its input, counts no recipe and
    /// starts the cook again; a campfire keeps the item on the fire and asks
    /// again next tick; a crafter plays its fail event and uses nothing. A
    /// furnace makes a result its output slot can't take (another item, other
    /// components, or past the stack size) no more than an empty one. Every
    /// listening mod is called in turn and sees what the mods before it left.
    /// `furnace_smelt`, `block_cook` and `crafter_craft` then tell what was
    /// made, and are not told when nothing was. Server-side.
    #[notes = r#"## Block crafting results

- **`prepare_block_craft(pos, kind, recipe, result)`**: the one point to change what a furnace, blast furnace, smoker, campfire or crafter makes, before it makes it, as Paper's `FurnaceSmeltEvent`, `BlockCookEvent` and `CrafterCraftEvent` do; `furnace_smelt`, `block_cook` and `crafter_craft` only listen afterwards. Put another stack in `result` to make it instead, or `None` to make nothing: the furnace keeps its input, the campfire keeps the item on the fire (and asks again next tick), the crafter fails as for an empty result. A furnace result its output can't take is not made either. With no listening mod the blocks build no arguments and make the recipe's result as before."#]
    #[hook(
        side = "server",
        contract = "listen",
        neoforge = "none (NeoForge has no such event)"
    )]
    fn prepare_block_craft();
}
