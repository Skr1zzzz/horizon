//! `HORIZON_SHADER_PACK_UNCHECKED`: a shader pack's programs compiled without some of wgpu's
//! runtime checks, to measure what they cost. A switch the player sets, off by default; no caller
//! passes checks in, so the unsafe call stays here, behind the environment variable.

use std::sync::OnceLock;

use wgpu::{Backend, ShaderRuntimeChecks};

/// The environment variable that takes the checks off.
const VAR: &str = "HORIZON_SHADER_PACK_UNCHECKED";

/// Which of wgpu's runtime checks are taken off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Off {
    /// Its clamp on every index (`bounds`).
    bounds: bool,
    /// Its bound on every loop (`loops`).
    loops: bool,
    /// Its guard on integer division (`div`).
    div: bool,
}

impl Off {
    fn any(self) -> bool {
        self.bounds || self.loops || self.div
    }

    /// What of it `backend` acts on. wgpu-hal's GL backend never reads a module's runtime
    /// checks (`gles/device.rs`), and its DX12 backend ignores `int_div_checks`: naga's HLSL
    /// always wraps integer division. Vulkan and Metal act on all three.
    fn on(self, backend: Backend) -> Self {
        match backend {
            Backend::Vulkan | Backend::Metal => self,
            Backend::Dx12 => Self { div: false, ..self },
            _ => Self::default(),
        }
    }

    fn checks(self) -> ShaderRuntimeChecks {
        ShaderRuntimeChecks {
            bounds_checks: !self.bounds,
            force_loop_bounding: !self.loops,
            int_div_checks: !self.div,
            ..ShaderRuntimeChecks::checked()
        }
    }

    /// The names of the checks it takes off (`off`), or keeps.
    fn names(self, off: bool) -> String {
        let names: Vec<_> = [
            (self.bounds, "bounds"),
            (self.loops, "loops"),
            (self.div, "div"),
        ]
        .into_iter()
        .filter(|&(taken, _)| taken == off)
        .map(|(_, name)| name)
        .collect();
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    }
}

/// The checks the variable's `value` asks to take off: comma-separated `bounds`, `loops`, `div`,
/// or `all` (or `1`) for the three; `0` and empty words ask for nothing. Also the words it
/// doesn't know, which take nothing off: the words are lower case, so `ALL` is one of them.
fn parse(value: &str) -> (Off, Vec<&str>) {
    let mut off = Off::default();
    let mut unknown = Vec::new();
    for word in value.split(',').map(str::trim) {
        match word {
            "" | "0" => {}
            "1" | "all" => {
                off = Off {
                    bounds: true,
                    loops: true,
                    div: true,
                }
            }
            "bounds" => off.bounds = true,
            "loops" => off.loops = true,
            "div" => off.div = true,
            _ => unknown.push(word),
        }
    }
    (off, unknown)
}

/// The checks taken off pack programs on `device`'s backend. The variable is read when the first
/// pack program compiles, and logged then, with a warning for words it doesn't know; what the
/// backend acts on is worked out for each device, as a later one can run on another backend.
fn pack_off(device: &wgpu::Device) -> Off {
    static ASKED: OnceLock<Off> = OnceLock::new();
    let backend = device.adapter_info().backend;
    let asked = *ASKED.get_or_init(|| {
        let value = std::env::var(VAR).unwrap_or_default();
        let (asked, unknown) = parse(&value);
        if !unknown.is_empty() {
            log::warn!(
                "shader pack: {VAR}: ignored {unknown:?}, which take no check off: it takes \
                 bounds, loops, div or all, in lower case"
            );
        }
        if asked.any() {
            let off = asked.on(backend);
            log::info!(
                "shader pack: {VAR}={value:?} on {backend:?}: the pack's programs compile \
                 without wgpu's checks: {}; still checked: {}{}",
                off.names(true),
                off.names(false),
                match backend {
                    Backend::Vulkan | Backend::Metal => "",
                    Backend::Dx12 if asked.div => {
                        " (integer division stays guarded on DX12: naga's HLSL always wraps it)"
                    }
                    Backend::Dx12 => "",
                    _ => " (this backend ignores the switch)",
                }
            );
        }
        asked
    });
    asked.on(backend)
}

