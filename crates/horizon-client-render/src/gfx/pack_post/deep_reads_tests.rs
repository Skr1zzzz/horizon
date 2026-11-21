//! A buffer's mip chain is made only when some pass may read it past the first level before the
//! buffer is written again (`deep_reads`).
use super::deep_reads::{chains_to_make, deep_bindings};
use std::io::{Cursor, Write};

/// A composite program reading colortex0 and colortex6 at level 0 alone (`texelFetch` and
/// `textureLod` at level 0, `textureSize` of level 0, a function's `textureLod` at 0), and
/// colortex3, 5 and 7 where lower levels may answer (an implicit level, a level from a uniform, a
/// function's implicit level).
fn composite() -> horizon_shaders::Program {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut add = |name: &str, source: &str| {
        zip.start_file(
            format!("shaders/{name}"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(source.as_bytes()).unwrap();
    };
    add(
        "composite.vsh",
        "#version 330 compatibility\nout vec2 uv;\n\
         void main() { uv = gl_MultiTexCoord0.xy; gl_Position = ftransform(); }\n",
    );
    add(
        "composite.fsh",
        "#version 330 compatibility\n\
         uniform sampler2D colortex0;\nuniform sampler2D colortex3;\nuniform sampler2D colortex5;\n\
         uniform sampler2D colortex6;\nuniform sampler2D colortex7;\nuniform float viewWidth;\n\
         in vec2 uv;\n/* RENDERTARGETS: 1 */\nlayout(location = 0) out vec4 o;\n\
         vec4 atBase(sampler2D s, vec2 p) { return texture2DLod(s, p, 0.0); }\n\
         vec4 filtered(sampler2D s, vec2 p) { return texture2D(s, p); }\n\
         void main() {\n    \
             vec4 base = texelFetch(colortex0, ivec2(gl_FragCoord.xy), 0) + texture2DLod(colortex0, uv, 0.0)\n        \
                 + float(textureSize(colortex0, 0).x) + atBase(colortex6, uv);\n    \
             o = base + texture2D(colortex3, uv) + texture2DLod(colortex5, uv, viewWidth) + filtered(colortex7, uv);\n}\n",
    );
    let pack = horizon_shaders::Pack::from_zip_bytes(
        "original-deep-reads",
        zip.finish().unwrap().into_inner(),
    )
    .unwrap();
    let mut macros = horizon_shaders::Macros::standard();
    let (properties, _) = horizon_shaders::read_properties(&pack, &mut macros);
    let post = horizon_shaders::load_post(&pack, "", &properties, &macros, &Default::default());
    let composite = (post.programs.into_iter())
        .find(|p| p.name == "composite")
        .unwrap_or_else(|| panic!("composite not translated: {:?}", post.notes));
    composite.program.expect("translated")
}

/// Level 0 named outright, in the program or in a function it hands the texture to, keeps a read
/// on the first level; an implicit level or one worked out at run time may leave it.
#[test]
fn only_reads_that_may_leave_the_first_level_count() {
    let program = composite();
    let deep = deep_bindings(&program.fragment).expect("every read followed");
    let mut names: Vec<&str> = (program.interface.samplers.iter())
        .filter(|s| deep.contains(&s.binding))
        .map(|s| s.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["colortex3", "colortex5", "colortex7"]);
    assert_eq!(deep_bindings(&program.vertex), Some(Vec::new()));
}

/// A chain is made when a pass reads it past the first level before something writes the
/// buffer, and when the game's draws still to come may.
#[test]
fn a_chain_is_made_for_a_deep_read_while_it_stays_current() {
    const C0: u16 = 1;
    const C3: u16 = 1 << 3;
    // (deep reads, writes) of each pass from the one asking for the chains.
    // It reads colortex0 at level 0 and writes it: nothing reads the chain.
    assert_eq!(chains_to_make(C0, [(0, C0)], false), 0);
    assert_eq!(chains_to_make(C0, [(0, C0)], true), 0);
    // It leaves colortex0 current, and the game's draws may read it.
    assert_eq!(chains_to_make(C0, [(0, 0)], true), C0);
    // A later pass reads colortex0 deep before anything writes it; colortex3 was written.
    assert_eq!(chains_to_make(C0 | C3, [(0, C3), (C0 | C3, 0)], false), C0);
    // Only after a pass wrote it.
    assert_eq!(chains_to_make(C0, [(0, 0), (0, C0), (C0, 0)], false), 0);
    // The asking pass's own read counts, though it writes the buffer.
    assert_eq!(chains_to_make(C3, [(C3, C3)], false), C3);
}
