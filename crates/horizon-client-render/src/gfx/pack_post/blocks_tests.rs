//! A frame's uniform blocks go up in one write a buffer (`blocks`).
use super::blocks::{Blocks, Staged};

/// Places follow one another up to the buffer's end. A frame's writes go up as one range over
/// all of them, with the last bytes of a block written twice, and nothing is left to upload
/// until the next write.
#[test]
fn a_frames_blocks_go_up_in_one_write() {
    let mut staged = Staged::new(1024);
    let places = [256, 256, 512].map(|size| staged.place(size));
    assert_eq!(places, [Some(0), Some(256), Some(512)]);
    assert_eq!(staged.place(16), None, "the buffer is full");
    staged.write(256, &[1; 32]);
    staged.write(0, &[2; 16]);
    staged.write(256, &[3; 32]);
    let (offset, bytes) = staged.take().expect("written");
    assert_eq!(offset, 0);
    assert_eq!(bytes.len(), 256 + 32);
    assert_eq!(bytes[..16], [2; 16]);
    assert_eq!(bytes[16..256], [0; 240], "a place no frame wrote");
    assert_eq!(bytes[256..], [3; 32]);
    assert!(staged.take().is_none(), "nothing written since");
    staged.write(512, &[4; 16]);
    assert_eq!(staged.take().map(|(at, b)| (at, b.len())), Some((512, 16)));
}

/// Every place starts at a multiple of the device's `min_uniform_buffer_offset_alignment` (16 at
/// least, as a block's members are), and holds its whole binding before the next place.
#[test]
fn places_are_aligned_and_hold_their_binding() {
    // 256 bytes, the most a device may ask: 100 and 16 bytes take 256 each, 257 takes 512.
    let mut blocks = Blocks::aligned(256);
    let places = [16, 100, 257, 256].map(|size| blocks.reserve(size));
    assert_eq!(places, [(0, 0), (0, 256), (0, 512), (0, 1024)]);
    // Over a buffer's 64 KiB: a buffer of its own; the next block goes after the first ones.
    assert_eq!(blocks.reserve(70_000), (1, 0));
    assert_eq!(blocks.reserve(16), (0, 1280));
    blocks.write((1, 0), &[1; 70_000]);
    // Below 16 bytes, places stay 16 apart.
    let mut blocks = Blocks::aligned(4);
    let places = [4, 20, 16].map(|size| blocks.reserve(size));
    assert_eq!(places, [(0, 0), (0, 16), (0, 48)]);
}
