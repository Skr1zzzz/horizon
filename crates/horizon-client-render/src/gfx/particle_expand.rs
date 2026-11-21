//! Camera-facing particle quads as records (#10229). The CPU path pushes four entity vertices
//! (176 bytes) for each quad (`particles::emit_quad`), which the entity batch then copies by
//! material and the frame uploads; a [`ParticleQuad`] holds the same quad in 64.
//!
//! With `HORIZON_PARTICLES_GPU=1` ([`wanted`]) the game hands a frame's quads to the renderer as
//! records ([`ParticleQuads`], `Renderer::stage_particle_quads`) wherever nothing reads the CPU
//! entity stream they then stay out of (`Renderer::expands_particles`).
//! `EntityBatch::update_with_particles` puts their vertices where the engine would have pushed
//! them, in the ordinary material and the translucent particle layer's, so every pass draws them
//! as before: one copy of each quad instead of two.

use super::entity_batch::{OIT_ENTITY, ORDINARY, TRANSLUCENT_PARTICLES, material};
use crate::mesher::Vertex;
use glam::Vec3;

/// `HORIZON_PARTICLES_GPU=1`: a frame's particle quads come to the renderer as records where they
/// may (`Renderer::expands_particles`). Off by default, for A/B runs against the vertices.
pub(super) fn wanted() -> bool {
    static WANTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WANTED.get_or_init(|| {
        matches!(
            std::env::var("HORIZON_PARTICLES_GPU").as_deref(),
            Ok("1" | "on" | "true")
        )
    })
}

/// One camera-facing particle quad (`QuadParticleRenderState.renderRotatedQuad`): its corners are
/// `center + d0`, `center + d1`, `center - d0` and `center - d1`, in that order, at the UV rect's
/// `(u1, v1)`, `(u1, v0)`, `(u0, v0)` and `(u0, v1)`. The particle engine works out `d0` and `d1`
/// (the camera's basis by the quad's size, a roll, a Polytone orientation), so expanding a record
/// only adds and subtracts: one rounding a component, wherever it is done.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleQuad {
    /// Camera-relative.
    pub center: [f32; 3],
    /// `Vertex::layer`: the overlay (none, or a particle layer's) and the HDR tier.
    pub layer: u32,
    pub d0: [f32; 3],
    /// `Vertex::color`.
    pub color: u32,
    pub d1: [f32; 3],
    /// `Vertex::light`.
    pub light: u32,
    /// The sprite's rect in the atlas: u0, v0, u1, v1.
    pub uv: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<ParticleQuad>() == 16 * 4);

impl ParticleQuad {
    /// The quad's four entity vertices.
    #[inline(always)]
    pub fn vertices(&self) -> [Vertex; 4] {
        let center = Vec3::from_array(self.center);
        let d0 = Vec3::from_array(self.d0);
        let d1 = Vec3::from_array(self.d1);
        let [u0, v0, u1, v1] = self.uv;
        let vertex = |pos: Vec3, uv: [f32; 2]| Vertex {
            normal: [0; 4],
            pos: pos.to_array(),
            uv,
            layer: self.layer,
            color: self.color,
            light: self.light,
            glint_uv: [0.0, 0.0],
        };
        [
            vertex(center + d0, [u1, v1]),
            vertex(center + d1, [u1, v0]),
            vertex(center - d0, [u0, v0]),
            vertex(center - d1, [u0, v1]),
        ]
    }
}

/// One frame's particle quads as records, for the renderer to put into the entity stream: those
/// of each material `EntityBatch` sorts them into, each in draw order.
#[derive(Clone, Debug, Default)]
pub struct ParticleQuads {
    /// Where they go in the frame's entity stream (`Frame::entities`): before its vertex `cut`,
    /// where the particle engine would have pushed them.
    pub cut: u32,
    /// How many prepared model copies (`Renderer::stage_prepared_draws`) the frame had staged
    /// when its particles were drawn: a later one at the same cut draws after them.
    pub draws_before: u32,
    /// The ordinary entities' material: the opaque particle layer.
    pub opaque: Vec<ParticleQuad>,
    /// The translucent particle layer (`TRANSLUCENT_PARTICLE_OVERLAY`), drawn after translucent
    /// terrain.
    pub translucent: Vec<ParticleQuad>,
}

