//! Loading many small files at once: startup reads tens of thousands of
//! textures and models with a content pack, and one at a time it waits on
//! each open and read in turn.
//!
//! And a frame's long loops ([`extend_frame`]): shared with the helper threads the game hands the
//! frame ([`set_frame_pool`]), which this crate doesn't own, instead of starting threads every
//! frame as [`par_map`] does.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

/// `f` over `items` on the machine's threads, results in `items`' order.
/// Threads take the next item as they finish one, so a run of slow items
/// doesn't hold back the others.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = horizon_threads::worker_count().min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut out: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    out.sort_unstable_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// Runs `run(k)` once for every `k` in `0..parts`, on a pool's threads and the calling one, and
/// returns once every call has.
pub type FrameRun = fn(usize, &(dyn Fn(usize) + Sync));

/// The pool a frame's loops are shared with: how this crate reaches the game's helper threads
/// (`horizon_client::parallel::frame_lanes` and `frame_pool`), which it doesn't own.
#[derive(Clone, Copy)]
pub struct FramePool {
    /// How many threads would take parts now, the calling one among them; 1 when there is no one
    /// to share with, and the loop stays as it is.
    pub lanes: fn() -> usize,
    pub run: FrameRun,
}

static FRAME_POOL: OnceLock<FramePool> = OnceLock::new();

/// Hands the frames their pool, once, as a window's game starts. Until then, and in a process
/// that never does (a test, a tool), every frame loop runs on the calling thread.
pub fn set_frame_pool(pool: FramePool) {
    let _ = FRAME_POOL.set(pool);
}

#[cfg(not(test))]
fn frame_pool() -> Option<FramePool> {
    FRAME_POOL.get().copied()
}

/// In tests, the pool [`with_test_pool`] gave this thread first.
#[cfg(test)]
fn frame_pool() -> Option<FramePool> {
    GIVEN
        .with(std::cell::Cell::get)
        .or_else(|| FRAME_POOL.get().copied())
}

#[cfg(test)]
thread_local! {
    static GIVEN: std::cell::Cell<Option<FramePool>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with `pool` as this thread's frame pool, as if the game had handed it over: how a
/// test takes the shared path without giving every other test the pool too.
#[cfg(test)]
pub(crate) fn with_test_pool<R>(pool: FramePool, f: impl FnOnce() -> R) -> R {
    GIVEN.with(|p| p.set(Some(pool)));
    let result = f();
    GIVEN.with(|p| p.set(None));
    result
}

/// A stand-in for the game's pool in tests: four lanes, whose parts four threads take in turn.
#[cfg(test)]
pub(crate) const FOUR_THREADS: FramePool = FramePool {
    lanes: four_lanes,
    run: test_run,
};

#[cfg(test)]
fn four_lanes() -> usize {
    4
}

/// [`FOUR_THREADS`]' threads.
#[cfg(test)]
pub(crate) fn test_run(parts: usize, run: &(dyn Fn(usize) + Sync)) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= parts {
                        break;
                    }
                    run(k);
                }
            });
        }
    });
}

/// `HORIZON_SERIAL_FRAME=1`: every frame loop stays on the render thread, as before frames shared
/// their work, for an A/B run of the shared path. A value read once.
pub fn serial_frame() -> bool {
    static SERIAL: OnceLock<bool> = OnceLock::new();
    *SERIAL.get_or_init(|| serial_frame_asked(std::env::var("HORIZON_SERIAL_FRAME").ok()))
}

/// Whether `HORIZON_SERIAL_FRAME`'s value asks for the serial path: `1`, `on` or `true`.
fn serial_frame_asked(value: Option<String>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "on" | "true"))
}

/// `f` over `items` in parts, every part's results appended to `out` in the items' order, as
/// `f(items, out)` appends them: on the frame's pool when `share` (the caller's threshold, past
/// which the work outweighs waking the helpers), a pool was handed over and it has more than one
/// lane now, unless `HORIZON_SERIAL_FRAME` says otherwise; else straight into `out` on this
/// thread. `f` gives each item's results from that item alone, in whatever thread and part it
/// runs.
pub fn extend_frame<T: Sync, R: Send>(
    items: &[T],
    share: bool,
    out: &mut Vec<R>,
    f: impl Fn(&[T], &mut Vec<R>) + Sync,
) {
    let pool = frame_pool().filter(|_| share && items.len() >= 2 && !serial_frame());
    match pool.map(|pool| ((pool.lanes)(), pool.run)) {
        Some((lanes, run)) if lanes > 1 => {
            extend_on(items, frame_parts(lanes, items.len()), run, out, f);
        }
        _ => f(items, out),
    }
}

/// How many parts a frame loop of `items` is cut into on `lanes` threads: four for each, so a
/// thread the system is slow to run holds up one small part, and no part is empty.
fn frame_parts(lanes: usize, items: usize) -> usize {
    (lanes * 4).min(items)
}

