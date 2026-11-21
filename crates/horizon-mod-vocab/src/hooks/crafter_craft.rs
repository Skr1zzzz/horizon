//! The `crafter_craft` hook: declarations the mod API generator reads (see `hooks/mod.rs`).
// catalog-order: 0189

pub trait Hooks {
    /// A crafter at `pos` completed recipe `recipe`. Not told when
    /// `prepare_block_craft` left no result.
    #[hook(
        side = "server",
        contract = "listen",
        neoforge = "none (NeoForge has no such event)"
    )]
    fn crafter_craft();
}
