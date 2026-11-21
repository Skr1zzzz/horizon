//! A frame's uniform blocks go up in one write a buffer (`blocks`).
use super::blocks::Staged;

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
