//! Sections a client mod has meshed again (`remesh_sections`) because the section geometry it adds there
//! changed: a schematic's ghost blocks moved, turned or got new blocks.
use super::Game;

impl Game<'_> {
    /// Marks every loaded section of this client's world that the block box from `min` to `max` reaches for
    /// meshing again (`World::mark_box_dirty`), as an edit there marks its section: streaming meshes them on its
    /// workers, and each mod's `SectionGeometry` is asked for them again.
    pub(super) fn remesh_sections(&mut self, min: [i32; 3], max: [i32; 3]) {
        self.world.mark_box_dirty(min, max);
    }
}
