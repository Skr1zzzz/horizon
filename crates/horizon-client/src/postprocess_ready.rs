//! The resident chunks `Game::postprocess_chunks` runs generation's marks on
//! (`LevelChunk.postProcessGeneration`), kept between ticks.
//!
//! A chunk qualifies once it is lit, still holds marks, and its 3×3 columns are
//! resident. Scanning every resident chunk each tick for the few that do was a
//! cost of the tick thread that grew with the world (#11755). This keeps the
//! marked chunks and the ones of them whose neighbours are all in, and reads
//! again only the columns the map was touched at since the last ask
//! (`ChunkMap::touched_since`) and their neighbours.
use crate::{chunk::Chunk, chunk_map::ChunkMap, fast_hash::FxHashSet};

type Key = (i32, i32);

/// Whether `chunk` waits for its marks to run: lit, with marks left.
fn marked(chunk: &Chunk) -> bool {
    chunk.lit && !chunk.postprocess.is_empty()
}

/// The scan's own test of `key`: a marked chunk whose 3×3 columns are all in `chunks`.
fn ready_in(chunks: &ChunkMap, (x, z): Key) -> bool {
    chunks.get(&(x, z)).is_some_and(|c| marked(c))
        && (-1..=1).all(|dz| (-1..=1).all(|dx| chunks.contains_key(&(x + dx, z + dz))))
}

/// One level's chunks waiting for their marks to run, kept per `ChunkMap` it
/// was last asked about: a different map is read whole again.
#[derive(Default)]
pub struct PostprocessReady {
    /// The resident chunks that are lit with marks left, as of `synced`.
    marked: FxHashSet<Key>,
    /// The `marked` ones whose 3×3 columns are all resident (`ChunkMap::surrounded`).
    ready: FxHashSet<Key>,
    /// The `ChunkMap::touch_cursor` both were read at.
    synced: Option<(u64, u64)>,
}

impl PostprocessReady {
    /// The chunks a scan of every chunk of `chunks` picks (lit, with marks
    /// left, their 3×3 columns resident), in the order it picks them: the
    /// map's own.
    pub fn ready(&mut self, chunks: &ChunkMap) -> Vec<Key> {
        self.sync(chunks);
        // Tested again as the scan tests them, which costs little for the few
        // there are and keeps the scan's answer where `surrounded` and the map's
        // keys disagree: chunks moved out for a while without an unload, which
        // `surrounded` still counts.
        let mut ready: Vec<Key> = self
            .ready
            .iter()
            .copied()
            .filter(|&key| ready_in(chunks, key))
            .collect();
        if ready.len() > 1 {
            // In the map's order, as the scan took them: what one chunk's marks
            // update and schedule comes before the next one's.
            let picked: FxHashSet<Key> = ready.into_iter().collect();
            ready = chunks
                .keys()
                .filter(|key| picked.contains(*key))
                .copied()
                .collect();
        }
        ready
    }

