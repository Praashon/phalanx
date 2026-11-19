//! Zero-allocation uniform spatial hash grid using intrusive flat buckets.

use crate::fixed::Fix;

const EMPTY: u32 = u32::MAX;

/// Pre-allocated spatial hash grid for fast `O(k)` radius and neighbor queries.
///
/// Reuses pre-allocated `cell_heads` and `next` buffers across ticks without
/// any heap allocations during simulation updates.
#[derive(Debug, Clone)]
pub struct SpatialHashGrid {
    cols: usize,
    rows: usize,
    cell_size: Fix,
    cell_heads: Vec<u32>,
    next: Vec<u32>,
}

impl SpatialHashGrid {
    /// Creates a new grid covering `[0, width] x [0, height]` with fixed capacity.
    #[must_use]
    pub fn new(width: Fix, height: Fix, cell_size: Fix, max_entities: usize) -> Self {
        let cell_raw = cell_size.raw().max(1);
        let cols = ((width.raw().max(1) + cell_raw - 1) / cell_raw).max(1) as usize;
        let rows = ((height.raw().max(1) + cell_raw - 1) / cell_raw).max(1) as usize;
        Self {
            cols,
            rows,
            cell_size: Fix::from_raw(cell_raw),
            cell_heads: vec![EMPTY; cols * rows],
            next: vec![EMPTY; max_entities],
        }
    }

    /// Resets all grid buckets in `O(cells)` with zero heap allocations.
    pub fn clear(&mut self) {
        self.cell_heads.fill(EMPTY);
    }

    /// Ensures the intrusive `next` capacity covers `needed` entities.
    pub fn ensure_capacity(&mut self, needed: usize) {
        if needed > self.next.len() {
            self.next.resize(needed, EMPTY);
        }
    }

    /// Inserts entity `id` at world coordinate `(x, y)`.
    pub fn insert(&mut self, id: usize, x: Fix, y: Fix) {
        if id >= self.next.len() {
            self.next.resize(id + 1, EMPTY);
        }
        let cx = self.coord_to_col(x);
        let cy = self.coord_to_row(y);
        let cell = cy * self.cols + cx;
        self.next[id] = self.cell_heads[cell];
        self.cell_heads[cell] = id as u32;
    }

    /// Invokes `visitor(id)` for every entity in cells overlapping the circle `(x, y, radius)`.
    pub fn query_aabb<F: FnMut(usize)>(&self, x: Fix, y: Fix, radius: Fix, mut visitor: F) {
        let min_cx = self.coord_to_col(x - radius);
        let max_cx = self.coord_to_col(x + radius);
        let min_cy = self.coord_to_row(y - radius);
        let max_cy = self.coord_to_row(y + radius);

        for cy in min_cy..=max_cy {
            let row_off = cy * self.cols;
            for cx in min_cx..=max_cx {
                let mut cur = self.cell_heads[row_off + cx];
                while cur != EMPTY {
                    let idx = cur as usize;
                    visitor(idx);
                    cur = self.next[idx];
                }
            }
        }
    }

    fn coord_to_col(&self, x: Fix) -> usize {
        if x.raw() <= 0 {
            return 0;
        }
        let c = (x.raw() / self.cell_size.raw()) as usize;
        c.min(self.cols - 1)
    }

    fn coord_to_row(&self, y: Fix) -> usize {
        if y.raw() <= 0 {
            return 0;
        }
        let r = (y.raw() / self.cell_size.raw()) as usize;
        r.min(self.rows - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn matches_brute_force_oracle() {
        let w = Fix::from_int(100);
        let h = Fix::from_int(60);
        let mut grid = SpatialHashGrid::new(w, h, Fix::from_int(8), 64);
        let mut rng = Rng::derive(777, 1);

        let mut pts = Vec::new();
        for i in 0..50 {
            let x = Fix::from_raw(rng.next_bounded_i32(w.raw()));
            let y = Fix::from_raw(rng.next_bounded_i32(h.raw()));
            pts.push((x, y));
            grid.insert(i, x, y);
        }

        for _ in 0..40 {
            let qx = Fix::from_raw(rng.next_bounded_i32(w.raw()));
            let qy = Fix::from_raw(rng.next_bounded_i32(h.raw()));
            let r = Fix::from_int(12);
            let r2 = i64::from(r.raw()) * i64::from(r.raw());

            let mut expected: Vec<usize> = pts
                .iter()
                .enumerate()
                .filter(|&(_, &(px, py))| Fix::dist_sq_i64(px - qx, py - qy) <= r2)
                .map(|(i, _)| i)
                .collect();
            expected.sort_unstable();

            let mut got = Vec::new();
            grid.query_aabb(qx, qy, r, |id| {
                let (px, py) = pts[id];
                if Fix::dist_sq_i64(px - qx, py - qy) <= r2 {
                    got.push(id);
                }
            });
            got.sort_unstable();
            assert_eq!(got, expected);
        }
    }
}
