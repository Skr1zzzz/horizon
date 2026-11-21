//! Client mods' dynamic textures (`ClientRenderClientHost::create_dynamic_texture`, #11138),
//! vanilla's `DynamicTexture` registered in the `TextureManager` under an id. Here every image
//! lives in one page of the HUD atlas ([`PAGE`] texels) that the atlas packs only when a client
//! mod is loaded ([`super::HudAtlas::build_over_with_image`]), placed by a shelf packer. Its id
//! names its rect among the atlas's sprites, so `DrawContext::sprite` (`native_sprite`),
//! `gui_sprite` and an inline `TextObject::Atlas` of `minecraft:gui` draw it as any other GUI
//! sprite, and no resource reload is needed. An update rewrites the image in place when it still
//! fits there; a free drops the name and gives the room back. The last pixels of each image stay
//! here, counted in the mod's byte budget, to patch the atlas texture again after a resource
//! reload or for a new renderer.
use super::{HudAtlas, Rect, gui_sprite::atlas_key_of_gui_sprite};
use mod_api::{
    DYNAMIC_TEXTURE_MAX_SIDE, DYNAMIC_TEXTURE_MOD_BYTES, DYNAMIC_TEXTURE_MOD_IMAGES,
    DynamicTextureBudget, DynamicTextureError, Identifier,
};
use std::collections::{BTreeMap, HashMap};

/// The page's size in texels: two mods' whole byte budgets (`DYNAMIC_TEXTURE_MOD_BYTES`), 8 MiB
/// of atlas, paid only by a game with a client mod loaded. It is wide rather than tall: the
/// atlas packs its tallest images first and rounds its height up to whole 1024-row bands.
pub const PAGE: [u32; 2] = [2048, 1024];

/// The atlas sprite of the page while the atlas is packed; [`Page::take`] then takes it out of
/// the sprites, so no mod draws the page by name and sees another mod's images.
pub(super) const PAGE_SPRITE: &str = "horizon/dynamic-textures";

/// Where the page lies in one packed atlas, in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// Its top left corner.
    pub origin: [u32; 2],
    /// Its width and height.
    pub size: [u32; 2],
    /// The atlas's width and height.
    pub atlas: [u32; 2],
}

impl Page {
    /// The packed page of an atlas `width` x `height`, taken out of its `sprites`; none when it
    /// packed none (no client mod was loaded).
    pub(super) fn take(
        sprites: &mut HashMap<String, Rect>,
        width: u32,
        height: u32,
    ) -> Option<Self> {
        let rect = sprites.remove(PAGE_SPRITE)?;
        let (w, h) = rect.texels((width, height));
        Some(Self {
            origin: [
                (rect.u0 * width as f32).round() as u32,
                (rect.v0 * height as f32).round() as u32,
            ],
            size: [w, h],
            atlas: [width, height],
        })
    }
    /// The sprite of a `size` image at `space`: its texels from the space's corner, its GUI size
    /// the image's.
    fn rect(&self, space: Space, size: [u32; 2]) -> Rect {
        let (x, y) = (self.origin[0] + space.x, self.origin[1] + space.y);
        let [width, height] = self.atlas.map(|side| side as f32);
        Rect {
            u0: x as f32 / width,
            v0: y as f32 / height,
            u1: (x + size[0]) as f32 / width,
            v1: (y + size[1]) as f32 / height,
            w: size[0],
            h: size[1],
        }
    }
    /// `image`'s pixels as a patch at its space, with a transparent column to its right and row
    /// below it where its space has them: the gap keeps a neighbour's texels, or those of an
    /// image freed there before, from bleeding in at the sprite's edges.
    fn patch(&self, image: &Image) -> crate::font::Patch {
        let [w, h] = image.size.map(|side| side as usize);
        let width = (w + 1).min(image.space.width as usize);
        let height = (h + 1).min(image.space.height as usize);
        let mut rgba = vec![0; width * height * 4];
        for (row, src) in image.pixels.chunks_exact(w * 4).enumerate() {
            rgba[row * width * 4..][..w * 4].copy_from_slice(src);
        }
        crate::font::Patch {
            x: self.origin[0] + image.space.x,
            y: self.origin[1] + image.space.y,
            width: width as u32,
            height: height as u32,
            rgba,
        }
    }
}

