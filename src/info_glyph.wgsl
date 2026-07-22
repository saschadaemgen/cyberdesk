// CARVILON CyberDesk - update-awareness info glyph (CD-13).
//
// A small status light in the top-right near the gear, composited over everything
// (transparent elsewhere, premultiplied OVER). Idle = a faint outline circle;
// updates available = a filled brand disc with a subtle pulsing halo and a small
// knocked-out count digit. Rendered per the floating law - its own element, no
// strip. This is a status light, not an alarm: the pulse amplitude is modest.

struct InfoUniforms {
    resolution : vec2<f32>,
    center     : vec2<f32>,   // glyph center, device px (origin top-left)
    radius     : f32,         // base radius, device px
    hover      : f32,         // 0 = idle, 1 = hovered
    avail      : f32,         // 0 = no updates, 1 = updates available (eased); `active` is reserved in WGSL
    pulse      : f32,         // 0..1 pulse oscillation (from the host clock)
    count      : f32,         // number of pending items (badge digit, 0 = none)
    // Three scalar pads (NOT a vec3, which std140 aligns to 16 and would inflate
    // the struct past the Rust InfoUniforms's 80 bytes).
    _pad0      : f32,
    _pad1      : f32,
    _pad2      : f32,
    brand      : vec4<f32>,   // rgb glyph color
    base       : vec4<f32>,   // rgb base color (the knocked-out digit)
};

@group(0) @binding(0) var<uniform> U : InfoUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

fn rounded_box_sdf(p : vec2<f32>, half : vec2<f32>, radius : f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

// Soft inside-coverage of a rounded rect (a 7-segment bar) centered at `c`.
fn seg(p : vec2<f32>, c : vec2<f32>, h : vec2<f32>) -> f32 {
    let d = rounded_box_sdf(p - c, h, min(h.x, h.y));
    return 1.0 - smoothstep(-0.7, 0.7, d);
}

// A single 7-segment digit (0..9) centered at the origin, scaled by `s` (px).
fn digit_cov(p : vec2<f32>, s : f32, n : i32) -> f32 {
    let gh = s;              // glyph half-height
    let gw = gh * 0.58;      // glyph half-width
    let th = gh * 0.16;      // segment half-thickness
    let hx = vec2<f32>(gw - th, th);
    let vy = vec2<f32>(th, gh * 0.5 - th);
    let a = seg(p, vec2<f32>(0.0, -(gh - th)), hx);
    let g = seg(p, vec2<f32>(0.0, 0.0), hx);
    let d = seg(p, vec2<f32>(0.0, gh - th), hx);
    let f = seg(p, vec2<f32>(-(gw - th), -gh * 0.5), vy);
    let b = seg(p, vec2<f32>(gw - th, -gh * 0.5), vy);
    let e = seg(p, vec2<f32>(-(gw - th), gh * 0.5), vy);
    let c = seg(p, vec2<f32>(gw - th, gh * 0.5), vy);
    // Per-digit segment masks (a b c d e f g).
    var la = 0.0; var lb = 0.0; var lc = 0.0; var ld = 0.0; var le = 0.0; var lf = 0.0; var lg = 0.0;
    if (n == 0) { la = a; lb = b; lc = c; ld = d; le = e; lf = f; }
    else if (n == 1) { lb = b; lc = c; }
    else if (n == 2) { la = a; lb = b; lg = g; le = e; ld = d; }
    else if (n == 3) { la = a; lb = b; lg = g; lc = c; ld = d; }
    else if (n == 4) { lf = f; lg = g; lb = b; lc = c; }
    else if (n == 5) { la = a; lf = f; lg = g; lc = c; ld = d; }
    else if (n == 6) { la = a; lf = f; lg = g; le = e; lc = c; ld = d; }
    else if (n == 7) { la = a; lb = b; lc = c; }
    else if (n == 8) { la = a; lb = b; lc = c; ld = d; le = e; lf = f; lg = g; }
    else { la = a; lb = b; lc = c; ld = d; lf = f; lg = g; } // 9 (and >9 clamp)
    return max(la, max(lb, max(lc, max(ld, max(le, max(lf, lg))))));
}

@fragment
fn fs_main(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy - U.center;
    let r = length(p);
    let R = U.radius;

    // Idle: a faint outline ring just inside the radius.
    let ring_r = R - 2.0;
    let stroke = max(R * 0.09, 1.4);
    let ring = 1.0 - smoothstep(stroke - 1.0, stroke + 1.0, abs(r - ring_r));

    // Active: a filled disc + a subtle pulsing halo just outside it.
    let disc = 1.0 - smoothstep(R - 1.0, R + 1.0, r);
    let halo = exp(-max(r - R, 0.0) / (R * 0.55)) * (0.18 + 0.30 * U.pulse) * U.avail;

    // Count badge: a knocked-out digit in the base color, centered in the disc.
    let n = clamp(i32(U.count + 0.5), 0, 9);
    var digit = 0.0;
    if (n >= 1) {
        digit = digit_cov(p, R * 0.5, n);
    }

    // Idle vs active blend (active is eased 0..1).
    let idle_alpha = ring * 0.5;
    let active_alpha = clamp(disc + halo, 0.0, 1.0);
    let alpha_core = mix(idle_alpha, active_alpha, U.avail);

    // Hover brightens and adds a soft outer halo on both states.
    let hover_halo = exp(-max(r - R * 1.02, 0.0) / (R * 0.42)) * U.hover * 0.4;
    let alpha = clamp(alpha_core + hover_halo, 0.0, 1.0);

    // Color: brand, with the digit knocked out to the base color inside the disc.
    let bright = mix(0.85, 1.2, U.hover);
    let col = mix(U.brand.rgb * bright, U.base.rgb, digit * U.avail);

    return vec4<f32>(col * alpha, alpha);
}
