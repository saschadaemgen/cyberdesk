//! Pulse Grid background — CPU model (CD-05, D-0012).
//!
//! This module is deliberately GPU-free: it owns the deterministic board
//! *generation* and the geometry the renderer turns into GPU work. A seeded
//! PRNG produces the same circuit board on every launch (acceptance: restart →
//! identical layout), so the board feels like YOUR board rather than random
//! noise per boot.
//!
//! What it produces:
//!   * [`SpriteInstance`]s for the static bake (lattice is a shader pass; traces,
//!     pads, solder dots and bus lines are instanced quads with SDF coverage),
//!   * [`Polyline`]s (points + cumulative arc length) kept on the CPU so the
//!     life layer (Stage B) can follow the traces with point-at-distance.
//!
//! The renderer treats each [`SpriteInstance`] as a single primitive whose
//! `params.x` selects the SDF: line (0), disk (1) or hollow ring (2).

// The polyline / trace data is generated in Stage A and consumed by the life
// layer (pulses + flares) in Stage B; a few helpers land ahead of their use.
#![allow(dead_code)]

use bytemuck::{Pod, Zeroable};

use crate::theme::Background;

/// One primitive for the instanced sprite pipeline (shared by the static bake
/// and the live pulses/flares). Layout mirrors `pulsegrid_sprite.wgsl`.
///
/// * `p0` — line endpoint A, or the centre of a disk/ring.
/// * `p1` — line endpoint B (unused for disk/ring).
/// * `color` — premultiplied-ish glow RGB, `a` an extra brightness multiplier.
/// * `params` — `[kind, half_width_or_thickness, radius, aa]`.
///   `kind`: 0 = line, 1 = filled disk, 2 = hollow ring.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SpriteInstance {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

pub const KIND_LINE: f32 = 0.0;
pub const KIND_DISK: f32 = 1.0;
pub const KIND_RING: f32 = 2.0;

impl SpriteInstance {
    pub fn line(a: [f32; 2], b: [f32; 2], half_width: f32, aa: f32, color: [f32; 4]) -> Self {
        Self {
            p0: a,
            p1: b,
            color,
            params: [KIND_LINE, half_width, 0.0, aa],
        }
    }
    pub fn disk(center: [f32; 2], radius: f32, aa: f32, color: [f32; 4]) -> Self {
        Self {
            p0: center,
            p1: center,
            color,
            params: [KIND_DISK, 0.0, radius, aa],
        }
    }
    pub fn ring(center: [f32; 2], radius: f32, half_thickness: f32, aa: f32, color: [f32; 4]) -> Self {
        Self {
            p0: center,
            p1: center,
            color,
            params: [KIND_RING, half_thickness, radius, aa],
        }
    }
}

