//! Pulse Grid background - CPU model (CD-05, D-0012).
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
/// * `p0` - line endpoint A, or the centre of a disk/ring.
/// * `p1` - line endpoint B (unused for disk/ring).
/// * `color` - premultiplied-ish glow RGB, `a` an extra brightness multiplier.
/// * `params` - `[kind, half_width_or_thickness, radius, aa]`.
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

/// A splitmix64 PRNG - tiny, dependency-free, deterministic. Seeded from the
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
        // Linear scan over segments (traces are short: 3-7 segments).
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
///
/// Since CD-06 the board is three depth layers (far → mid → near) baked into one
/// texture. `prims` holds every layer's primitives; `layers` keeps each depth's
/// trace polylines separately so the life layer can give each depth its own
/// pulse count, speed and brightness. `buses` and `pads` (flare anchors) belong
/// to the near layer.
pub struct Board {
    pub prims: Vec<SpriteInstance>,
    /// Trace polylines per depth: `[far, mid, near]`.
    pub layers: [Vec<Polyline>; 3],
    pub buses: Vec<Polyline>,
    pub pads: Vec<[f32; 2]>,
}

/// Depth indices into [`Board::layers`].
pub const LAYER_FAR: usize = 0;
pub const LAYER_MID: usize = 1;
pub const LAYER_NEAR: usize = 2;

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

/// Per-depth generation parameters (CD-06). Near is the base; mid and far scale
/// down in cell size, trace count, line width and brightness. `feature_scale`
/// shrinks pads / solder dots / vias / chip pins on the recede layers.
struct LayerSpec {
    cell: f32,          // lattice/step cell in physical px
    trace_count: i32,
    trace_hw: f32,      // trace half-width in physical px
    feature_scale: f32, // pad/solder/via/pin radius scale
    bright: f32,        // brightness multiplier vs near
    with_chips: bool,   // chip footprints (near + mid only)
}

