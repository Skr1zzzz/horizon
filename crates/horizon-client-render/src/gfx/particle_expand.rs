//! Camera-facing particle quads as records (#10229). The CPU path pushes four entity vertices
//! (176 bytes) for each quad (`particles::emit_quad`), which the entity batch then copies by
//! material and the frame uploads; a [`ParticleQuad`] holds the same quad in 64.
//!
//! With `HORIZON_PARTICLES_GPU=1` ([`wanted`]) the game hands a frame's quads to the renderer as
//! records ([`ParticleQuads`], `Renderer::stage_particle_quads`) wherever nothing reads the CPU
//! entity stream they then stay out of (`Renderer::expands_particles`).
//! `EntityBatch::update_with_particles` puts their vertices where the engine would have pushed
//! them, in the ordinary material and the translucent particle layer's, so every pass draws them
//! as before.
//!
//! Where compute shaders run, [`ParticleExpand`] writes those vertices on the GPU: the batch
//! leaves their room, the frame's upload skips it, and `particle_expand.wgsl` fills it from the
//! records, 64 bytes a quad uploaded instead of 176. Otherwise the batch writes them, one copy
//! of each quad instead of two.

use super::entity_batch::{OIT_ENTITY, ORDINARY, TRANSLUCENT_PARTICLES, material};
use super::upload::Uploads;
use crate::mesher::Vertex;
use glam::Vec3;

/// Workgroups in a row of a dispatch (at most 65535 per dimension), of 64 records each.
const ROW: u32 = 32_768;
/// The words before the records: each layer's records, each layer's first vertex, a zero the
/// shader cannot fold away (`keep`), 3 spare.
const HEAD_WORDS: usize = 8;

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

/// The compute pass that writes the records' vertices into the entity vertex buffer.
pub(super) struct ParticleExpand {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    /// The head and the records, growing to the next power of two.
    records: Option<wgpu::Buffer>,
    /// The bind group and the buffers it binds (buffers compare by identity).
    group: Option<(wgpu::BindGroup, [wgpu::Buffer; 2])>,
}

impl ParticleExpand {
    /// The pass with `HORIZON_PARTICLES_GPU=1` and compute shaders, else `None` (the batch then
    /// writes the records' vertices itself).
    pub(super) fn new(device: &wgpu::Device, compute: bool) -> Option<Self> {
        if !wanted() || !compute {
            return None;
        }
        Self::compile(device)
    }