/// A splitmix64 PRNG — tiny, dependency-free, deterministic. Seeded from the
/// `background.seed` token so the board is identical across launches.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f32 in [0, 1).
    pub fn unit(&mut self) -> f32 {
        // 24 bits of mantissa precision.
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }

    /// Uniform integer in [a, b] (inclusive).
    pub fn range_i(&mut self, a: i32, b: i32) -> i32 {
        if b <= a {
            return a;
        }
        let span = (b - a + 1) as u64;
        a + (self.next_u64() % span) as i32
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    /// Index in [0, n).
    pub fn index(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
}

/// A trace/bus path in physical pixels, with cumulative arc length for
/// point-at-distance queries (used by the life layer).
#[derive(Clone)]
pub struct Polyline {
    pub pts: Vec<[f32; 2]>,
    pub cum: Vec<f32>,
    pub total: f32,
}

impl Polyline {
    fn from_points(pts: Vec<[f32; 2]>) -> Self {
        let mut cum = Vec::with_capacity(pts.len());
        let mut total = 0.0;
        for (i, p) in pts.iter().enumerate() {
            if i > 0 {
                let a = pts[i - 1];
                total += ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
            }
            cum.push(total);
        }
        Self { pts, cum, total }
    }

    /// Position at arc-distance `d` (clamped to the polyline extent).
    pub fn point_at(&self, d: f32) -> [f32; 2] {
        if self.pts.is_empty() {
            return [0.0, 0.0];
        }
        if self.pts.len() == 1 || d <= 0.0 {
            return self.pts[0];
        }
        if d >= self.total {
            return *self.pts.last().unwrap();
        }
        // Linear scan over segments (traces are short: 3–7 segments).
        let mut i = 1;
        while i < self.cum.len() && self.cum[i] < d {
            i += 1;
        }
        let seg_start = self.cum[i - 1];
        let seg_len = (self.cum[i] - seg_start).max(1e-4);
        let t = ((d - seg_start) / seg_len).clamp(0.0, 1.0);
        let a = self.pts[i - 1];
        let b = self.pts[i];
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }
}

/// The generated board: static primitives to bake, plus polylines kept on the
/// CPU for the life layer.
pub struct Board {
    pub prims: Vec<SpriteInstance>,
    pub traces: Vec<Polyline>,
    pub buses: Vec<Polyline>,
    pub pads: Vec<[f32; 2]>,
}

/// The 8 lattice step directions (E, NE, N, NW, W, SW, S, SE). Even indices are
/// axis-aligned (90° routing), odd indices diagonal (45° routing).
const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

fn mix_white(c: [f32; 3], t: f32) -> [f32; 3] {
    [
        c[0] * (1.0 - t) + t,
        c[1] * (1.0 - t) + t,
        c[2] * (1.0 - t) + t,
    ]
}

fn scaled_color(c: [f32; 3], glow: f32) -> [f32; 4] {
    [c[0] * glow, c[1] * glow, c[2] * glow, 1.0]
}

/// Generate the board for a `width × height` physical frame at DPI `scale`.
/// Deterministic in `(cfg.seed, width, height, scale)`.
pub fn generate(width: u32, height: u32, scale: f32, cfg: &Background, brand: [f32; 3]) -> Board {
    let mut rng = Rng::new(cfg.seed);
    let (w, h) = (width as f32, height as f32);
    let cell = (cfg.lattice_cell * scale).max(4.0);
    let aa = 0.9;

    let cols = (w / cell).floor() as i32;
    let rows = (h / cell).floor() as i32;

    let mut prims: Vec<SpriteInstance> = Vec::new();
    let mut traces: Vec<Polyline> = Vec::new();
    let mut buses: Vec<Polyline> = Vec::new();
    let mut pads: Vec<[f32; 2]> = Vec::new();

    // Precomputed colors (brand family, so the one-color-world holds).
    let trace_col = scaled_color(brand, cfg.trace_glow);
    let bus_col = scaled_color(brand, cfg.bus_glow);
    let pad_col = scaled_color(mix_white(brand, 0.35), cfg.pad_glow);
    let solder_col = scaled_color(mix_white(brand, 0.2), cfg.solder_glow);

    let trace_hw = (cfg.trace_width * scale * 0.5).max(0.5);
    let bus_hw = (cfg.bus_width * scale * 0.5).max(0.6);
    let pad_r = cfg.pad_radius * scale;
    let pad_t = cfg.pad_thickness * scale;
    let solder_r = cfg.solder_radius * scale;

    let node_px = |c: i32, r: i32| -> [f32; 2] { [c as f32 * cell, r as f32 * cell] };

    // --- Routed traces -------------------------------------------------------
    // Trace count scales with area (~30 at 1080p → density per megapixel).
    let mpx = (w * h) / 1.0e6;
    let trace_count = (cfg.trace_density * mpx).round().max(1.0) as i32;

    if cols > 3 && rows > 3 {
        for _ in 0..trace_count {
            let segs = rng.range_i(cfg.trace_seg_min, cfg.trace_seg_max);
            let mut c = rng.range_i(1, cols - 1);
            let mut r = rng.range_i(1, rows - 1);
            let mut nodes: Vec<(i32, i32)> = vec![(c, r)];
            let mut prev_dir: i32 = [0, 2, 4, 6][rng.index(4)]; // start on an axis

            for seg in 0..segs {
                // Pick a direction: occasionally a 45° diagonal, else 90°/straight,
                // never an immediate reversal.
                let mut dir;
                let opposite = (prev_dir + 4) % 8;
                loop {
                    dir = if rng.chance(cfg.diagonal_chance) {
                        [1, 3, 5, 7][rng.index(4)]
                    } else {
                        [0, 2, 4, 6][rng.index(4)]
                    };
                    if seg == 0 || dir != opposite {
                        break;
                    }
                }
                let len = rng.range_i(cfg.trace_len_min, cfg.trace_len_max);
                let (dx, dy) = DIRS[dir as usize];
                let nc = (c + dx * len).clamp(1, cols - 1);
                let nr = (r + dy * len).clamp(1, rows - 1);
                if nc == c && nr == r {
                    continue; // clamped to a no-op; try another segment
                }
                c = nc;
                r = nr;
                nodes.push((c, r));
                prev_dir = dir;
            }

            if nodes.len() < 2 {
                continue;
            }

            // Line segments between nodes.
            let pts: Vec<[f32; 2]> = nodes.iter().map(|&(c, r)| node_px(c, r)).collect();
            for w2 in pts.windows(2) {
                prims.push(SpriteInstance::line(w2[0], w2[1], trace_hw, aa, trace_col));
            }
            // Solder dots at interior bends.
            for &p in &pts[1..pts.len() - 1] {
                prims.push(SpriteInstance::disk(p, solder_r, aa, solder_col));
            }
            // Pads (hollow rings) at both endpoints.
            let first = pts[0];
            let last = *pts.last().unwrap();
            prims.push(SpriteInstance::ring(first, pad_r, pad_t, aa, pad_col));
            prims.push(SpriteInstance::ring(last, pad_r, pad_t, aa, pad_col));
            pads.push(first);
            pads.push(last);

            traces.push(Polyline::from_points(pts));
        }
    }

    // --- Bus lines -----------------------------------------------------------
    // Exactly `bus_count` full-width horizontal lines on distinct lattice rows
    // in the central band, ~2× trace width and slightly brighter.
    let mut used_rows: Vec<i32> = Vec::new();
    let band_lo = (rows / 4).max(1);
    let band_hi = (rows * 3 / 4).max(band_lo + 1);
    for _ in 0..cfg.bus_count.max(0) {
        let mut row = rng.range_i(band_lo, band_hi);
        let mut tries = 0;
        while used_rows.contains(&row) && tries < 8 {
            row = rng.range_i(band_lo, band_hi);
            tries += 1;
        }
        used_rows.push(row);
        let y = row as f32 * cell;
        let a = [0.0, y];
        let b = [w, y];
        prims.push(SpriteInstance::line(a, b, bus_hw, aa, bus_col));
        // Pads where the bus meets the frame edges.
        prims.push(SpriteInstance::ring([cell, y], pad_r, pad_t, aa, pad_col));
        prims.push(SpriteInstance::ring([w - cell, y], pad_r, pad_t, aa, pad_col));
        buses.push(Polyline::from_points(vec![a, b]));
    }

    Board {
        prims,
        traces,
        buses,
        pads,
    }
}
