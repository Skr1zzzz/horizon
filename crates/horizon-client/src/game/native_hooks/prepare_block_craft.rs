use super::*;

/// The hook points. Each does nothing in Horizon.
impl Game<'_> {
    /// The block at `pos` is about to make `result`, where vanilla computes
    /// it: a furnace, blast furnace or smoker finishing a cook
    /// (`AbstractFurnaceBlockEntity.burn`), a lit campfire finishing an item
    /// (`CampfireBlockEntity.cookTick`), a crafter crafting
    /// (`CrafterBlock.dispenseFrom`). `kind` is the block's id and `recipe`
    /// the recipe's (None for a campfire item no recipe takes, whose result
    /// is the item itself). A changed `result` is made instead; None makes
    /// nothing. Server-side.
    pub(in crate::game) fn native_prepare_block_craft(
        &mut self,
        pos: [i32; 3],
        kind: &str,
        recipe: Option<&str>,
        result: &mut Option<ItemStack>,
    ) {
        self.fire_prepare_block_craft(pos, kind, recipe, result);
    }
}