/// A place in the page, in texels from its corner: the columns an image took of its shelf, and
/// the shelf's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Space {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// The texels an image of `size` takes in a page of `page`: one more each way for the gap, but
/// never more than the page (at its edge the atlas's own gap is there).
fn need(page: [u32; 2], size: [u32; 2]) -> [u32; 2] {
    [(size[0] + 1).min(page[0]), (size[1] + 1).min(page[1])]
}

/// The page's shelf packer: rows of images, each as tall as the first image that opened it, and
/// the free runs of columns of each. An image takes the shortest shelf tall enough that wastes at
/// most half its height, else a new shelf below the last, else any shelf tall enough; a freed
/// image's columns go back to its shelf, and emptied shelves at the bottom close.
#[derive(Clone, Debug, Default)]
struct Shelves {
    rows: Vec<Shelf>,
}

#[derive(Clone, Debug)]
struct Shelf {
    y: u32,
    height: u32,
    /// Free runs of columns, `[x, width]`, left to right, none touching another.
    free: Vec<[u32; 2]>,
}

impl Shelves {
    /// A space of `w` x `h` texels in a page of `page`, or none when no shelf has room and no new
    /// shelf fits below the last.
    fn alloc(&mut self, page: [u32; 2], [w, h]: [u32; 2]) -> Option<Space> {
        if w > page[0] || h > page[1] {
            return None;
        }
        let row = match shortest(&self.rows, [w, h], true) {
            Some(row) => row,
            None => {
                let top = self.rows.last().map_or(0, |shelf| shelf.y + shelf.height);
                if top + h <= page[1] {
                    self.rows.push(Shelf {
                        y: top,
                        height: h,
                        free: vec![[0, page[0]]],
                    });
                    self.rows.len() - 1
                } else {
                    shortest(&self.rows, [w, h], false)?
                }
            }
        };
        let shelf = &mut self.rows[row];
        let run = shelf.free.iter().position(|run| run[1] >= w)?;
        let [x, width] = shelf.free[run];
        if width == w {
            shelf.free.remove(run);
        } else {
            shelf.free[run] = [x + w, width - w];
        }
        Some(Space {
            x,
            y: shelf.y,
            width: w,
            height: shelf.height,
        })
    }
    /// `space`'s columns back to its shelf in a page `page_width` wide.
    fn free(&mut self, page_width: u32, space: Space) {
        let Some(shelf) = self.rows.iter_mut().find(|shelf| shelf.y == space.y) else {
            return;
        };
        let free = &mut shelf.free;
        let at = free.partition_point(|run| run[0] < space.x);
        free.insert(at, [space.x, space.width]);
        if at + 1 < free.len() && free[at][0] + free[at][1] == free[at + 1][0] {
            free[at][1] += free[at + 1][1];
            free.remove(at + 1);
        }
        if at > 0 && free[at - 1][0] + free[at - 1][1] == free[at][0] {
            free[at - 1][1] += free[at][1];
            free.remove(at);
        }
        while self
            .rows
            .last()
            .is_some_and(|shelf| shelf.free == [[0, page_width]])
        {
            self.rows.pop();
        }
    }
}

/// The shortest of `rows` tall enough for `w` x `h` with a free run that wide, and when `snug`,
/// no more than half as tall again.
fn shortest(rows: &[Shelf], [w, h]: [u32; 2], snug: bool) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, shelf)| {
            shelf.height >= h
                && (!snug || shelf.height <= h + h / 2)
                && shelf.free.iter().any(|run| run[1] >= w)
        })
        .min_by_key(|(_, shelf)| shelf.height)
        .map(|(row, _)| row)
}

/// Every mod's dynamic textures: where each lies in the page, and its last pixels.
#[derive(Default)]
pub struct DynamicTextures {
    /// Each image by its id (`mymod:emoji/smile`), the name it has among the atlas's sprites.
    images: BTreeMap<String, Image>,
    shelves: Shelves,
    limits: Limits,
}

