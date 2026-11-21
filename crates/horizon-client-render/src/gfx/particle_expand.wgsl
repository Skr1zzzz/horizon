// Particle quads expanded on the GPU (gfx/particle_expand.rs): each record is one camera-facing
// quad (`ParticleQuad`), and its four vertices go into the room `EntityBatch` left for its layer
// in the entity vertex buffer, the same words `ParticleQuad::vertices` makes on the CPU. One
// invocation per record.
//
// Words, not structs: records and vertices are read and written as u32 words, so the layouts are
// the Rust ones byte for byte.

// The head (8 words): the opaque layer's records, the translucent layer's, the first vertex of
// each layer's room, a zero the compiler cannot see (`keep`), 3 spare.
const HEAD_WORDS: u32 = 8u;
// `ParticleQuad` (16 words): centre (3), layer, d0 (3), colour, d1 (3), light, UV rect (4).
const QUAD_WORDS: u32 = 16u;
// `mesher::Vertex` (11 words): pos (3), uv (2), layer, color, light, normal (4 x i8), glint_uv (2).
const VERTEX_WORDS: u32 = 11u;

@group(0) @binding(0) var<storage, read> records: array<u32>;
@group(0) @binding(1) var<storage, read_write> out: array<u32>;

fn f(word: u32) -> f32 {
    return bitcast<f32>(word);
}

// Metal compiles shaders with fast math. A value xored with `keep` is opaque to it, so each
// corner is one rounded add or subtract, as on the CPU.
var<private> keep: u32;

fn r3(v: vec3<f32>) -> vec3<f32> {
    return bitcast<vec3<f32>>(bitcast<vec3<u32>>(v) ^ vec3(keep));
}

fn put_vertex(at: u32, pos: vec3<f32>, u: u32, v: u32, layer: u32, color: u32, light: u32) {
    let o = at * VERTEX_WORDS;
    out[o] = bitcast<u32>(pos.x);
    out[o + 1u] = bitcast<u32>(pos.y);
    out[o + 2u] = bitcast<u32>(pos.z);
    out[o + 3u] = u;
    out[o + 4u] = v;
    out[o + 5u] = layer;
    out[o + 6u] = color;
    out[o + 7u] = light;
    // No normal, no glint UV.
    out[o + 8u] = 0u;
    out[o + 9u] = 0u;
    out[o + 10u] = 0u;
}

@compute @workgroup_size(64)
fn expand_quads(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let opaque = records[0];
    let i = id.y * groups.x * 64u + id.x;
    if i >= opaque + records[1] {
        return;
    }
    keep = records[4];
    var at = records[2] + i * 4u;
    if i >= opaque {
        at = records[3] + (i - opaque) * 4u;
    }
    let q = HEAD_WORDS + i * QUAD_WORDS;
    let center = vec3(f(records[q]), f(records[q + 1u]), f(records[q + 2u]));
    let layer = records[q + 3u];
    let d0 = vec3(f(records[q + 4u]), f(records[q + 5u]), f(records[q + 6u]));
    let color = records[q + 7u];
    let d1 = vec3(f(records[q + 8u]), f(records[q + 9u]), f(records[q + 10u]));
    let light = records[q + 11u];
    // The UV rect's bits, copied: u0, v0, u1, v1.
    let u0 = records[q + 12u];
    let v0 = records[q + 13u];
    let u1 = records[q + 14u];
    let v1 = records[q + 15u];
    put_vertex(at, r3(center + d0), u1, v1, layer, color, light);
    put_vertex(at + 1u, r3(center + d1), u1, v0, layer, color, light);
    put_vertex(at + 2u, r3(center - d0), u0, v0, layer, color, light);
    put_vertex(at + 3u, r3(center - d1), u0, v1, layer, color, light);
}
