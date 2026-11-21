# Textures made at run time

A client mod that makes its images while the game runs (an emoji set it downloaded, a panel, a gauge, glyphs from
its own font) creates each one under a name and draws it as a GUI sprite, with no resource reload. This is
vanilla's `DynamicTexture`: Java registers one with `TextureManager.register(id, texture)`, sends its pixels with
`DynamicTexture.upload()`, draws it with `GuiGraphics.blit(id, ...)` and frees it with `TextureManager.release(id)`.
In Horizon the image lives in a page of the GUI atlas, so it draws wherever a GUI sprite does. The calls have
Sunrise's names and signatures, so a mod written for one builds for the other.

```rust,ignore
use mod_api::prelude::*;
use mod_api::{DynamicTextureError, Identifier};

const SMILE: &str = "mymod:emoji/smile";

/// Upload one emoji: 32 by 32 pixels, row by row from the top left, four bytes (RGBA) each.
fn upload_smile(host: &mut dyn ClientHost, rgba: &[u8]) -> Result<(), DynamicTextureError> {
    let id = Identifier::parse(SMILE).unwrap();
    match host.create_dynamic_texture(&id, 32, 32, rgba) {
        // Already there: give it the new pixels instead.
        Err(DynamicTextureError::NameTaken) => host.update_dynamic_texture(&id, 32, 32, rgba),
        result => result,
    }
}

fn gui_render_post(&self, _: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
    // The whole image (`src` in its own pixels), drawn over 9 by 9 GUI pixels.
    event
        .batch
        .sprite(SMILE, [0.0, 0.0, 32.0, 32.0], [8.0, 8.0, 9.0, 9.0], None);
}
```

The four calls are on the client host. They change the game's atlas, so they take `&mut dyn ClientHost`: call
them from a hook that has it, such as `client_tick_post`, not from inside a draw.

- **`create_dynamic_texture(id, width, height, rgba)`** makes the image. From then on its id is a GUI sprite of
  `width` x `height` pixels, and its pixels reach the GPU with the next frame.
- **`update_dynamic_texture(id, width, height, rgba)`** gives it new pixels. The size may change: the image stays
  where it is when the new size fits there, and moves when it is larger. Update when the image changed, not every
  frame: each update copies the pixels and uploads the image.
- **`free_dynamic_texture(id)`** frees it: the name is gone at once, and its room in the page and its share of the
  budget come back. An image never freed lasts until the game ends.
- **`dynamic_texture_budget()`** answers what the mod may still create, `DynamicTextureBudget { images, bytes }`.

## Where it draws

The id is a GUI sprite like any other:

- `DrawContext::sprite(id, src, dst, tint)` draws it; `src` is in the image's own pixels.
- `gui_sprite(&id)` finds where it lies in the atlas and its size.
- An inline atlas object in text, `TextContent::Object(TextObject::Atlas { atlas: "minecraft:gui".into(), sprite:
  "mymod:emoji/smile".into(), fallback: None })`, draws it in one 8 x 8 glyph cell. That is how a chat emoji works:
  in `client_chat_received`, replace `message` with a `Text` whose `:smile:` spans are such objects. The text box the
  player types in holds plain characters, so it shows `:smile:` until the message is sent.

Sampling is the atlas's: nearest, as GUI sprites. Each image keeps a transparent texel to its right and below it,
so its neighbours never bleed into its edges.

## Names

The id is in a namespace the calling mod owns: its own id (`mymod:...`) or another namespace its manifest declares;
the path is the mod's to choose. A mod never changes another's image: an id outside its namespaces, or `minecraft:`,
is `NotOwned`. An id that already names an image, or a GUI sprite of the packs, is `NameTaken` on create: either
`mymod:emoji/smile` itself or `mymod:gui/sprites/emoji/smile`, the sprite an inline atlas object finds first for
it. That way one id always draws one picture.

## The page and the budgets

The images share one page of the GUI atlas, 2048 x 1024 texels (8 MiB), which the atlas holds only when a client
mod is loaded: a game without mods pays nothing. Images are placed in rows as they come, first come first served;
when the page has no room left for an image, `create` is `NoSlot`, and so is an `update` that grows an image past
what fits. Free an image, or make it smaller.

| Limit | Value | Refusal |
|---|---|---|
| Images a mod holds | `DYNAMIC_TEXTURE_MOD_IMAGES`, 1024 | `OverBudget` |
| Pixels a mod holds | `DYNAMIC_TEXTURE_MOD_BYTES`, 4 MiB (`width * height * 4` an image) | `OverBudget` |
| Longest side | `DYNAMIC_TEXTURE_MAX_SIDE`, 1024 pixels | `TooLarge` |

A mod's budget holds 1024 emoji of 32 x 32, or one 1024 x 1024 image, and the page holds two mods' whole budgets.
These are limits of the atlas page; vanilla's `TextureManager` has none, so read what remains with
`dynamic_texture_budget` rather than assume. The bytes are what the game keeps of each image's last pixels, to put
them back after a resource reload; an update counts its new size in place of the old, and a free gives its share
back.

## Refusals

Every call answers `Result<_, DynamicTextureError>`, and a refused call changes nothing:

| Refusal | When |
|---|---|
| `NotOwned` | the id is outside the calling mod's namespaces, or the image is another mod's |
| `BadSize` | a side is 0, or the pixels are not `width * height * 4` bytes |
| `TooLarge` | a side is longer than 1024 |
| `NoSlot` | the page has no room left for the image |
| `OverBudget` | the mod would pass 1024 images or 4 MiB |
| `NameTaken` | `create` of an id that names an image or a GUI sprite |
| `Unknown` | `update` or `free` of an id that names no image |
| `Unavailable` | no page here: a server, or no client mod loaded |

Match `DynamicTextureError` non-exhaustively. Sunrise's `ClassChange` is not here: its images live in slots of
fixed size classes, and an update across classes is refused there; here an update that does not fit moves.

## Resource reloads

A resource pack change rebuilds the GUI atlas. Every image keeps its name and its place in the new atlas's page,
and the game puts its last pixels back by itself, with the next frame: there is nothing to upload again. Ask
`gui_sprite` again after a reload if you kept a region: the page may lie elsewhere in the new atlas. If a resource
pack brings a sprite under an image's id, the pack's sprite keeps the name until that pack is turned off again.

## Sprites made at load instead

An image that does not change while the game runs can be a runtime sprite instead
(`Hooks::register_runtime_sprites`): a PNG the atlas stitches at every load. Those are capped at 128 for all mods
together and change only with a resource reload; `mod_image` is one 1024 x 1024 slot shared by every mod. A
downloaded image set, or anything the mod makes after startup, belongs here.
