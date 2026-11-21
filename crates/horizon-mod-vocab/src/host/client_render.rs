//! Host method declarations; access defaults and game forwarders are generated.
#![allow(unused_variables)]
use crate::*;
/// The client host's methods of the `client_render` area, implemented by the game and reached through [`crate::ClientHost`].
#[horizon_mod_api_macros::host_area]
pub trait ClientRenderClientHost {
    /// Atomically overwrite retained model copies; use [`InstanceGroupRef::write`].
    #[access(PublishedResources)]
    fn instance_group_write(
        &self,
        group: InstanceGroupRef,
        first: u32,
        instances: &[ModelInstance],
    ) -> Result<(), GroupError>;
    /// Set the drawn length; use [`InstanceGroupRef::set_len`].
    #[access(PublishedResources)]
    fn instance_group_set_len(&self, group: InstanceGroupRef, len: u32) -> Result<(), GroupError>;
    /// Replace the group pose; use [`InstanceGroupRef::set_group_transform`].
    #[access(PublishedResources)]
    fn instance_group_set_transform(
        &self,
        group: InstanceGroupRef,
        transform: [f32; 12],
    ) -> Result<(), GroupError>;
    /// Retire retained model copies; use [`InstanceGroupRef::destroy`].
    #[access(PublishedResources)]
    fn instance_group_destroy(&self, group: InstanceGroupRef) -> Result<(), GroupError>;
    /// Side: client.
    /// Asks `block_render_visibility` again for every state and, when the answers differ
    /// from those in use, remeshes every loaded section with them. Call it when your answer changes (a toggle key, a scan
    /// finishing) and once when the player joins, not every frame: a rebuild takes the workers a
    /// few seconds. With no part answering, nothing is asked and the mesher keeps its no-mod path.
    #[access(WorldOnly)]
    fn refresh_render_visibility(&mut self);
    /// Side: client.
    /// Asks `biome_colors` again for every biome and, when the colours
    /// differ from those in use, rebuilds every loaded section and the far
    /// terrain with them (`LevelRenderer.allChanged`, as changing Biome Blend
    /// does). Call it when your colours change (a new season, a new day of a
    /// slow fade) and once when the player joins, not every frame: a rebuild
    /// takes the workers a few seconds.
    #[access(WorldOnly)]
    fn refresh_biome_colors(&mut self);
    /// Side: client.
    /// Meshes again every loaded section that the block box from `min` to `max` (both corners included, in
    /// either order) reaches, on the workers, as when their blocks change: each mod's `SectionGeometry` is
    /// asked for those sections again. Call it when what your renderer adds there changes (a schematic moved,
    /// turned or got new blocks), with the box it left and the box it now covers, not every frame: each section
    /// costs the workers a mesh. Sections above or below the level and columns this client hasn't loaded are
    /// left out; a column that loads later meshes then anyway.
    #[access(WorldOnly)]
    fn remesh_sections(&mut self, min: [i32; 3], max: [i32; 3]);
    /// Side: client.
    /// A mod's own RGBA image, uploaded into the HUD atlas slot
    /// `MOD_IMAGE_SPRITE` and drawn with
    /// `native_sprite`: its own source rectangle crops and stretches it. `rgba`
    /// is `width * height` pixels of four bytes, each side at most
    /// `MOD_IMAGE_SIZE`; false when it is not that
    /// size. The upload costs a texture patch of the image, so send one when the
    /// image changed, not every frame.
    ///
    /// The slot exists whenever a client mod, native or wasm, is loaded: the atlas packs it at
    /// load and on a resource reload, with no mixin and no opt-in. It is one image for all mods
    /// together: a second mod's write replaces the first's.
    #[access(WorldOnly)]
    fn mod_image(&mut self, width: u32, height: u32, rgba: &[u8]) -> bool;
    /// Side: client.
    /// For a client mod drawing a block's faces itself (a copycat drawing its material's, from
    /// `block_sprites`): the 0xRRGGBB colour a face of `state` with tint index `tint_index`
    /// (`FaceSprite::tint_index`) is multiplied by at `pos`, as the chunk mesher tints it there
    /// (`BlockColors.getColor`, biome blend included: grass, foliage and water follow the biome
    /// around `pos`). None for an untinted face (-1, or an index past the state's tints). Reads
    /// this client's copy of the world.
    #[access(WorldOnly)]
    fn block_tint_color(&self, state: BlockState, tint_index: i32, pos: [i32; 3]) -> Option<u32>;
    /// Side: client.
    /// Published asset/data read: available in Setup, Reloading and World.
    /// For a client mod: what each resolved block state draws on its six sides, preserving
    /// its properties without reparsing names. Resolve states once at load. Each face's sprite (array and
    /// layer, as `WorldDrawn::block_sprites` cuts them) and its texture by the name `cube` and `quad` take,
    /// where it lies on the side, how its texture runs across it (`FaceSprite::quad_on` draws it on another
    /// block's box, turned as its model turns it), its tint source and index (`block_tint_color`) and
    /// whether it is cut out, translucent or animated; its particle sprite; whether it is a plain
    /// cube. None for a block the game does not know. The model is the state's first random variant. API
    /// 380.
    #[access(PublishedResources)]
    fn block_sprites(&self, blocks: &[BlockState]) -> Vec<Option<BlockSprites>>;
    /// Side: client.
    /// Published asset/data read: available in Setup, Reloading and World.
    /// For a client mod drawing a block where none is (a schematic's ghost blocks, added through
    /// `add_section_geometry`): every quad of `state`'s baked model, in the order the chunk mesher draws them,
    /// from the same models. Those `block_sprites` leaves out are included: a flower's crossed planes, an
    /// element turned on an axis. Each gives its corners and UVs, its sprite and that sprite's atlas name, the
    /// side it faces, its cull face, its tint index and source, its shade and whether it is translucent. The
    /// model is the variant `block_sprites` reads. Empty for a state with no model quads (air, a fluid, a block
    /// drawn only as a block entity); None for a state the game does not know. Ask again when
    /// `block_sprites_version` changes.
    #[access(PublishedResources)]
    fn block_model_quads(&self, state: BlockState) -> Option<Vec<BlockModelQuad>>;
    /// Side: client.
    /// Published asset/data read: available in Setup, Reloading and World.
    /// For a client mod: each texture's sprite by name (`minecraft:block/stone`, `block/stone`), none for one no
    /// model uses. API 380.
    #[access(PublishedResources)]
    fn texture_sprites(&self, textures: &[&str]) -> Vec<Option<SpriteRef>>;
    /// Side: client.
    /// Published asset/data read: available in Setup, Reloading and World.
    /// For a client mod reading the player's own block textures (resource packs included) on the CPU: a map
    /// averaging each block's top colour, a software renderer. Each sprite's (`block_sprites`, `texture_sprites`)
    /// RGBA8, rows top to bottom, square, every one the arrays' edge (`rgba.len()` is edge × edge × 4): the pixels
    /// the arrays were filled with when the resources loaded, so an animated sprite's is the frame it starts on.
    /// None for a sprite past the arrays' layers, and for all of them on a client that loaded no pixels. The
    /// pixels are kept compressed once the GPU has them: one call uncompresses them all once, so ask for every
    /// sprite at once and again only when `block_sprites_version` changes. `Err`, reading nothing, for more than
    /// 256 MiB of pixels in one call (`MAX_SPRITE_PIXEL_BYTES`, the default texture budget: every layer fits
    /// unless the player raised `textureBudgetMiB`; ask in parts then).
    #[access(PublishedResources)]
    fn sprite_pixels(&self, sprites: &[SpriteRef]) -> Result<Vec<Option<Vec<u8>>>, String>;
    /// Side: client.
    /// Published asset/data read: available in Setup, Reloading and World.
    /// For a client mod: where GUI sprite `sprite` lies in the GUI atlas and its size, none for one the atlas lacks
    /// (a server's metrics-only atlas lacks them all). The atlas names vanilla's sprites by path alone
    /// (`gui/sprites/container/slot`) and a mod's with its namespace (`mymod:gui/panel`).
    #[access(PublishedResources)]
    fn gui_sprite(&self, sprite: &Identifier) -> Option<AtlasRegion>;
    /// Side: client.
    /// Published asset/data read: available in Setup, Reloading and World.
    /// For a client mod: changes each time the resources load (a resource pack turned on or off can move every
    /// sprite): ask `native_block_sprites` again then. API 380.
    #[access(PublishedResources)]
    fn block_sprites_version(&self) -> u64;
}