/// Generate one depth layer's primitives (traces + component vocabulary) into
/// `prims`, its trace polylines into `traces`, and - when `pads_out` is set (the
/// near layer) - endpoint / hub pads into it as flare anchors.
#[allow(clippy::too_many_arguments)]
fn gen_layer(
    rng: &mut Rng,
    spec: &LayerSpec,
    w: f32,
    h: f32,
    cfg: &Background,
    scale: f32,
    brand: [f32; 3],
    prims: &mut Vec<SpriteInstance>,
    traces: &mut Vec<Polyline>,
    mut pads_out: Option<&mut Vec<[f32; 2]>>,
) {
    let aa = 0.9;
    let cell = spec.cell;
    let cols = (w / cell).floor() as i32;
    let rows = (h / cell).floor() as i32;
    if cols <= 3 || rows <= 3 {
        return;
    }
    let fs = spec.feature_scale;
    let b = spec.bright;

    // Colors (brand family, so the one-color-world holds), dimmed by depth.
    let trace_col = scaled_color(brand, cfg.trace_glow * b);
    let pad_col = scaled_color(mix_white(brand, 0.35), cfg.pad_glow * b);
    let solder_col = scaled_color(mix_white(brand, 0.2), cfg.solder_glow * b);
    let hub_col = scaled_color(mix_white(brand, 0.45), cfg.pad_glow * b);
    let chip_pin_col = scaled_color(mix_white(brand, 0.35), cfg.chip_pin_glow * b);
    let via_col = scaled_color(mix_white(brand, 0.2), cfg.solder_glow * b);

    let hw = spec.trace_hw;
    let pad_r = cfg.pad_radius * scale * fs;
    let pad_t = (cfg.pad_thickness * scale * fs).max(0.4);
    let solder_r = cfg.solder_radius * scale * fs;
    let hub_r = pad_r * 1.7;
    let via_r = (solder_r * 0.9).max(0.4);
    let pin_r = (solder_r * 0.85).max(0.4);
    let mpx = (w * h) / 1.0e6;

    let node_px = |c: i32, r: i32| -> [f32; 2] { [c as f32 * cell, r as f32 * cell] };

    // --- Junction hubs (generated first, so traces can route toward them) ----
    let hub_count = (cfg.hub_density * mpx).round().max(0.0) as i32;
    let mut hubs: Vec<(i32, i32)> = Vec::new();
    for _ in 0..hub_count {
        let c = rng.range_i(2, (cols - 2).max(2));
        let r = rng.range_i(2, (rows - 2).max(2));
        hubs.push((c, r));
        let p = node_px(c, r);
        prims.push(SpriteInstance::ring(p, hub_r, pad_t, aa, hub_col));
        if let Some(pads) = pads_out.as_deref_mut() {
            pads.push(p);
        }
    }

    // --- Routed traces -------------------------------------------------------
    for _ in 0..spec.trace_count {
        let segs = rng.range_i(cfg.trace_seg_min, cfg.trace_seg_max);
        // Some traces start on a hub (routing several traces toward one point).
        let (mut c, mut r) = if !hubs.is_empty() && rng.chance(cfg.hub_attach_chance) {
            hubs[rng.index(hubs.len())]
        } else {
            (rng.range_i(1, cols - 1), rng.range_i(1, rows - 1))
        };
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
            // Mix short zigzag runs with occasional long straight runs (breaks
            // the every-region-looks-the-same predictability, esp. on far).
            let len = if rng.chance(cfg.long_run_chance) {
                rng.range_i(cfg.trace_len_max, cfg.trace_len_max * 3)
            } else {
                rng.range_i(cfg.trace_len_min, cfg.trace_len_max)
            };
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

        let pts: Vec<[f32; 2]> = nodes.iter().map(|&(c, r)| node_px(c, r)).collect();
        for w2 in pts.windows(2) {
            prims.push(SpriteInstance::line(w2[0], w2[1], hw, aa, trace_col));
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
        if let Some(pads) = pads_out.as_deref_mut() {
            pads.push(first);
            pads.push(last);
        }
        traces.push(Polyline::from_points(pts));
    }

    // --- Chip footprints (near + mid): outline rectangle + pin-pad rows -------
    if spec.with_chips {
        let chip_count = (cfg.chip_density * mpx).round().max(0.0) as i32;
        for _ in 0..chip_count {
            let cw = rng.range_i(cfg.chip_min_cells, cfg.chip_max_cells);
            let chh = rng.range_i(cfg.chip_min_cells - 1, cfg.chip_max_cells - 1).max(1);
            if cols - cw - 2 < 2 || rows - chh - 2 < 2 {
                continue;
            }
            let c0 = rng.range_i(1, cols - cw - 1);
            let r0 = rng.range_i(1, rows - chh - 1);
            let x0 = c0 as f32 * cell;
            let y0 = r0 as f32 * cell;
            let x1 = (c0 + cw) as f32 * cell;
            let y1 = (r0 + chh) as f32 * cell;
            // Body outline (4 edges).
            prims.push(SpriteInstance::line([x0, y0], [x1, y0], hw, aa, trace_col));
            prims.push(SpriteInstance::line([x1, y0], [x1, y1], hw, aa, trace_col));
            prims.push(SpriteInstance::line([x1, y1], [x0, y1], hw, aa, trace_col));
            prims.push(SpriteInstance::line([x0, y1], [x0, y0], hw, aa, trace_col));
            // Pin-pad rows along the top and bottom edges (interior columns).
            for ci in 1..cw {
                let px = (c0 + ci) as f32 * cell;
                prims.push(SpriteInstance::disk([px, y0], pin_r, aa, chip_pin_col));
                prims.push(SpriteInstance::disk([px, y1], pin_r, aa, chip_pin_col));
            }
            // Some chips carry pins on all four edges.
            if rng.chance(cfg.chip_four_chance) {
                for ri in 1..chh {
                    let py = (r0 + ri) as f32 * cell;
                    prims.push(SpriteInstance::disk([x0, py], pin_r, aa, chip_pin_col));
                    prims.push(SpriteInstance::disk([x1, py], pin_r, aa, chip_pin_col));
                }
            }
        }
    }

    // --- Via clusters (all layers): small scatters of filled dots ------------
    let via_count = (cfg.via_density * mpx).round().max(0.0) as i32;
    let spread = cell * 1.1;
    for _ in 0..via_count {
        let cx = rng.range(cell, (w - cell).max(cell + 1.0));
        let cy = rng.range(cell, (h - cell).max(cell + 1.0));
        let n = rng.range_i(cfg.via_min, cfg.via_max);
        for _ in 0..n {
            let p = [cx + rng.range(-spread, spread), cy + rng.range(-spread, spread)];
            prims.push(SpriteInstance::disk(p, via_r, aa, via_col));
        }
    }
}

/// Generate the board for a `width × height` physical frame at DPI `scale`.
/// Deterministic in `(cfg.seed, width, height, scale)`.
///
/// Three depth layers (far → mid → near) are generated, each from its own seed
/// derived from `cfg.seed`, and their primitives baked into one texture. The two
/// bus lines and the flare-anchor pads belong to the near layer.
pub fn generate(width: u32, height: u32, scale: f32, cfg: &Background, brand: [f32; 3]) -> Board {
    let (w, h) = (width as f32, height as f32);

    // Per-layer seeds derived deterministically from the master seed, so all
    // three depths stay identical across launches (the determinism contract).
    let mut master = Rng::new(cfg.seed);
    let far_seed = master.next_u64();
    let mid_seed = master.next_u64();
    let near_seed = master.next_u64();
    let bus_seed = master.next_u64();

    let mpx = (w * h) / 1.0e6;
    let near_count = (cfg.trace_density * mpx).round().max(1.0);

    let near_cell = (cfg.lattice_cell * scale).max(4.0);
    let mid_cell = (cfg.lattice_cell * scale * cfg.mid_cell_scale).max(3.0);
    let far_cell = (cfg.lattice_cell * scale * cfg.far_cell_scale).max(3.0);

    let near_hw = (cfg.trace_width * scale * 0.5).max(0.5);
    let mid_hw = (cfg.trace_width * scale * cfg.mid_width_scale * 0.5).max(0.4);
    let far_hw = (cfg.trace_width * scale * cfg.far_width_scale * 0.5).max(0.35);

    let far_spec = LayerSpec {
        cell: far_cell,
        trace_count: (near_count * cfg.far_count_scale).round() as i32,
        trace_hw: far_hw,
        feature_scale: cfg.far_width_scale,
        bright: cfg.far_bright,
        with_chips: false,
    };
    let mid_spec = LayerSpec {
        cell: mid_cell,
        trace_count: (near_count * cfg.mid_count_scale).round() as i32,
        trace_hw: mid_hw,
        feature_scale: cfg.mid_width_scale,
        bright: cfg.mid_bright,
        with_chips: true,
    };
    let near_spec = LayerSpec {
        cell: near_cell,
        trace_count: near_count as i32,
        trace_hw: near_hw,
        feature_scale: 1.0,
        bright: 1.0,
        with_chips: true,
    };

    let mut prims: Vec<SpriteInstance> = Vec::new();
    let mut layers: [Vec<Polyline>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut pads: Vec<[f32; 2]> = Vec::new();

    // Far first, near last (dimmest → brightest; additive, so order is cosmetic).
    gen_layer(&mut Rng::new(far_seed), &far_spec, w, h, cfg, scale, brand, &mut prims, &mut layers[LAYER_FAR], None);
    gen_layer(&mut Rng::new(mid_seed), &mid_spec, w, h, cfg, scale, brand, &mut prims, &mut layers[LAYER_MID], None);
    gen_layer(
        &mut Rng::new(near_seed),
        &near_spec,
        w,
        h,
        cfg,
        scale,
        brand,
        &mut prims,
        &mut layers[LAYER_NEAR],
        Some(&mut pads),
    );

    // --- Bus lines (near layer) ----------------------------------------------
    // `bus_count` full-width horizontal lines on distinct lattice rows in the
    // central band, ~2× near trace width and slightly brighter.
    let mut buses: Vec<Polyline> = Vec::new();
    {
        let mut rng = Rng::new(bus_seed);
        let aa = 0.9;
        let bus_col = scaled_color(brand, cfg.bus_glow);
        let pad_col = scaled_color(mix_white(brand, 0.35), cfg.pad_glow);
        let bus_hw = (cfg.bus_width * scale * 0.5).max(0.6);
        let pad_r = cfg.pad_radius * scale;
        let pad_t = cfg.pad_thickness * scale;
        let rows = (h / near_cell).floor() as i32;
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
            let y = row as f32 * near_cell;
            let a = [0.0, y];
            let bb = [w, y];
            prims.push(SpriteInstance::line(a, bb, bus_hw, aa, bus_col));
            prims.push(SpriteInstance::ring([near_cell, y], pad_r, pad_t, aa, pad_col));
            prims.push(SpriteInstance::ring([w - near_cell, y], pad_r, pad_t, aa, pad_col));
            pads.push([near_cell, y]);
            pads.push([w - near_cell, y]);
            buses.push(Polyline::from_points(vec![a, bb]));
        }
    }

    Board { prims, layers, buses, pads }
}

// --- Life layer: travelling pulses and node flares (Stage B) -----------------

/// A light pulse travelling along a trace (or a bus line).
struct Pulse {
    layer: usize, // depth index into `board.layers` (ignored when `is_bus`)
    line: usize,  // index into `board.layers[layer]`, or `board.buses` when `is_bus`
    is_bus: bool,
    dist: f32, // arc distance along the polyline (physical px)
    speed: f32,
}

/// An expanding, fading ring at a pad.
struct Flare {
    center: [f32; 2],
    age: f32, // seconds since spawn
}

/// The animated life of the board. Its randomness (respawns, flare positions)
/// runs off a seeded PRNG, but it is intentionally NOT part of the determinism
/// contract - only the static board layout must match across launches. Positions
/// are produced fresh each frame from the CPU polylines.
pub struct PulseSim {
    pulses: Vec<Pulse>,
    flares: Vec<Flare>,
    rng: Rng,
    flare_timer: f32,
    brand: [f32; 3],
    scale: f32,
    // Per-depth pulse scales `[far, mid, near]` (near is the base = 1.0). Depth
    // in motion: near bright/fast, mid dimmer/slower, far faint/slow.
    layer_speed: [f32; 3],
    layer_bright: [f32; 3],
    layer_size: [f32; 3],
}

impl PulseSim {
    pub fn new(
        board: &Board,
        cfg: &crate::theme::PulseTokens,
        brand: [f32; 3],
        width: f32,
        scale: f32,
        seed: u64,
    ) -> Self {
        // The life PRNG is decorrelated from the board PRNG.
        let mut rng = Rng::new(seed ^ 0x5DEE_CE66_D1B2_A5F3);
        let mut pulses = Vec::new();

        // Per-depth scales, indexed [far, mid, near].
        let layer_speed = [cfg.far_speed_scale, cfg.mid_speed_scale, 1.0];
        let layer_bright = [cfg.far_bright, cfg.mid_bright, 1.0];
        let layer_size = [cfg.far_size_scale, cfg.mid_size_scale, 1.0];

        // Trace pulses per depth: near is the base count (scaled with width),
        // mid fewer, far sparse.
        let base = ((cfg.count as f32) * (width / cfg.count_ref_width.max(1.0)))
            .round()
            .max(1.0);
        let counts = [
            (base * cfg.far_count_scale).round().max(0.0) as usize,
            (base * cfg.mid_count_scale).round().max(0.0) as usize,
            base as usize,
        ];
        for layer in 0..3 {
            let traces = &board.layers[layer];
            if traces.is_empty() {
                continue;
            }
            for _ in 0..counts[layer] {
                let line = rng.index(traces.len());
                let total = traces[line].total.max(1.0);
                pulses.push(Pulse {
                    layer,
                    line,
                    is_bus: false,
                    dist: rng.range(0.0, total),
                    speed: rng.range(cfg.speed_min, cfg.speed_max) * scale * layer_speed[layer],
                });
            }
        }
        // Bus pulses: a couple per bus, slower (near layer).
        for line in 0..board.buses.len() {
            let total = board.buses[line].total.max(1.0);
            for _ in 0..2 {
                pulses.push(Pulse {
                    layer: LAYER_NEAR,
                    line,
                    is_bus: true,
                    dist: rng.range(0.0, total),
                    speed: rng.range(cfg.speed_min, cfg.speed_max) * scale * cfg.bus_speed_scale,
                });
            }
        }

        let flare_timer = rng.range(cfg.flare_interval_min, cfg.flare_interval_max);

        Self {
            pulses,
            flares: Vec::new(),
            rng,
            flare_timer,
            brand,
            scale,
            layer_speed,
            layer_bright,
            layer_size,
        }
    }

    fn line<'a>(&self, board: &'a Board, p: &Pulse) -> &'a Polyline {
        if p.is_bus {
            &board.buses[p.line]
        } else {
            &board.layers[p.layer][p.line]
        }
    }

    fn respawn(
        &mut self,
        board: &Board,
        cfg: &crate::theme::PulseTokens,
        layer: usize,
        is_bus: bool,
    ) -> (usize, f32) {
        if is_bus {
            let line = self.rng.index(board.buses.len());
            let speed = self.rng.range(cfg.speed_min, cfg.speed_max) * self.scale * cfg.bus_speed_scale;
            (line, speed)
        } else {
            let line = self.rng.index(board.layers[layer].len());
            let speed = self.rng.range(cfg.speed_min, cfg.speed_max) * self.scale * self.layer_speed[layer];
            (line, speed)
        }
    }

    /// Advance the simulation by `dt` seconds and emit the sprites for this
    /// frame (pulse heads + fading trails, then flare rings).
    pub fn step(&mut self, board: &Board, cfg: &crate::theme::PulseTokens, dt: f32) -> Vec<SpriteInstance> {
        let aa = 0.9;
        let head_white = mix_white(self.brand, 0.6);
        let mut out: Vec<SpriteInstance> = Vec::with_capacity(self.pulses.len() * (cfg.trail_steps as usize + 1) + 8);

        // Pulses.
        for i in 0..self.pulses.len() {
            let is_bus = self.pulses[i].is_bus;
            let layer = self.pulses[i].layer;
            // Depth feel: bus keeps its own size; otherwise scale head size and
            // brightness by the pulse's depth (near full, far faint/small).
            let (bright, size_scale) = if is_bus {
                (1.0_f32, cfg.bus_size_scale)
            } else {
                (self.layer_bright[layer], self.layer_size[layer])
            };
            let total = self.line(board, &self.pulses[i]).total;

            self.pulses[i].dist += self.pulses[i].speed * dt;
            if self.pulses[i].dist > total {
                let (line, speed) = self.respawn(board, cfg, layer, is_bus);
                self.pulses[i].line = line;
                self.pulses[i].dist = 0.0;
                self.pulses[i].speed = speed;
            }

            let poly = self.line(board, &self.pulses[i]);
            let dist = self.pulses[i].dist;
            let head_r = cfg.head_radius * self.scale * size_scale;
            let head = poly.point_at(dist);
            out.push(SpriteInstance::disk(
                head,
                head_r,
                aa,
                [head_white[0], head_white[1], head_white[2], cfg.head_glow * bright],
            ));

            let spacing = cfg.trail_spacing * self.scale * size_scale;
            for k in 1..=cfg.trail_steps {
                let td = dist - spacing * k as f32;
                if td < 0.0 {
                    break;
                }
                let frac = 1.0 - (k as f32) / (cfg.trail_steps as f32 + 1.0);
                let pos = poly.point_at(td);
                out.push(SpriteInstance::disk(
                    pos,
                    head_r * frac,
                    aa,
                    [
                        self.brand[0] * cfg.trail_glow,
                        self.brand[1] * cfg.trail_glow,
                        self.brand[2] * cfg.trail_glow,
                        frac * bright,
                    ],
                ));
            }
        }

        // Node flares: spawn on a timer, age, and expire.
        self.flare_timer -= dt;
        if self.flare_timer <= 0.0 && !board.pads.is_empty() {
            let idx = self.rng.index(board.pads.len());
            self.flares.push(Flare {
                center: board.pads[idx],
                age: 0.0,
            });
            self.flare_timer = self.rng.range(cfg.flare_interval_min, cfg.flare_interval_max);
        }
        let life = cfg.flare_life.max(0.01);
        let thickness = cfg.flare_thickness * self.scale;
        for f in &mut self.flares {
            f.age += dt;
        }
        self.flares.retain(|f| f.age < life);
        for f in &self.flares {
            let t = f.age / life; // 0 -> 1
            let radius = cfg.flare_max_radius * self.scale * t;
            let alpha = cfg.flare_glow * (1.0 - t);
            out.push(SpriteInstance::ring(
                f.center,
                radius,
                thickness,
                aa,
                [self.brand[0], self.brand[1], self.brand[2], alpha],
            ));
        }

        out
    }
}
