# Replacing what a furnace, campfire or crafter makes

`Hooks::prepare_block_craft` asks your mod for the stack a block is about to make, at the
point where vanilla computes it, before anything is used up:

| Block | When it asks | `recipe` |
| --- | --- | --- |
| Furnace, blast furnace, smoker | A cook finishes (`AbstractFurnaceBlockEntity.burn`) | The cooking recipe's id |
| Campfire, soul campfire | An item's cooking time is up (`CampfireBlockEntity.cookTick`) | The campfire recipe's id, or `None` when no recipe takes the item any more (its result is then the item itself) |
| Crafter | A redstone pulse crafts (`CrafterBlock.dispenseFrom`) | The crafting recipe's id |

`event.pos` is the block's position and `event.kind` its id (`minecraft:furnace`,
`minecraft:smoker`, `minecraft:soul_campfire`...). `event.result` arrives as the recipe made
it. Put another stack there and the block makes that instead; set it to `None` and the
block makes nothing. Every listening mod is called in turn and sees what the mods before it
left.

```rust,ignore
use mod_api::{Hooks, PrepareBlockCraftEvent, ServerHost};

fn prepare_block_craft(&self, _: &mut dyn ServerHost, event: &mut PrepareBlockCraftEvent<'_>) {
    match event.recipe {
        // Raw iron smelts into two ingots.
        Some("minecraft:iron_ingot_from_smelting_raw_iron") => {
            if let Some(stack) = event.result.as_mut() {
                let _ = stack.set_count(stack.count() * 2);
            }
        }
        // Smokers refuse beef.
        Some("minecraft:cooked_beef_from_smoking") => *event.result = None,
        _ => {}
    }
}
```

## When the result is taken away

`None` follows each block's own vanilla path for a recipe whose result is empty:

| Block | What happens with `None` |
| --- | --- |
| Furnace, blast furnace, smoker | Nothing goes to the output, the input stays, the recipe is not counted for experience, and the cook starts again from zero. Fuel burnt this tick stays burnt. |
| Campfire | The item stays on the fire with its time up, as vanilla keeps an item whose result it can't make, and the hook is asked about it again next tick until a mod lets a result through. To drop the item uncooked, answer with the item itself. |
| Crafter | The crafter plays its fail event (level event 1050) and uses nothing. |

A furnace makes a replaced result only when its output slot can take it, as vanilla's
`canBurn` checks the recipe's: the slot is empty, or holds the same item with the same
components and room for the whole stack. Otherwise nothing is made, as with `None`.

## After the result is made

`furnace_smelt`, `block_cook` and `crafter_craft` still only listen. They tell what the
block made, a replaced stack included, and are not told when no result was made. With no
mod listening to `prepare_block_craft`, the blocks build none of its arguments and make the
recipe's result as before.