struct Image {
    /// The native mod that created it: its budget counts the image, and only it changes it.
    owner: Box<str>,
    space: Space,
    size: [u32; 2],
    /// The last pixels given, `size` RGBA: what the atlas gets again after a rebuild.
    pixels: Vec<u8>,
    /// Whether its id is among the atlas's sprites as this image: a sprite the packs brought
    /// under the same name after a resource reload keeps the name.
    named: bool,
    /// Whether the atlas texture lacks these pixels yet: set by a create, an update, a rebuilt
    /// atlas or a new renderer, cleared when [`DynamicTextures::patches`] hands them to a frame.
    dirty: bool,
}

/// Each mod's budgets and the longest side: the API's own, smaller in tests.
#[derive(Clone, Copy, Debug)]
struct Limits {
    images: u32,
    bytes: u64,
    side: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            images: DYNAMIC_TEXTURE_MOD_IMAGES,
            bytes: DYNAMIC_TEXTURE_MOD_BYTES,
            side: DYNAMIC_TEXTURE_MAX_SIDE,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Held {
    images: u32,
    bytes: u64,
}

impl DynamicTextures {
    /// Mod `owner` (none outside a mod's call) creates image `id` of `size` (width, height) from
    /// `rgba`, `width * height` pixels of four bytes, in room of `atlas`'s page, and names it
    /// among the atlas's sprites. Every refusal comes before the pixels are copied, and changes
    /// nothing.
    pub fn create(
        &mut self,
        atlas: &mut HudAtlas,
        owner: Option<&str>,
        id: &Identifier,
        size: [u32; 2],
        rgba: &[u8],
    ) -> Result<(), DynamicTextureError> {
        let page = atlas
            .dynamic_texture_page
            .ok_or(DynamicTextureError::Unavailable)?;
        let owner = owned(owner, id)?;
        let bytes = self.sized(size, rgba.len())?;
        let name = id.to_string();
        if self.images.contains_key(&name) || taken(&atlas.sprites, &name) {
            return Err(DynamicTextureError::NameTaken);
        }
        let held = self.held(owner);
        if held.images >= self.limits.images || held.bytes + bytes > self.limits.bytes {
            return Err(DynamicTextureError::OverBudget);
        }
        let space = self
            .shelves
            .alloc(page.size, need(page.size, size))
            .ok_or(DynamicTextureError::NoSlot)?;
        atlas.sprites.insert(name.clone(), page.rect(space, size));
        atlas.revision = HudAtlas::next_revision();
        self.images.insert(
            name,
            Image {
                owner: owner.into(),
                space,
                size,
                pixels: rgba.to_vec(),
                named: true,
                dirty: true,
            },
        );
        Ok(())
    }
    /// Mod `owner` gives its image `id` new pixels of `size`: in the space it holds when they fit
    /// there, else in new room, the old given back. Every refusal comes before any copy, and
    /// changes nothing.
    pub fn update(
        &mut self,
        atlas: &mut HudAtlas,
        owner: Option<&str>,
        id: &Identifier,
        size: [u32; 2],
        rgba: &[u8],
    ) -> Result<(), DynamicTextureError> {
        let page = atlas
            .dynamic_texture_page
            .ok_or(DynamicTextureError::Unavailable)?;
        let owner = owned(owner, id)?;
        let name = id.to_string();
        let held = self.held(owner);
        let (old, kept) = match self.images.get(&name) {
            None => return Err(DynamicTextureError::Unknown),
            Some(image) if image.owner.as_ref() != owner => {
                return Err(DynamicTextureError::NotOwned);
            }
            Some(image) => (image.space, image.pixels.len() as u64),
        };
        let bytes = self.sized(size, rgba.len())?;
        if held.bytes - kept + bytes > self.limits.bytes {
            return Err(DynamicTextureError::OverBudget);
        }
        let [w, h] = need(page.size, size);
        let space = if w <= old.width && h <= old.height {
            old
        } else {
            let space = self
                .shelves
                .alloc(page.size, [w, h])
                .ok_or(DynamicTextureError::NoSlot)?;
            self.shelves.free(page.size[0], old);
            space
        };
        let image = self.images.get_mut(&name).expect("the image checked above");
        if image.pixels.len() == rgba.len() {
            image.pixels.copy_from_slice(rgba);
        } else {
            image.pixels = rgba.to_vec();
        }
        if image.space != space || image.size != size {
            image.space = space;
            image.size = size;
            if image.named {
                atlas.sprites.insert(name, page.rect(space, size));
                atlas.revision = HudAtlas::next_revision();
            }
        }
        image.dirty = true;
        Ok(())
    }
    /// Mod `owner` frees its image `id`: the name leaves the atlas's sprites, the room and the
    /// budget come back. A refusal changes nothing.
    pub fn free(
        &mut self,
        atlas: &mut HudAtlas,
        owner: Option<&str>,
        id: &Identifier,
    ) -> Result<(), DynamicTextureError> {
        let page = atlas
            .dynamic_texture_page
            .ok_or(DynamicTextureError::Unavailable)?;
        let owner = owned(owner, id)?;
        let name = id.to_string();
        match self.images.get(&name) {
            None => return Err(DynamicTextureError::Unknown),
            Some(image) if image.owner.as_ref() != owner => {
                return Err(DynamicTextureError::NotOwned);
            }
            Some(_) => {}
        }
        let image = self.images.remove(&name).expect("the image checked above");
        self.shelves.free(page.size[0], image.space);
        if image.named {
            atlas.sprites.remove(&name);
            atlas.revision = HudAtlas::next_revision();
        }
        Ok(())
    }
    /// What mod `owner` may still create.
    pub fn budget(
        &self,
        atlas: &HudAtlas,
        owner: Option<&str>,
    ) -> Result<DynamicTextureBudget, DynamicTextureError> {
        atlas
            .dynamic_texture_page
            .ok_or(DynamicTextureError::Unavailable)?;
        let owner = owner.ok_or(DynamicTextureError::NotOwned)?;
        let held = self.held(owner);
        Ok(mod_api::__private::built::dynamic_texture_budget(
            self.limits.images.saturating_sub(held.images),
            self.limits.bytes.saturating_sub(held.bytes),
        ))
    }
    /// A rebuilt atlas (a resource reload): every image keeps its place in the new atlas's page,
    /// its name goes back among the sprites and its kept pixels are patched again. A sprite the
    /// new packs have under the name keeps it. An atlas with no page leaves the names out and
    /// the pixels kept, until one has a page again.
    pub fn rebind(&mut self, atlas: &mut HudAtlas) {
        let page = atlas.dynamic_texture_page;
        for (name, image) in &mut self.images {
            image.dirty = true;
            image.named = page.is_some() && !taken(&atlas.sprites, name);
            match page {
                Some(page) if image.named => {
                    atlas
                        .sprites
                        .insert(name.clone(), page.rect(image.space, image.size));
                }
                Some(_) => log::warn!(
                    "GUI atlas: dynamic texture {name} is hidden by a resource pack's sprite of that name"
                ),
                None => {}
            }
        }
        atlas.revision = HudAtlas::next_revision();
    }
    /// A new renderer, whose atlas texture holds only the packed pixels: every image is patched
    /// again.
    pub fn forget_uploads(&mut self) {
        for image in self.images.values_mut() {
            image.dirty = true;
        }
    }
    /// The images `atlas`'s texture lacks, as patches into its page, each once until it changes
    /// again.
    pub fn patches(&mut self, atlas: &HudAtlas, patches: &mut Vec<crate::font::Patch>) {
        let Some(page) = atlas.dynamic_texture_page else {
            return;
        };
        for image in self.images.values_mut().filter(|image| image.dirty) {
            image.dirty = false;
            patches.push(page.patch(image));
        }
    }
    /// The images and bytes mod `owner` holds.
    fn held(&self, owner: &str) -> Held {
        self.images
            .values()
            .filter(|image| image.owner.as_ref() == owner)
            .fold(Held::default(), |held, image| Held {
                images: held.images + 1,
                bytes: held.bytes + image.pixels.len() as u64,
            })
    }
    /// The bytes of a `[width, height]` image of `len` bytes: `BadSize` for a side of 0 or
    /// pixels that are not `width * height * 4` bytes, `TooLarge` past the longest side. The
    /// sides are checked before the bytes are counted, so the count cannot overflow.
    fn sized(&self, [width, height]: [u32; 2], len: usize) -> Result<u64, DynamicTextureError> {
        if width == 0 || height == 0 {
            return Err(DynamicTextureError::BadSize);
        }
        if width > self.limits.side || height > self.limits.side {
            return Err(DynamicTextureError::TooLarge);
        }
        let bytes = u64::from(width) * u64::from(height) * 4;
        if bytes != len as u64 {
            return Err(DynamicTextureError::BadSize);
        }
        Ok(bytes)
    }
}

/// Whether the atlas has a sprite under `name`, or the GUI sprite an inline `TextObject::Atlas`
/// of `minecraft:gui` finds first for it (`mymod:gui/sprites/emoji/smile` for
/// `mymod:emoji/smile`): an image never shadows a pack's sprite, nor a pack's sprite an image.
fn taken(sprites: &HashMap<String, Rect>, name: &str) -> bool {
    sprites.contains_key(name) || sprites.contains_key(&atlas_key_of_gui_sprite(name))
}

/// `owner`, when `id` is in a namespace `owner` owns (its own or its manifest's other
/// namespaces), never vanilla's `minecraft`.
fn owned<'a>(owner: Option<&'a str>, id: &Identifier) -> Result<&'a str, DynamicTextureError> {
    owner
        .filter(|owner| {
            id.namespace() != "minecraft"
                && mod_api::__private::owns_namespace(owner, id.namespace())
        })
        .ok_or(DynamicTextureError::NotOwned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use DynamicTextureError::*;

    /// A 64 x 32 page at (512, 600) of the 1024 x 1024 boot atlas.
    const ORIGIN: [u32; 2] = [512, 600];

    fn id(text: &str) -> Identifier {
        Identifier::parse(text).unwrap()
    }
    fn atlas(origin: [u32; 2]) -> HudAtlas {
        let mut atlas = HudAtlas::boot();
        atlas.dynamic_texture_page = Some(Page {
            origin,
            size: [64, 32],
            atlas: [atlas.width, atlas.height],
        });
        atlas
    }
    /// Three 16 x 16 images, four images and a 32-texel side a mod.
    fn textures() -> DynamicTextures {
        DynamicTextures {
            limits: Limits {
                images: 4,
                bytes: 3 * 16 * 16 * 4,
                side: 32,
            },
            ..Default::default()
        }
    }
    fn pixels([w, h]: [u32; 2], rgba: [u8; 4]) -> Vec<u8> {
        rgba.repeat((w * h) as usize)
    }
    /// A sprite's rect in texels of the 1024 x 1024 atlas, and its GUI size.
    fn texels(rect: Rect) -> [u32; 6] {
        let at = |uv: f32| (uv * 1024.0).round() as u32;
        [
            at(rect.u0),
            at(rect.v0),
            at(rect.u1),
            at(rect.v1),
            rect.w,
            rect.h,
        ]
    }
    fn drained(textures: &mut DynamicTextures, atlas: &HudAtlas) -> Vec<crate::font::Patch> {
        let mut patches = Vec::new();
        textures.patches(atlas, &mut patches);
        patches
    }

    #[test]
    fn the_shelves_pack_reuse_freed_room_and_refuse_a_full_page() {
        let page = [64, 32];
        let mut shelves = Shelves::default();
        let at = |space: Option<Space>| space.map(|s| [s.x, s.y, s.width, s.height]);
        // Three 17-texel squares share a shelf; a fourth fits neither it nor a shelf below.
        assert_eq!(at(shelves.alloc(page, [17, 17])), Some([0, 0, 17, 17]));
        let second = shelves.alloc(page, [17, 17]).unwrap();
        assert_eq!([second.x, second.y], [17, 0]);
        assert_eq!(at(shelves.alloc(page, [17, 17])), Some([34, 0, 17, 17]));
        assert_eq!(shelves.alloc(page, [17, 17]), None, "a full page");
        // A shorter image opens a shelf below rather than waste the tall one.
        assert_eq!(at(shelves.alloc(page, [8, 4])), Some([0, 17, 8, 4]));
        // A freed image's columns take the next image of its size.
        shelves.free(64, second);
        assert_eq!(at(shelves.alloc(page, [17, 17])), Some([17, 0, 17, 17]));
        // Emptied shelves at the bottom close, so the whole page is free again.
        for space in [
            [0, 17, 8, 4],
            [0, 0, 17, 17],
            [17, 0, 17, 17],
            [34, 0, 17, 17],
        ] {
            let [x, y, width, height] = space;
            shelves.free(
                64,
                Space {
                    x,
                    y,
                    width,
                    height,
                },
            );
        }
        assert_eq!(at(shelves.alloc(page, [64, 32])), Some([0, 0, 64, 32]));
        assert_eq!(shelves.alloc(page, [65, 1]), None, "wider than the page");
    }

    #[test]
    fn an_image_resolves_as_a_gui_sprite_and_an_inline_atlas_object_until_freed() {
        let mut atlas = atlas(ORIGIN);
        let mut textures = textures();
        let smile = id("a:emoji/smile");
        let revision = atlas.revision;
        let red = [255, 0, 0, 255];
        textures
            .create(
                &mut atlas,
                Some("a"),
                &smile,
                [16, 16],
                &pixels([16, 16], red),
            )
            .unwrap();
        assert_ne!(atlas.revision, revision, "cached text layouts look again");
        // `DrawContext::sprite` and `gui_sprite` read the name; the inline object its GUI atlas.
        let expected = [512, 600, 528, 616, 16, 16];
        assert_eq!(texels(atlas.sprites["a:emoji/smile"]), expected);
        let inline = atlas
            .inline_sprite("minecraft:gui", "a:emoji/smile")
            .unwrap();
        assert_eq!(texels(inline), expected);
        // The next frame uploads it once, with a transparent gap right and below.
        let patches = drained(&mut textures, &atlas);
        assert_eq!(patches.len(), 1);
        let patch = &patches[0];
        assert_eq!(
            [patch.x, patch.y, patch.width, patch.height],
            [512, 600, 17, 17]
        );
        let texel = |x: usize, y: usize| &patch.rgba[(y * 17 + x) * 4..][..4];
        assert_eq!(texel(15, 15), red);
        assert_eq!(texel(16, 0), [0; 4]);
        assert_eq!(texel(0, 16), [0; 4]);
        assert!(drained(&mut textures, &atlas).is_empty(), "uploaded once");
        let revision = atlas.revision;
        textures.free(&mut atlas, Some("a"), &smile).unwrap();
        assert_ne!(atlas.revision, revision);
        assert!(!atlas.sprites.contains_key("a:emoji/smile"));
        // The boot atlas has no missing sprite to fall back to.
        assert!(
            atlas
                .inline_sprite("minecraft:gui", "a:emoji/smile")
                .is_none()
        );
    }

    #[test]
    fn every_refusal_is_named_and_changes_nothing() {
        let mut atlas = atlas(ORIGIN);
        atlas.dynamic_texture_page = None;
        let mut textures = textures();
        let square = pixels([16, 16], [1, 2, 3, 4]);
        assert_eq!(
            textures.create(&mut atlas, Some("a"), &id("a:one"), [16, 16], &square),
            Err(Unavailable)
        );
        assert_eq!(textures.budget(&atlas, Some("a")), Err(Unavailable));
        let mut atlas = self::atlas(ORIGIN);
        for name in ["a:one", "a:two", "a:three"] {
            textures
                .create(&mut atlas, Some("a"), &id(name), [16, 16], &square)
                .unwrap();
        }
        let tiny = pixels([1, 1], [9; 4]);
        for name in ["b:p0", "b:p1", "b:p2", "b:p3"] {
            textures
                .create(&mut atlas, Some("b"), &id(name), [1, 1], &tiny)
                .unwrap();
        }
        let budget = |textures: &DynamicTextures, atlas: &HudAtlas, owner| {
            let budget = textures.budget(atlas, Some(owner)).unwrap();
            [u64::from(budget.images), budget.bytes]
        };
        assert_eq!(budget(&textures, &atlas, "a"), [1, 0]);
        assert_eq!(budget(&textures, &atlas, "b"), [0, 3 * 1024 - 4 * 4]);
        assert_eq!(textures.budget(&atlas, None), Err(NotOwned));
        atlas.sprites.insert("a:taken".into(), atlas.white);
        atlas
            .sprites
            .insert("c:gui/sprites/panel".into(), atlas.white);
        let state = |textures: &DynamicTextures, atlas: &HudAtlas| {
            let mut names: Vec<_> = atlas.sprites.keys().cloned().collect();
            names.sort();
            (
                names,
                budget(textures, atlas, "a"),
                budget(textures, atlas, "b"),
            )
        };
        let before = state(&textures, &atlas);
        let (wide, long, big) = (
            pixels([32, 14], [5; 4]),
            pixels([33, 1], [0; 4]),
            pixels([32, 32], [0; 4]),
        );
        type Call<'a> = (
            Option<&'a str>,
            &'a str,
            [u32; 2],
            &'a [u8],
            DynamicTextureError,
        );
        let refusals: [Call<'_>; 12] = [
            (None, "a:x", [1, 1], &tiny, NotOwned),
            (Some("b"), "a:x", [1, 1], &tiny, NotOwned),
            (Some("minecraft"), "minecraft:x", [1, 1], &tiny, NotOwned),
            (Some("c"), "c:x", [0, 4], &[], BadSize),
            (Some("c"), "c:x", [4, 4], &square[..63], BadSize),
            (Some("c"), "c:x", [33, 1], &long, TooLarge),
            (Some("a"), "a:one", [16, 16], &square, NameTaken),
            (Some("a"), "a:taken", [1, 1], &tiny, NameTaken),
            (Some("c"), "c:panel", [1, 1], &tiny, NameTaken),
            (Some("a"), "a:four", [16, 16], &square, OverBudget),
            (Some("b"), "b:p4", [1, 1], &tiny, OverBudget),
            (Some("c"), "c:wide", [32, 14], &wide, NoSlot),
        ];
        for (owner, name, size, rgba, refusal) in refusals {
            let got = textures.create(&mut atlas, owner, &id(name), size, rgba);
            assert_eq!(got, Err(refusal), "create {name} {size:?}");
            assert_eq!(
                state(&textures, &atlas),
                before,
                "create {name} changed nothing"
            );
        }
        let one = id("a:one");
        let updates: [Call<'_>; 5] = [
            (Some("b"), "a:one", [1, 1], &tiny, NotOwned),
            (Some("c"), "c:none", [1, 1], &tiny, Unknown),
            (Some("a"), "a:one", [16, 16], &square[..16], BadSize),
            (Some("a"), "a:one", [32, 32], &big, OverBudget),
            (Some("b"), "b:p0", [32, 14], &wide, NoSlot),
        ];
        for (owner, name, size, rgba, refusal) in updates {
            let got = textures.update(&mut atlas, owner, &id(name), size, rgba);
            assert_eq!(got, Err(refusal), "update {name} {size:?}");
            assert_eq!(
                state(&textures, &atlas),
                before,
                "update {name} changed nothing"
            );
        }
        assert_eq!(textures.free(&mut atlas, Some("b"), &one), Err(NotOwned));
        assert_eq!(
            textures.free(&mut atlas, Some("c"), &id("c:none")),
            Err(Unknown)
        );
        assert_eq!(state(&textures, &atlas), before);
        // A freed image gives its budget and its room back.
        let rect = atlas.sprites["a:two"];
        textures.free(&mut atlas, Some("a"), &id("a:two")).unwrap();
        assert_eq!(budget(&textures, &atlas, "a"), [2, 1024]);
        textures
            .create(&mut atlas, Some("a"), &id("a:four"), [16, 16], &square)
            .unwrap();
        assert_eq!(texels(atlas.sprites["a:four"]), texels(rect));
    }

    #[test]
    fn an_update_replaces_the_pixels_in_place_or_moves_a_larger_image() {
        let mut atlas = atlas(ORIGIN);
        let mut textures = textures();
        let (img, red, green) = (id("a:img"), [255, 0, 0, 255], [0, 255, 0, 255]);
        let create = |textures: &mut DynamicTextures, atlas: &mut HudAtlas, name| {
            let pixels = pixels([8, 8], red);
            textures.create(atlas, Some("a"), &id(name), [8, 8], &pixels)
        };
        create(&mut textures, &mut atlas, "a:img").unwrap();
        create(&mut textures, &mut atlas, "a:next").unwrap();
        drained(&mut textures, &atlas);
        // The same size: new pixels in the same place, and no layout looks again.
        let revision = atlas.revision;
        textures
            .update(&mut atlas, Some("a"), &img, [8, 8], &pixels([8, 8], green))
            .unwrap();
        assert_eq!(atlas.revision, revision);
        assert_eq!(texels(atlas.sprites["a:img"]), [512, 600, 520, 608, 8, 8]);
        let patches = drained(&mut textures, &atlas);
        assert_eq!([patches[0].x, patches[0].y], [512, 600]);
        assert_eq!(patches[0].rgba[..4], green);
        // Smaller: still its place, the sprite its new size.
        textures
            .update(&mut atlas, Some("a"), &img, [6, 4], &pixels([6, 4], green))
            .unwrap();
        assert_ne!(atlas.revision, revision);
        assert_eq!(texels(atlas.sprites["a:img"]), [512, 600, 518, 604, 6, 4]);
        // Larger than its place: it moves past `a:next`, and its old columns come back.
        textures
            .update(
                &mut atlas,
                Some("a"),
                &img,
                [16, 8],
                &pixels([16, 8], green),
            )
            .unwrap();
        assert_eq!(texels(atlas.sprites["a:img"]), [530, 600, 546, 608, 16, 8]);
        let patches = drained(&mut textures, &atlas);
        let at = [
            patches[0].x,
            patches[0].y,
            patches[0].width,
            patches[0].height,
        ];
        assert_eq!(at, [530, 600, 17, 9]);
        create(&mut textures, &mut atlas, "a:back").unwrap();
        assert_eq!(texels(atlas.sprites["a:back"])[..2], [512, 600]);
        assert_eq!(texels(atlas.sprites["a:next"]), [521, 600, 529, 608, 8, 8]);
    }

    #[test]
    fn a_rebuilt_atlas_gets_every_kept_image_again() {
        let mut atlas = atlas(ORIGIN);
        let mut textures = textures();
        let one = id("a:one");
        let blue = [0, 0, 255, 255];
        textures
            .create(&mut atlas, Some("a"), &one, [4, 2], &pixels([4, 2], blue))
            .unwrap();
        drained(&mut textures, &atlas);
        // A resource reload: the page lies elsewhere in the new atlas.
        let mut rebuilt = self::atlas([100, 700]);
        textures.rebind(&mut rebuilt);
        assert_eq!(texels(rebuilt.sprites["a:one"]), [100, 700, 104, 702, 4, 2]);
        let patches = drained(&mut textures, &rebuilt);
        assert_eq!(patches.len(), 1);
        assert_eq!([patches[0].x, patches[0].y], [100, 700]);
        assert_eq!(patches[0].rgba[..4], blue);
        // A new renderer's texture has only the packed pixels.
        textures.forget_uploads();
        assert_eq!(drained(&mut textures, &rebuilt).len(), 1);
        // An atlas with no page names nothing and refuses every call, keeping the image.
        let mut bare = self::atlas(ORIGIN);
        bare.dynamic_texture_page = None;
        textures.rebind(&mut bare);
        assert!(!bare.sprites.contains_key("a:one"));
        assert_eq!(textures.free(&mut bare, Some("a"), &one), Err(Unavailable));
        assert!(drained(&mut textures, &bare).is_empty());
        // A pack's sprite of the same name keeps it, through the image's free too.
        let mut shadowed = self::atlas(ORIGIN);
        let white = shadowed.white;
        shadowed.sprites.insert("a:one".into(), white);
        textures.rebind(&mut shadowed);
        assert_eq!(texels(shadowed.sprites["a:one"]), texels(white));
        textures.free(&mut shadowed, Some("a"), &one).unwrap();
        assert_eq!(texels(shadowed.sprites["a:one"]), texels(white));
    }
}
