// CARVILON CyberDesk — settings gear button.
//
// A small cog drawn in the top-right corner, composited over everything as the
// last pass. Transparent everywhere except the gear and its hover halo, so the
// premultiplied OVER blend leaves the rest of the frame untouched. Color is the
// brand token; the button brightens and grows a soft halo on hover.

struct GearUniforms {
    resolution : vec2<f32>,
    center     : vec2<f32>,   // gear center, device px (origin top-left)
    radius     : f32,         // base radius, device px
    hover      : f32,         // 0 = idle, 1 = hovered
    _pad       : vec2<f32>,
    brand      : vec4<f32>,   // rgb ring/gear color
};

@group(0) @binding(0) var<uniform> U : GearUniforms;

const TAU : f32 = 6.28318530718;

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

// Filled-cog coverage: a body with rounded teeth and a hollow hub.
fn gear_mask(p : vec2<f32>, radius : f32) -> f32 {
    let r = length(p);
    let a = atan2(p.y, p.x);
    let teeth = 8.0;
    // Square-ish teeth modulating the outer radius.
    let t = 0.5 + 0.5 * cos(a * teeth);
    let tooth = smoothstep(0.35, 0.65, t);
    let outer = radius * (0.72 + 0.20 * tooth);
    let body = 1.0 - smoothstep(outer - 1.5, outer + 1.5, r);
    // Hollow hub.
    let hub = radius * 0.30;
    let hole = 1.0 - smoothstep(hub - 1.5, hub + 1.5, r);
    return clamp(body - hole, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy - U.center;
    let r = length(p);

    let gear = gear_mask(p, U.radius);

    // Soft halo just outside the cog, revealed on hover.
    let halo_edge = U.radius * 1.02;
    let halo = exp(-max(r - halo_edge, 0.0) / (U.radius * 0.42)) * U.hover;

    let alpha = clamp(gear + halo * 0.45, 0.0, 1.0);
    let bright = mix(0.72, 1.15, U.hover);
    let col = U.brand.rgb * bright;

    // Premultiplied OVER.
    return vec4<f32>(col * alpha, alpha);
}
