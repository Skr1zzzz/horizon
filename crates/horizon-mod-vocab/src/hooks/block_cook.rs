//! The `block_cook` hook: declarations the mod API generator reads (see `hooks/mod.rs`).
// catalog-order: 0172

pub trait Hooks {
    /// A block finished cooking `item` into `result` at `pos`. `result` is
    /// what was made, after `prepare_block_craft`; an item it left no result
    /// stays on the fire and is not told.
    #[hook(
        side = "server",
        contract = "listen",
        neoforge = "none (NeoForge has no such event)"
    )]
    fn block_cook();
}
