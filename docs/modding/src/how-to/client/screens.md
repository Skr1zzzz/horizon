# Screens, the inventory key and the sign editor

A client mod can close the screen a player has open, open the player's inventory, and read which sign
the sign editor is on. The two writes take the same paths as the keys the player uses, so the server
sees only the packets of a normal client.

```rust
// Escape: close whatever is open.
if host.close_screen() {
    // a screen was open and is now closed
}

// The inventory key, key.inventory.
if host.open_inventory() {
    // the inventory screen is up (or the ridden vehicle's screen was asked for)
}

// Which sign the editor is on, if any.
if let Some(sign) = host.sign_editor() {
    let _ = (sign.pos, sign.front);
}
```

`ClientHost::close_screen()` follows the screen's own Escape close. A container screen closes as the
player's container close: the client sends `ServerboundContainerClosePacket` with the menu's id and then
closes its copy of the menu. It never drops a container menu locally alone, which would leave the
server's menu open. A pause menu closes to the game, another screen runs its own close, and the sign
editor sends `ServerboundSignUpdatePacket` with the lines being edited, as Escape does. The death
screen does not close this way. The screen hooks' closing notices fire as they do for Escape. It returns
`false` when no screen is open.

`ClientHost::open_inventory()` does what `key.inventory` does: it opens the player's inventory screen
locally and sends nothing, because the player's menu is always open on the server. Riding a vehicle
with its own inventory (a tamed mount or a chest boat), it sends the open-inventory player command
(`ServerboundPlayerCommandPacket`) at the player's next tick instead, as the key's press is. It returns
`false` with a screen already open, for a spectator, in a portal transition, or with no world.

`ClientHost::sign_editor()` returns `Some(SignEditView { pos, front })` while the sign editor is open:
the sign's block position and whether its front side is being edited. It returns `None` with no sign
editor open, or outside a world. A close of the sign editor always sends the sign update, however it
closes; there is no close that skips it, because the vanilla client never omits that packet.

## Quit Game

A native client mod with its own Quit Game confirmation can call `host.request_client_quit()`
after the player confirms. The call works from a title input hook's shared `ClientHost`.
It returns `true` when queued, then the client starts its normal shutdown after the hook
dispatch returns. This includes saving and stopping an open integrated server.
It returns `false` if a quit is already queued or underway, or when the host cannot quit
the primary window, for example on a headless client or a splitscreen guest.

## Taking a screenshot

`ClientHost::request_screenshot()` takes a screenshot as the screenshot key (F2) does. The next
frame is read back at the size F2 saves, the `screenshot` hook runs on it as it does for F2 (so every
mod's screenshot listener sees mod-started screenshots too, and may change, rename or cancel them), and
the PNG is written to the player's `screenshots` folder under vanilla's file name with F2's chat line.
`Ok(())` means the screenshot is queued, not yet written.

A mod-started screenshot never replaces a file. Each PNG is written to a temporary file in the same
folder and then published under its final name only if no file has that name: a name the hook picked
that already exists keeps the existing file, the new one is deleted, and the chat line reports the
failure. A failed write leaves no partial file. A name the hook picked may create at most one new
folder: a deeper path reports the failure and writes nothing.

It returns `Err(ScreenshotError)` and queues nothing:

| Error | When |
|---|---|
| `Busy` | a capture is queued or being read back, the `screenshot` hook is running, a screenshot is still being written, or a screenshot sequence runs |
| `BudgetExhausted` | mod-started screenshots have written 4 GiB this game session, shared by all mods |
| `Folder` | the `screenshots` folder could not be created |
| `Unavailable` | no window renders here: a server's thread or a client without a renderer |

One screenshot runs at a time, so a mod cannot queue a burst. The session budget counts each written
capture at its uncompressed size after the hook (4 bytes a pixel, 6 for an HDR copy), an upper bound on
the PNGs, and at least 1 MiB: about 500 screenshots at 1920 by 1080, 120 at 3840 by 2160, and never more
than 4096. A capture that would pass the budget once the hook has run is not written, and the chat line
reports the failure. The budget is reset when the game restarts. F2 itself is never limited.

## Moving and hiding a container screen's slots

A client mod that redraws a vanilla container screen (Legacy4J's inventory, crafting table or
furnace layouts, for example) can move the open screen's slots to its own artwork, or hide them, as
a Java screen sets `Slot.x`/`Slot.y` or deactivates a slot:

```rust
// Menu index 0 of a crafting table is its result: put it at (134, 30).
host.set_screen_slot(0, Some([134.0, 30.0]))?;
// On the same screen, hide the first slot of the player's inventory.
host.set_screen_slot(10, None)?;
// Where a slot is now, or None while hidden.
let result = host.screen_slot(0);
```

`slot` is a menu slot index, in the order `MenuRef::slot_boxes` uses. A position is the item's top
left corner in GUI pixels from the screen's `rect` top left (`leftPos`, `topPos`): the numbers a Java
`Slot` holds. From the next frame the slot (its item, its empty-slot icon and the recipe book's ghost
item) is drawn, hovered, highlighted, clicked, dragged over and given its tooltip at its new place,
and `MenuRef::slot_boxes` reports it there; a hidden slot is none of these, and `ScreenView.slots` no
longer counts it. A position outside the screen's image is allowed: a click on a moved slot there is
a click on the slot, where vanilla would count it outside the window, as a Java screen that places
slots outside its image widens `hasClickedOutside` over them. The screen's background is the mod's
to draw. `ClientHost::screen_slot()` reads the slot's place back at once, before the frame
that draws it.

Only the screen changes. Nothing is sent: the menu, its slot indices and the server keep every
slot, so a hidden slot keeps its item and a shift-click may still fill it, as with a deactivated
Java slot. The changes last until the screen closes or another menu replaces it; the next open
starts from the screen's own layout. Repeating a slot's current place changes nothing, so a screen
hook may call it every frame.

It returns `Err(ScreenSlotRefusal)` and changes nothing:

| Refusal | When |
|---|---|
| `NoScreen` | no container screen is open: another screen or none, the creative inventory, or a native mod's own screen for the menu, which places its slots itself |
| `NoSuchSlot` | the open menu has no slot of that index |
| `HiddenAtOpen` | showing a slot the screen did not lay out when it opened, as vanilla leaves a mount's missing saddle or body armour slot inactive (hiding it again is already so, and returns `Ok`) |
| `BadPosition` | a coordinate is not finite; there is no other bound |
| `Unavailable` | on a server, or with no game backend |