    fn sync(&mut self, chunks: &ChunkMap) {
        let now = chunks.touch_cursor();
        if self.synced == Some(now) {
            return;
        }
        let touched: Option<FxHashSet<Key>> = self
            .synced
            .and_then(|at| chunks.touched_since(at))
            .map(|keys| keys.copied().collect());
        match touched {
            Some(keys) => {
                for &key in &keys {
                    if chunks.get(&key).is_some_and(|c| marked(c)) {
                        self.marked.insert(key);
                    } else {
                        self.marked.remove(&key);
                    }
                }
                // A column coming or going changes whether its neighbours are surrounded.
                for &(x, z) in &keys {
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            self.read(chunks, (x + dx, z + dz));
                        }
                    }
                }
            }
            None => {
                self.marked.clear();
                self.ready.clear();
                self.marked.extend(
                    chunks
                        .iter()
                        .filter(|(_, chunk)| marked(chunk))
                        .map(|(key, _)| *key),
                );
                self.ready.extend(
                    self.marked
                        .iter()
                        .copied()
                        .filter(|&key| chunks.surrounded(key)),
                );
            }
        }
        self.synced = Some(now);
    }

    fn read(&mut self, chunks: &ChunkMap, key: Key) {
        if self.marked.contains(&key) && chunks.surrounded(key) {
            self.ready.insert(key);
        } else {
            self.ready.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// What `Game::postprocess_chunks` scanned before the index: every
    /// resident chunk, in the map's order.
    fn scan(chunks: &ChunkMap) -> Vec<Key> {
        chunks
            .iter()
            .filter(|(_, c)| c.lit && !c.postprocess.is_empty())
            .map(|(k, _)| *k)
            .filter(|&(x, z)| {
                (-1..=1).all(|dz| (-1..=1).all(|dx| chunks.contains_key(&(x + dx, z + dz))))
            })
            .collect()
    }

    fn chunk(key: Key, lit: bool, marks: bool) -> Arc<Chunk> {
        let mut chunk = Chunk::empty(key.0, key.1);
        chunk.lit = lit;
        if marks {
            chunk.postprocess.push([key.0 * 16, 0, key.1 * 16]);
        }
        Arc::new(chunk)
    }

    /// Loads, neighbours arriving, chunks lit later, marks run, unloads, a
    /// whole-map write, a cleared and a swapped map: the same chunks as the
    /// scan, in its order, asked at every step.
    #[test]
    fn ready_is_the_scan_of_every_chunk() {
        let mut chunks = ChunkMap::default();
        let mut index = PostprocessReady::default();
        let mut ask = |chunks: &ChunkMap, expected: &[Key]| {
            let ready = index.ready(chunks);
            assert_eq!(ready, scan(chunks));
            let mut sorted = ready.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, expected);
        };
        // A 3×3 of lit marked chunks: only its middle is surrounded.
        for z in -1..=1 {
            for x in -1..=1 {
                chunks.insert((x, z), chunk((x, z), true, true));
            }
        }
        ask(&chunks, &[(0, 0)]);
        // The column east of it arrives, unlit and unmarked: (1, 0) is surrounded now.
        for z in -1..=1 {
            chunks.insert((2, z), chunk((2, z), false, false));
        }
        ask(&chunks, &[(0, 0), (1, 0)]);
        // (2, 0) is lit with marks later, but its own east is missing.
        let c = Arc::make_mut(chunks.get_mut(&(2, 0)).unwrap());
        c.lit = true;
        c.postprocess.push([32, 0, 0]);
        ask(&chunks, &[(0, 0), (1, 0)]);
        // The marks of (0, 0) run (`postprocess_chunks` takes them).
        Arc::make_mut(chunks.get_mut(&(0, 0)).unwrap())
            .postprocess
            .clear();
        ask(&chunks, &[(1, 0)]);
        // An unload takes the readiness of its neighbours with it.
        chunks.remove(&(2, 1));
        ask(&chunks, &[]);
        chunks.insert((2, 1), chunk((2, 1), true, true));
        ask(&chunks, &[(1, 0)]);
        // A chunk unloaded and loaded again unlit is not ready.
        chunks.remove(&(1, 0));
        chunks.insert((1, 0), chunk((1, 0), false, true));
        ask(&chunks, &[]);
        // A write to every chunk at once, after an ask that had read every touch.
        for c in chunks.values_mut() {
            Arc::make_mut(c).lit = true;
        }
        ask(&chunks, &[(1, 0)]);
        // Another level's map in its place, and back.
        let mut other = ChunkMap::default();
        other.insert((5, 5), chunk((5, 5), true, true));
        std::mem::swap(&mut chunks, &mut other);
        ask(&chunks, &[]);
        std::mem::swap(&mut chunks, &mut other);
        ask(&chunks, &[(1, 0)]);
        chunks.clear();
        ask(&chunks, &[]);
    }

    /// Random loads, unloads, lighting and marks run over a small area, asked
    /// at random pauses so touches pile up between asks, then more touches
    /// than the map keeps: the scan's chunks, in its order, every time.
    #[test]
    fn ready_follows_random_changes_as_the_scan_does() {
        let mut seed = 7u64;
        let mut next = move |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let mut chunks = ChunkMap::default();
        let mut index = PostprocessReady::default();
        let mut several = 0;
        for step in 0..20_000 {
            let key = (next(6) as i32 - 3, next(6) as i32 - 3);
            match next(16) {
                0..=5 => {
                    let (lit, marks) = (next(4) != 0, next(3) != 0);
                    chunks.insert(key, chunk(key, lit, marks));
                }
                6 => {
                    chunks.remove(&key);
                }
                7..=9 => {
                    if let Some(c) = chunks.get_mut(&key) {
                        Arc::make_mut(c).lit = true;
                    }
                }
                10 | 11 => {
                    if let Some(c) = chunks.get_mut(&key) {
                        Arc::make_mut(c).postprocess.clear();
                    }
                }
                _ => {}
            }
            if next(3) == 0 {
                let expected = scan(&chunks);
                several += usize::from(expected.len() > 1);
                assert_eq!(index.ready(&chunks), expected, "step {step}");
            }
        }
        assert!(
            several > 100,
            "the run must pick several chunks at once: {several}"
        );
        // More touches than the map keeps between two asks: read everything again.
        for i in 0..10_000 {
            let key = (i % 6 - 3, i % 5 - 3);
            if let Some(c) = chunks.get_mut(&key) {
                Arc::make_mut(c).lit = i % 2 == 0;
            }
        }
        assert_eq!(index.ready(&chunks), scan(&chunks));
    }
}
