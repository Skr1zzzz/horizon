//! The `furnace_smelt` hook: declarations the mod API generator reads (see `hooks/mod.rs`).
// catalog-order: 0222

pub trait Hooks {
    /// A furnace finished smelting `item` into `result` at `pos`. `result` is
    /// what was made, after `prepare_block_craft`; a cook it left no result is
    /// not told. Server-side.
    #[hook(
        side = "server",
        contract = "listen",
        neoforge = "none (NeoForge has no such event)"
    )]
    fn furnace_smelt();
}
