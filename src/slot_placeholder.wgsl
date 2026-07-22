// CARVILON CyberDesk - lazy-slot placeholder (CD-09).
//
// A slot with no browser yet (added via Ctrl+T, awaiting its first navigation)
// draws this instead of a page: a rounded fill slightly lifted above the base
// color, with the slot's index as a faint 7-segment glyph in the center. Purely
// shell-side (no CEF), so a new column shows immediately with no white
// about:blank flash. One instanced draw covers every empty slot.
//
// Instance layout (per placeholder slot):
//   @location(0) rect  = (x, y, w, h) in device px
//   @location(1) fill  = (r, g, b, corner_radius_px)   fill color + rounding
//   @location(2) glyph = (r, g, b, sel)  faint color + sel: -1 MF zone, 0 Spine, 1..4 digit
//   @location(3) dot   = (r, g, b, present)            pending-URL dot (CD-10)

struct Globals {
    resolution : vec2<f32>,
    _pad       : vec2<f32>,
};
@group(0) @binding(0) var<uniform> G : Globals;

struct VOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) local : vec2<f32>,   // px within the rect, origin at its center
    @location(1) half  : vec2<f32>,   // rect half-extents (px)
    @location(2) fill  : vec4<f32>,
    @location(3) glyph : vec4<f32>,
    @location(4) dot   : vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi : u32,
    @location(0) rect : vec4<f32>,
    @location(1) fill : vec4<f32>,
    @location(2) glyph : vec4<f32>,
    @location(3) dot : vec4<f32>,
) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    let px = rect.xy + c * rect.zw;
    let ndc = vec2<f32>(px.x / G.resolution.x * 2.0 - 1.0, 1.0 - px.y / G.resolution.y * 2.0);
    var out : VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.half = rect.zw * 0.5;
    out.local = (c - vec2<f32>(0.5)) * rect.zw;
    out.fill = fill;
    out.glyph = glyph;
    out.dot = dot;
    return out;
}

fn rounded_box_sdf(p : vec2<f32>, half : vec2<f32>, radius : f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

// Soft inside-coverage of a rounded rect centered at `c` with half-extents `h`.
fn seg_cov(p : vec2<f32>, c : vec2<f32>, h : vec2<f32>) -> f32 {
    let d = rounded_box_sdf(p - c, h, min(h.x, h.y));
    return 1.0 - smoothstep(-0.75, 0.75, d);
}

@fragment
fn fs_main(in : VOut) -> @location(0) vec4<f32> {
    let radius = in.fill.a;
    let d = rounded_box_sdf(in.local, in.half, radius);
    let fillmask = 1.0 - smoothstep(-0.9, 0.9, d);
    if (fillmask <= 0.002) {
        discard;
    }

    let p = in.local;      // center origin, +y down
    let dg = in.glyph.a;   // glyph selector: -1 = MF zone, 0 = left Spine zone, 1..4 = slot digit
    var col = in.fill.rgb;

    if (dg < 0.5) {
        // A frame zone (D-0022): a thin inset rounded-rect outline (shared) plus a
        // core glyph - the slot family, differing glyph. The LEFT (Spine) zone
        // shows a diamond; the RIGHT Multifunctional zone shows three stacked
        // bars (a rows / tab-rail glyph). No text; the shell has no font.
        let m = min(in.half.x, in.half.y);
        let inset = m * 0.05;
        let ot = max(m * 0.010, 1.0);   // outline half-thickness
        let od = abs(rounded_box_sdf(p, in.half - vec2<f32>(inset), radius)) - ot;
        let ocov = 1.0 - smoothstep(-0.9, 0.9, od);

        var core_cov = 0.0;
        if (dg < -0.5) {
            // MF zone: three stacked horizontal rounded bars.
            let bw = m * 0.16;              // bar half-width
            let bh = max(m * 0.024, 1.0);   // bar half-height
            let sp = m * 0.13;              // vertical spacing between bar centers
            let b0 = seg_cov(p, vec2<f32>(0.0, -sp), vec2<f32>(bw, bh));
            let b1 = seg_cov(p, vec2<f32>(0.0, 0.0), vec2<f32>(bw, bh));
            let b2 = seg_cov(p, vec2<f32>(0.0, sp), vec2<f32>(bw, bh));
            core_cov = max(b0, max(b1, b2));
        } else {
            // Left Spine zone: a diamond (rotated-square outline).
            let ds = m * 0.13;             // diamond half-size
            let dt = max(ds * 0.16, 1.0);  // diamond outline half-thickness
            let dd = abs(abs(p.x) + abs(p.y) - ds) - dt;
            core_cov = 1.0 - smoothstep(-0.9, 0.9, dd);
        }
        col = col + in.glyph.rgb * max(ocov, core_cov);
        return vec4<f32>(col * fillmask, fillmask);
    }

    // 7-segment index glyph (slots), centered, sized to the slot height.
    let gh = min(in.half.y, in.half.x) * 0.34;   // glyph half-height
    let gw = gh * 0.56;                           // glyph half-width
    let th = gh * 0.12;                           // segment half-thickness

    // Segment centers/half-extents (a top, g mid, d bottom; f/e left, b/c right).
    let hx = vec2<f32>(gw - th, th);
    let vy = vec2<f32>(th, gh * 0.5 - th);
    let A = seg_cov(p, vec2<f32>(0.0, -(gh - th)), hx);
    let Gm = seg_cov(p, vec2<f32>(0.0, 0.0), hx);
    let D = seg_cov(p, vec2<f32>(0.0, gh - th), hx);
    let F = seg_cov(p, vec2<f32>(-(gw - th), -gh * 0.5), vy);
    let B = seg_cov(p, vec2<f32>(gw - th, -gh * 0.5), vy);
    let E = seg_cov(p, vec2<f32>(-(gw - th), gh * 0.5), vy);
    let C = seg_cov(p, vec2<f32>(gw - th, gh * 0.5), vy);

    let is1 = abs(dg - 1.0) < 0.5;
    let is2 = abs(dg - 2.0) < 0.5;
    let is3 = abs(dg - 3.0) < 0.5;
    let is4 = abs(dg - 4.0) < 0.5;

    var cov = 0.0;
    if (is2 || is3) { cov = max(cov, A); }
    if (is1 || is2 || is3 || is4) { cov = max(cov, B); }
    if (is1 || is3 || is4) { cov = max(cov, C); }
    if (is2 || is3) { cov = max(cov, D); }
    if (is2) { cov = max(cov, E); }
    if (is4) { cov = max(cov, F); }
    if (is2 || is3 || is4) { cov = max(cov, Gm); }

    // Fill plus the faint glyph added over it; premultiplied OVER.
    col = col + in.glyph.rgb * cov;

    // Pending-URL dot (CD-10): a small scheme-colored disk above the digit, so a
    // restored-but-unspawned column reads as "a page is waiting here".
    if (in.dot.a > 0.5) {
        let dot_c = vec2<f32>(0.0, -(gh * 1.8));
        let dot_r = max(gh * 0.17, 2.0);
        let dd = length(p - dot_c) - dot_r;
        let dcov = 1.0 - smoothstep(-0.9, 0.9, dd);
        col = mix(col, in.dot.rgb, dcov);
    }

    return vec4<f32>(col * fillmask, fillmask);
}
