//! Camera-facing particle quads as records (#10229). The CPU path pushes four entity vertices
//! (176 bytes) for each quad (`particles::emit_quad`), which the entity batch then copies by
//! material and the frame uploads; a [`ParticleQuad`] holds the same quad in 64.

use crate::mesher::Vertex;
use glam::Vec3;

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
}