/// The shader module of one of a shader pack's programs (`gfx/pack_post.rs`): without the checks
/// `HORIZON_SHADER_PACK_UNCHECKED` takes off on this backend, else as any other module. Only for
/// a pack's programs: Horizon's own modules keep every check.
pub fn pack_program_module(
    device: &wgpu::Device,
    desc: wgpu::ShaderModuleDescriptor<'_>,
) -> wgpu::ShaderModule {
    let off = pack_off(device);
    if !off.any() {
        return device.create_shader_module(desc);
    }
    // SAFETY: wgpu leaves it to the caller that the shader never indexes out of bounds, never
    // loops forever and never divides an integer by zero (or `MIN` by -1) once a check is off. A
    // shader pack's program is someone else's code that nobody checked for that, so the
    // precondition isn't met: such a program reaches undefined behaviour (a wrong picture, a
    // driver crash, a lost device). Horizon accepts that only because the player asked for it by
    // setting `HORIZON_SHADER_PACK_UNCHECKED`, a measurement switch, as with
    // `experimental_features`: no caller passes checks in, and safe code can't set the variable
    // (`std::env::set_var` is unsafe). Nothing else bounds what the checks bounded, except on
    // Vulkan: robust buffer access, which wgpu turns on where the GPU has it, still bounds buffer
    // accesses, and robust image access, where the GPU has it, image loads; on a GPU without image
    // robustness, `bounds` also unclamps `texelFetch`. On DX12 (buffers bound as root
    // descriptors, which carry no size) and on Metal, nothing bounds buffer reads once `bounds`
    // is off. Indices into a shader's own arrays, vectors and matrices and its loops have no net
    // on any backend.
    unsafe { device.create_shader_module_trusted(desc, off.checks()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether bounds, loops and integer division stay checked, and ray queries tracked.
    fn kept(off: Off) -> (bool, bool, bool, bool) {
        let c = off.checks();
        (
            c.bounds_checks,
            c.force_loop_bounding,
            c.int_div_checks,
            c.ray_query_initialization_tracking,
        )
    }

    /// The variable takes off the checks it names and keeps the others (ray query tracking too,
    /// under `all`); unset, `0` or a word it doesn't know keeps them all.
    #[test]
    fn unchecked_switch_takes_off_only_the_checks_it_names() {
        let asked = |value| kept(parse(value).0);
        for value in ["", "0", "fast"] {
            assert_eq!(asked(value), (true, true, true, true), "{value:?}");
        }
        assert_eq!(asked("bounds"), (false, true, true, true));
        assert_eq!(asked("loops"), (true, false, true, true));
        assert_eq!(asked("div"), (true, true, false, true));
        assert_eq!(asked("bounds, loops"), (false, false, true, true));
        assert_eq!(asked("all"), (false, false, false, true));
        assert_eq!(asked("1"), (false, false, false, true));
    }

    /// Words the variable doesn't know, upper case among them, are reported, not dropped in
    /// silence; the words it knows still count.
    #[test]
    fn unchecked_switch_reports_words_it_does_not_know() {
        let (off, unknown) = parse("ALL, bound,loops, ,0");
        assert_eq!(unknown, ["ALL", "bound"]);
        assert_eq!(kept(off), (true, false, true, true));
        assert!(parse("all").1.is_empty());
    }

    /// What each backend acts on: all three on Vulkan and Metal, no `div` on DX12, none on GL.
    #[test]
    fn unchecked_switch_takes_off_only_what_the_backend_acts_on() {
        let (all, _) = parse("all");
        assert_eq!(kept(all.on(Backend::Vulkan)), (false, false, false, true));
        assert_eq!(kept(all.on(Backend::Metal)), (false, false, false, true));
        assert_eq!(kept(all.on(Backend::Dx12)), (false, false, true, true));
        assert_eq!(kept(all.on(Backend::Gl)), (true, true, true, true));
        assert!(!parse("div").0.on(Backend::Dx12).any());
        assert!(!all.on(Backend::Gl).any());
    }
}
