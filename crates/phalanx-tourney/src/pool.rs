#![forbid(unsafe_code)]

//! Cache-line-padded lock-free work-stealing thread pool built over `std::thread::scope`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Cache-line-padded 64-bit atomic counter avoiding false sharing across cores.
#[repr(align(64))]
#[derive(Debug, Default)]
pub struct CachePaddedU64(AtomicU64);

impl CachePaddedU64 {
    /// Creates a new zero-initialized counter.
    #[must_use]
    pub const fn new(val: u64) -> Self {
        Self(AtomicU64::new(val))
    }

    /// Atomically adds `delta`.
    pub fn fetch_add(&self, delta: u64, order: Ordering) -> u64 {
        self.0.fetch_add(delta, order)
    }

    /// Loads current value.
    pub fn load(&self, order: Ordering) -> u64 {
        self.0.load(order)
    }
}

/// Packed `(head: u32, tail: u32)` work-stealing shard cursor on its own cache line.
///
/// The owner thread claims items from `head` (front), while idle worker threads
/// steal items from `tail` (back) via single-word lock-free 64-bit CAS. Since
/// the interval `[head, tail)` shrinks monotonically inside a fixed slice, the
/// deque is inherently immune to ABA hazards with zero `unsafe`.
#[repr(align(64))]
struct WorkShard {
    bounds: AtomicU64,
}

impl WorkShard {
    fn new(start: u32, end: u32) -> Self {
        Self {
            bounds: AtomicU64::new(pack(start, end)),
        }
    }

    fn pop_front(&self) -> Option<usize> {
        let mut cur = self.bounds.load(Ordering::Acquire);
        loop {
            let (head, tail) = unpack(cur);
            if head >= tail {
                return None;
            }
            let next = pack(head + 1, tail);
            match self
                .bounds
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Some(head as usize),
                Err(actual) => cur = actual,
            }
        }
    }

    fn steal_back(&self) -> Option<usize> {
        let mut cur = self.bounds.load(Ordering::Acquire);
        loop {
            let (head, tail) = unpack(cur);
            if head >= tail {
                return None;
            }
            let new_tail = tail - 1;
            let next = pack(head, new_tail);
            match self
                .bounds
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Some(new_tail as usize),
                Err(actual) => cur = actual,
            }
        }
    }
}

const fn pack(head: u32, tail: u32) -> u64 {
    ((head as u64) << 32) | (tail as u64)
}

const fn unpack(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)
}

/// Live atomic telemetry updated by worker threads during tournament execution.
#[derive(Debug, Default)]
pub struct PoolTelemetry {
    /// Completed match count.
    pub matches_done: CachePaddedU64,
    /// Cumulative simulated ticks.
    pub total_ticks: CachePaddedU64,
    /// Cumulative VM instructions executed.
    pub total_vm_instructions: CachePaddedU64,
}

/// Executes `tasks` in parallel across `threads` workers using lock-free work stealing.
///
/// Results are returned in arbitrary execution order paired with each task's index,
/// allowing callers to reduce results deterministically.
pub fn execute_work_stealing<T, R, F>(
    tasks: &[T],
    threads: usize,
    telemetry: &PoolTelemetry,
    worker_fn: F,
) -> Vec<(usize, R)>
where
    T: Sync,
    R: Send,
    F: Fn(usize, &T, &PoolTelemetry) -> R + Sync,
{
    if tasks.is_empty() {
        return Vec::new();
    }
    let worker_count = threads.max(1).min(tasks.len());
    let chunk = tasks.len().div_ceil(worker_count);

    let mut shards = Vec::with_capacity(worker_count);
    for w in 0..worker_count {
        let start = (w * chunk).min(tasks.len()) as u32;
        let end = ((w + 1) * chunk).min(tasks.len()) as u32;
        shards.push(WorkShard::new(start, end));
    }

    let mut per_worker_out: Vec<Vec<(usize, R)>> = Vec::with_capacity(worker_count);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for my_id in 0..worker_count {
            let shards_ref = &shards;
            let f_ref = &worker_fn;
            handles.push(scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    let task_idx = shards_ref[my_id].pop_front().or_else(|| {
                        for off in 1..worker_count {
                            let victim = (my_id + off) % worker_count;
                            if let Some(idx) = shards_ref[victim].steal_back() {
                                return Some(idx);
                            }
                        }
                        None
                    });
                    let Some(idx) = task_idx else {
                        break;
                    };
                    let res = f_ref(idx, &tasks[idx], telemetry);
                    local.push((idx, res));
                }
                local
            }));
        }
        for h in handles {
            per_worker_out.push(h.join().expect("worker thread panicked"));
        }
    });

    per_worker_out.into_iter().flatten().collect()
}