    fn compile(device: &wgpu::Device) -> Option<Self> {
        if device.limits().max_storage_buffers_per_shader_stage < 2 {
            return None;
        }
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Particle quads"),
            source: wgpu::ShaderSource::Wgsl(include_str!("particle_expand.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Particle quads"),
            layout: None,
            module: &module,
            entry_point: Some("expand_quads"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        let internal = pollster::block_on(internal.pop());
        if let Some(e) = internal.or(pollster::block_on(validation.pop())) {
            log::warn!("Horizon renderer: particle quads expanded on the CPU ({e})");
            return None;
        }
        Some(Self {
            pipeline,
            layout,
            records: None,
            group: None,
        })
    }

    /// Records the frame's expansion: `quads`' opaque records write their vertices into `out`
    /// from vertex `rooms[0]` on, the translucent ones from `rooms[1]` on. Call after the entity
    /// stream's upload, which leaves those rooms out.
    pub(super) fn dispatch(
        &mut self,
        device: &wgpu::Device,
        uploads: &mut Uploads,
        encoder: &mut wgpu::CommandEncoder,
        quads: &ParticleQuads,
        rooms: [u32; 2],
        out: &wgpu::Buffer,
    ) {
        let count = quads.len() as u32;
        if count == 0 {
            return;
        }
        let quad = std::mem::size_of::<ParticleQuad>() as u64;
        let head = (HEAD_WORDS * 4) as u64;
        let bytes = (head + u64::from(count) * quad)
            .max(256)
            .next_power_of_two();
        if self.records.as_ref().is_none_or(|b| b.size() < bytes) {
            self.records = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Particle quads"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(records) = &self.records else {
            return;
        };
        let opaque = quads.opaque.len() as u32;
        let words: [u32; HEAD_WORDS] = [
            opaque,
            quads.translucent.len() as u32,
            rooms[0],
            rooms[1],
            0,
            0,
            0,
            0,
        ];
        uploads.buffer(encoder, records, 0, bytemuck::cast_slice(&words));
        uploads.buffer(encoder, records, head, bytemuck::cast_slice(&quads.opaque));
        uploads.buffer(
            encoder,
            records,
            head + u64::from(opaque) * quad,
            bytemuck::cast_slice(&quads.translucent),
        );
        let key = [records.clone(), out.clone()];
        if self.group.as_ref().is_none_or(|(_, k)| *k != key) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Particle quads"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: key[0].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: key[1].as_entire_binding(),
                    },
                ],
            });
            self.group = Some((group, key));
        }
        let Some((group, _)) = &self.group else {
            return;
        };
        let workgroups = count.div_ceil(64);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Particle quads"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(workgroups.min(ROW), workgroups.div_ceil(ROW), 1);
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

    #[test]
    fn the_shader_validates() {
        let source = include_str!("particle_expand.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    }

    /// The shader writes into each layer's room what `ParticleQuad::vertices` makes on the CPU,
    /// to the bit (signed zeros, large and tiny offsets, more records than a workgroup), and
    /// leaves the vertices around the rooms alone.
    #[test]
    fn the_shader_writes_what_the_cpu_makes() {
        let _gpu = crate::gfx::test_gpu::lock();
        let Some((_adapter, device, queue)) = crate::gfx::test_gpu::optional_device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let Some(mut expand) = ParticleExpand::compile(&device) else {
            eprintln!("no compute shaders: skipped");
            return;
        };
        let quad = |i: u32, layer: u32| ParticleQuad {
            center: [1.5 + i as f32, -0.0, 1.0e-3 * i as f32 - 2.0],
            layer,
            d0: [0.3, -0.7 * i as f32, 1.0e7],
            color: 0x8040_20ff ^ i,
            d1: [-0.1, 0.2, f32::MIN_POSITIVE],
            light: 0xf000 | i,
            uv: [0.25, 0.5 + i as f32 / 64.0, 0.375, 0.625],
        };
        let quads = ParticleQuads {
            opaque: (0..70).map(|i| quad(i, 2 << 16)).collect(),
            translucent: (0..5)
                .map(|i| quad(100 + i, crate::gfx::TRANSLUCENT_PARTICLE_OVERLAY))
                .collect(),
            ..Default::default()
        };
        // Each room between other vertices, which stay as they were.
        let rooms = [7u32, 7 + 70 * 4 + 9];
        let vertices = rooms[1] + 5 * 4 + 3;
        let vertex = std::mem::size_of::<Vertex>();
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (vertices as usize * vertex) as u64,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&out, 0, &vec![0xab; out.size() as usize]);
        let mut uploads = Uploads::new(&device);
        let mut encoder = device.create_command_encoder(&Default::default());
        expand.dispatch(&device, &mut uploads, &mut encoder, &quads, rooms, &out);
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: out.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&out, 0, &read, 0, out.size());
        uploads.finish(&encoder);
        queue.submit([encoder.finish()]);
        read.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = read.slice(..).get_mapped_range().unwrap();
        let mut expected = vec![0xabu8; data.len()];
        for (list, first) in [(&quads.opaque, rooms[0]), (&quads.translucent, rooms[1])] {
            for (i, quad) in list.iter().enumerate() {
                let at = (first as usize + i * 4) * vertex;
                expected[at..at + 4 * vertex]
                    .copy_from_slice(bytemuck::cast_slice::<Vertex, u8>(&quad.vertices()));
            }
        }
        for (i, (got, want)) in data
            .chunks_exact(vertex)
            .zip(expected.chunks_exact(vertex))
            .enumerate()
        {
            assert_eq!(got, want, "vertex {i}");
        }
    }
}
