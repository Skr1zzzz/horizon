//! The passes' uniform blocks (`Pass::block`), side by side in a few uniform buffers. A pass has
//! a place of its own from the first frame it binds, and a frame's blocks reach the GPU in one
//! write a buffer once its passes are recorded. With a buffer of its own, each pass took a write
//! of its own every frame, and wgpu-core 30's `Queue::write_buffer` makes a staging buffer for
//! every call (`StagingBuffer::new`) and records its copy under the queue's and the device's
//! locks: some 25 a frame with Complementary.
//!
//! A queue write lands before the commands of the next submission, whenever it is made while
//! they are recorded, so one write at the end gives every pass the block it last wrote this
//! frame, as its own write did, the second of an entity program bound again for End portal
//! faces included.

/// Bytes of each buffer, unless a block needs more.
const CHUNK: usize = 64 << 10;

/// The buffers, each with its bytes (`Staged`), and the alignment of every place: the device's
/// `min_uniform_buffer_offset_alignment`, at least 16 bytes.
pub(super) struct Blocks {
    chunks: Vec<(wgpu::Buffer, Staged)>,
    align: usize,
}

impl Blocks {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let align = device.limits().min_uniform_buffer_offset_alignment as usize;
        Self {
            chunks: Vec::new(),
            align: align.max(16),
        }
    }

    /// A place of its own for a block bound as `size` bytes: its buffer, and its offset there.
    pub(super) fn place(&mut self, device: &wgpu::Device, size: u64) -> (usize, usize) {
        let size = (size as usize).next_multiple_of(self.align);
        for (k, (_, staged)) in self.chunks.iter_mut().enumerate() {
            if let Some(offset) = staged.place(size) {
                return (k, offset);
            }
        }
        let capacity = size.max(CHUNK);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shader pack uniform blocks"),
            size: capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut staged = Staged::new(capacity);
        let offset = staged
            .place(size)
            .expect("a new buffer holds its first block");
        self.chunks.push((buffer, staged));
        (self.chunks.len() - 1, offset)
    }

    /// `block` into its place, for the frame's [`upload`](Self::upload).
    pub(super) fn write(&mut self, (k, offset): (usize, usize), block: &[u8]) {
        self.chunks[k].1.write(offset, block);
    }

    /// The binding of the `size` bytes at a place.
    pub(super) fn binding(
        &self,
        (k, offset): (usize, usize),
        size: u64,
    ) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.chunks[k].0,
            offset: offset as u64,
            size: wgpu::BufferSize::new(size),
        })
    }

    /// What the frame's passes wrote, into the buffers: one write for each buffer written.
    pub(super) fn upload(&mut self, queue: &wgpu::Queue) {
        for (buffer, staged) in &mut self.chunks {
            if let Some((offset, bytes)) = staged.take() {
                queue.write_buffer(buffer, offset as u64, bytes);
            }
        }
    }
}

/// One buffer's bytes as its places were last written, which it holds once they are uploaded;
/// how much of it is placed; and the part written since the last upload.
pub(super) struct Staged {
    bytes: Vec<u8>,
    used: usize,
    dirty: Option<std::ops::Range<usize>>,
}

impl Staged {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            bytes: vec![0; capacity],
            used: 0,
            dirty: None,
        }
    }

    /// The offset of a place of `size` bytes after those placed before; `None` past the end.
    pub(super) fn place(&mut self, size: usize) -> Option<usize> {
        let offset = self.used;
        if offset + size > self.bytes.len() {
            return None;
        }
        self.used += size;
        Some(offset)
    }

    /// `block` at `offset`, the start of a place it fits in.
    pub(super) fn write(&mut self, offset: usize, block: &[u8]) {
        let end = offset + block.len();
        self.bytes[offset..end].copy_from_slice(block);
        // Copies take whole words; a place is a multiple of 16 bytes.
        let end = end.next_multiple_of(4);
        self.dirty = Some(match self.dirty.take() {
            Some(d) => d.start.min(offset)..d.end.max(end),
            None => offset..end,
        });
    }

    /// What to upload, and its offset: one range over everything written since the last call.
    /// The places between those written hold what they held when last uploaded.
    pub(super) fn take(&mut self) -> Option<(usize, &[u8])> {
        let dirty = self.dirty.take()?;
        Some((dirty.start, &self.bytes[dirty]))
    }
}
