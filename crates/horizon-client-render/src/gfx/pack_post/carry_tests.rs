//! What a buffer's alt side holds matters from the first pass of the frame that shows it, unless
//! that pass overwrites it whole (`carry`).
use super::carry::{discards, first_shown};

const C1: u16 = 1 << 1;
const C2: u16 = 1 << 2;
const C4: u16 = 1 << 4;
const C5: u16 = 1 << 5;
const C7: u16 = 1 << 7;

/// A buffer counts as overwritten first only when the first pass to show its alt side
/// overwrites it whole: a later whole write, after a partial one or a flip, is too late.
#[test]
fn only_the_first_pass_to_show_alt_counts() {
    // (shows alt of, overwrites whole) for each pass.
    let passes = [(C5, C5), (C7 | C1, C7), (0, 0), (C2 | C1, C2 | C1)];
    assert_eq!(
        first_shown(C1 | C2 | C4 | C5 | C7, passes),
        (C2 | C5 | C7, C4)
    );
    // A stage's flip shows alt without writing it.
    assert_eq!(first_shown(C5, [(C5, 0), (C5, C5)]), (0, 0));
}

/// A `discard` anywhere a fragment stage can reach, a function it calls included, leaves pixels
/// unwritten.
#[test]
fn a_discard_in_a_called_function_counts() {
    let cut = naga::front::wgsl::parse_str(
        "fn cut(a: f32) {\n    if a < 0.5 {\n        discard;\n    }\n}\n\
         @fragment fn main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {\n    \
         cut(p.x);\n    return vec4(1.0);\n}\n",
    )
    .unwrap();
    assert!(discards(&cut));
    let whole = naga::front::wgsl::parse_str(
        "@fragment fn main() -> @location(0) vec4<f32> {\n    return vec4(1.0);\n}\n",
    )
    .unwrap();
    assert!(!discards(&whole));
}