/// [`extend_frame`]'s shared path: `items` cut into `parts` runs, each run's results kept apart
/// until `run` ran them all, then appended in order.
fn extend_on<T: Sync, R: Send>(
    items: &[T],
    parts: usize,
    run: FrameRun,
    out: &mut Vec<R>,
    f: impl Fn(&[T], &mut Vec<R>) + Sync,
) {
    if items.is_empty() {
        return;
    }
    let size = items.len().div_ceil(parts.max(1));
    let count = items.len().div_ceil(size);
    let slots: Vec<Mutex<Vec<R>>> = (0..count).map(|_| Mutex::new(Vec::new())).collect();
    run(count, &|k: usize| {
        let part = &items[k * size..((k + 1) * size).min(items.len())];
        let mut done = Vec::with_capacity(part.len());
        f(part, &mut done);
        *slots[k].lock().unwrap_or_else(PoisonError::into_inner) = done;
    });
    for slot in slots {
        out.extend(slot.into_inner().unwrap_or_else(PoisonError::into_inner));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FOUR_THREADS, FramePool, extend_frame, extend_on, par_map, serial_frame,
        serial_frame_asked, test_run, with_test_pool,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn results_keep_the_items_order() {
        let items: Vec<u32> = (0..1000).collect();
        assert_eq!(
            par_map(&items, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
        assert!(par_map(&[] as &[u32], |x| *x).is_empty());
    }

    /// On several threads, every item's results come back once, in the items' order, after what
    /// `out` already held, for item counts that don't divide into the parts evenly; an item may
    /// give several results or none.
    #[test]
    fn shared_parts_append_what_one_thread_appends() {
        let f = |part: &[u64], out: &mut Vec<u64>| {
            for &x in part {
                // Spin a little so the threads overlap.
                let mut h = x;
                for _ in 0..200 {
                    h = h.wrapping_mul(6364136223846793005).wrapping_add(1);
                }
                if x % 3 != 0 {
                    out.push(x * 10);
                }
                if x % 5 == 0 {
                    out.push(h);
                }
            }
        };
        for n in [0usize, 1, 2, 7, 33, 1000, 4097] {
            let items: Vec<u64> = (0..n as u64).collect();
            let mut serial = vec![u64::MAX];
            f(&items, &mut serial);
            for parts in [1, 3, 32] {
                let mut shared = vec![u64::MAX];
                extend_on(&items, parts, test_run, &mut shared, f);
                assert_eq!(shared, serial, "{n} items in {parts} parts");
            }
        }
    }

    /// A frame loop is one call over all its items on the calling thread below its caller's
    /// threshold, pool or not, past it while the game handed it no pool or one with a single lane
    /// now; past it with a pool of several lanes it goes in parts on the pool's threads, with the
    /// same results.
    #[test]
    fn only_a_loop_past_its_threshold_with_lanes_to_share_goes_in_parts() {
        fn one_lane() -> usize {
            1
        }
        const ONE_LANE: FramePool = FramePool {
            lanes: one_lane,
            run: test_run,
        };
        let items: Vec<u32> = (0..500).collect();
        let run = |share: bool| {
            let (calls, elsewhere) = (AtomicUsize::new(0), AtomicBool::new(false));
            let here = std::thread::current().id();
            let mut out = Vec::new();
            extend_frame(&items, share, &mut out, |part, out: &mut Vec<u32>| {
                calls.fetch_add(1, Ordering::Relaxed);
                if std::thread::current().id() != here {
                    elsewhere.store(true, Ordering::Relaxed);
                }
                out.extend(part.iter().map(|x| x + 1));
            });
            assert_eq!(out, (1..=500).collect::<Vec<_>>(), "share: {share}");
            (calls.into_inner(), elsewhere.into_inner())
        };
        assert_eq!(run(false), (1, false));
        assert_eq!(run(true), (1, false), "no pool handed over");
        assert_eq!(with_test_pool(FOUR_THREADS, || run(false)), (1, false));
        assert_eq!(with_test_pool(ONE_LANE, || run(true)), (1, false));
        let (calls, elsewhere) = with_test_pool(FOUR_THREADS, || run(true));
        // `HORIZON_SERIAL_FRAME=1` in the test's environment keeps even this one whole.
        assert!(serial_frame() || (calls > 1 && elsewhere), "{calls} parts");
    }

    /// `HORIZON_SERIAL_FRAME` asks for the serial path with `1`, `on` or `true`, nothing else.
    #[test]
    fn the_serial_switch_reads_its_values() {
        for asked in ["1", "on", "TRUE", " true "] {
            assert!(serial_frame_asked(Some(asked.to_owned())), "{asked}");
        }
        for not in ["0", "off", "", "yes"] {
            assert!(!serial_frame_asked(Some(not.to_owned())), "{not}");
        }
        assert!(!serial_frame_asked(None));
    }
}
