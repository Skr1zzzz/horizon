//! Opt-in switches that leave out work done for a buffer's "alt" side (6.1.6) when the first
//! pass of the frame to show that side overwrites it whole, which only holds if that pass's
//! vertex stage covers its whole target. A composite program's does as packs write them
//! (`gl_Position = ftransform()` over the full-screen quad), but nothing checks it, so they are
//! off by default.
//!
//! A buffer's alt side is shown by a pass writing it (a composite pass writes alt, keeping
//! what it doesn't draw over or blending onto it) and by a flip (alt becomes main): reads,
//! gbuffers draws and images use main alone. Until the first of these, what alt holds changes
//! no pixel; when the first is a write that replaces every pixel, never.

use std::sync::OnceLock;

/// Whether `module` (a fragment stage) may discard: a `discard` anywhere in it.
pub(super) fn discards(module: &naga::Module) -> bool {
    fn block(b: &naga::Block) -> bool {
        b.iter().any(|s| match s {
            naga::Statement::Kill => true,
            naga::Statement::Block(b) => block(b),
            naga::Statement::If { accept, reject, .. } => block(accept) || block(reject),
            naga::Statement::Switch { cases, .. } => cases.iter().any(|c| block(&c.body)),
            naga::Statement::Loop {
                body, continuing, ..
            } => block(body) || block(continuing),
            _ => false,
        })
    }
    module.functions.iter().any(|(_, f)| block(&f.body))
        || module.entry_points.iter().any(|e| block(&e.function.body))
}

/// Of the buffers `open`: those whose alt side the frame's passes first show in a pass that
/// overwrites it whole, and those whose alt side no pass shows. `passes` gives, for each pass of
/// the frame in order (a stage's flips as a pass of their own), the buffers it shows alt of (it
/// writes or flips them) and those it overwrites whole.
pub(super) fn first_shown(open: u16, passes: impl IntoIterator<Item = (u16, u16)>) -> (u16, u16) {
    let (mut open, mut whole) = (open, 0);
    for (shown, overwritten) in passes {
        let first = open & shown;
        whole |= first & overwritten;
        open &= !first;
        if open == 0 {
            break;
        }
    }
    (whole, open)
}

/// Whether switch `name` is set to `1`, read once and logged (`what` it does) when it is.
fn switch(lock: &OnceLock<bool>, name: &str, what: &str) -> bool {
    *lock.get_or_init(|| {
        let on = std::env::var(name).is_ok_and(|v| v.trim() == "1");
        if on {
            log::info!("shader pack: {name}: {what}");
        }
        on
    })
}

/// `HORIZON_SHADER_PACK_NO_FLIP_COPY=1`: a buffer that isn't cleared and flipped an odd number
/// of times in the last frame has its main side copied to alt, as the frame starts, only when the
/// first pass of the frame to show alt doesn't overwrite it whole.
pub(super) fn no_flip_copy() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    switch(
        &ON,
        "HORIZON_SHADER_PACK_NO_FLIP_COPY",
        "buffers that aren't cleared skip the copy to their other side where the first pass to \
         show that side overwrites it whole",
    )
}

/// `HORIZON_SHADER_PACK_NO_ALT_CLEAR=1`: a cleared buffer's alt side is cleared as the frame
/// starts only when a pass of the frame shows it before overwriting it whole.
pub(super) fn no_alt_clear() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    switch(
        &ON,
        "HORIZON_SHADER_PACK_NO_ALT_CLEAR",
        "cleared buffers skip clearing their other side where no pass shows it before \
         overwriting it whole",
    )
}