impl ParticleQuads {
    /// Empties both lists, keeping their storage.
    pub fn clear(&mut self) {
        self.cut = 0;
        self.draws_before = 0;
        self.opaque.clear();
        self.translucent.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty() && self.translucent.is_empty()
    }

    pub fn len(&self) -> usize {
        self.opaque.len() + self.translucent.len()
    }

    /// The list a quad on `layer` joins, by the material `EntityBatch` gives it; `None` for any
    /// other material, whose quads stay vertices.
    pub(crate) fn list(&mut self, layer: u32) -> Option<&mut Vec<ParticleQuad>> {
        if layer & OIT_ENTITY != 0 {
            return None;
        }
        match material(layer, false) {
            ORDINARY => Some(&mut self.opaque),
            TRANSLUCENT_PARTICLES => Some(&mut self.translucent),
            _ => None,
        }
    }

    /// Adds the opaque layer's room to `ordinary`, the ordinary vertices before each prepared
    /// copy at `cuts` (their `cpu_vertices`, in the order they were staged), for the copies drawn
    /// after these quads: staged past their cut, or at it after them.
    pub(super) fn shift_cuts(&self, cuts: impl Iterator<Item = u32>, ordinary: &mut [u32]) {
        let room = self.opaque.len() as u32 * 4;
        for (index, (ordinary, cut)) in ordinary.iter_mut().zip(cuts).enumerate() {
            if cut > self.cut || (cut == self.cut && index as u32 >= self.draws_before) {
                *ordinary += room;
            }
        }
    }

    /// Pushes every record's vertices onto `out`, the opaque layer's first, and empties the
    /// lists. `EntityBatch` sorts the two layers apart again, so its copy is the one the engine's
    /// own vertices would have made.
    pub(crate) fn expand_into(&mut self, out: &mut Vec<Vertex>) {
        out.reserve(self.len() * 4);
        for quad in self.opaque.iter().chain(&self.translucent) {
            out.extend_from_slice(&quad.vertices());
        }
        self.opaque.clear();
        self.translucent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_expands_to_the_quads_corners_and_sprite_rect() {
        let quad = ParticleQuad {
            center: [1.0, 2.0, -3.0],
            layer: 15 | (2 << 16),
            d0: [0.5, -0.25, 0.0],
            color: 0x80ff_4020,
            d1: [0.5, 0.25, 0.125],
            light: 0xf0_30,
            uv: [0.25, 0.5, 0.375, 0.625],
        };
        let vertices = quad.vertices();
        let corners = vertices.map(|v| v.pos);
        assert_eq!(
            corners,
            [
                [1.5, 1.75, -3.0],
                [1.5, 2.25, -2.875],
                [0.5, 2.25, -3.0],
                [0.5, 1.75, -3.125],
            ]
        );
        let uvs = vertices.map(|v| v.uv);
        assert_eq!(
            uvs,
            [[0.375, 0.625], [0.375, 0.5], [0.25, 0.5], [0.25, 0.625]]
        );
        for v in vertices {
            assert_eq!(
                (v.layer, v.color, v.light, v.normal, v.glint_uv),
                (15 | (2 << 16), 0x80ff_4020, 0xf0_30, [0; 4], [0.0; 2])
            );
        }
    }

    /// Prepared copies keep their place among the ordinary quads around the particles' room: a
    /// copy staged before the particles stays before it, one staged after them comes after its
    /// opaque quads (never the translucent ones), unless it was staged at an earlier cut.
    #[test]
    fn prepared_copies_after_the_particles_count_their_opaque_quads() {
        let quad = <ParticleQuad as bytemuck::Zeroable>::zeroed();
        let quads = ParticleQuads {
            cut: 40,
            draws_before: 2,
            opaque: vec![quad; 3],
            translucent: vec![quad; 5],
        };
        // Two copies staged before the particles (the second at their cut), then three after them:
        // at their cut, past it, and at an earlier cut.
        let cuts = [12, 40, 40, 52, 8];
        let mut ordinary = [4, 20, 20, 28, 4];
        quads.shift_cuts(cuts.into_iter(), &mut ordinary);
        assert_eq!(ordinary, [4, 20, 32, 40, 4]);
    }
}
