//! Refusals when a client mod moves or hides a slot of the open container screen.

/// Why `ClientHost::set_screen_slot` changed nothing. Every refusal leaves the screen's
/// slots as they were.
///
/// The cases are part of the call's wire fingerprint and are fixed once published:
/// adding, renaming or reordering one refuses every wasm mod built before the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[derive(horizon_mod_api_macros::Wire)]
pub enum ScreenSlotRefusal {
    /// No container screen is open: another screen or none, the creative inventory, or a
    /// native mod's own screen for the menu, which lays its slots out itself.
    NoScreen,
    /// The open menu has no slot of that index.
    NoSuchSlot,
    /// The screen did not lay the slot out when it opened, as vanilla leaves a slot
    /// inactive (a mount's missing saddle or body armour slot): it can't be shown.
    HiddenAtOpen,
    /// A coordinate that is not finite or lies more than 1,000,000 GUI pixels from the
    /// screen's `leftPos`/`topPos`.
    BadPosition,
    /// Asked with no game backend, or on a server: screens are the client's.
    Unavailable,
}

impl std::fmt::Display for ScreenSlotRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoScreen => "no container screen is open",
            Self::NoSuchSlot => "the open menu has no such slot",
            Self::HiddenAtOpen => "the slot was hidden when the screen opened",
            Self::BadPosition => "the position is not finite or is too far away",
            Self::Unavailable => "screens are on the client",
        })
    }
}

impl std::error::Error for ScreenSlotRefusal {}

impl crate::NoBackend for ScreenSlotRefusal {
    fn no_backend() -> Self {
        Self::Unavailable
    }
}
