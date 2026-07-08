// CARVILON CyberDesk — surf-zone loading line.
//
// A thin brand-colored bar along the top edge of the surf zone with a gentle
// highlight that sweeps left to right while a page loads. Overall alpha is
// driven by a host-side intensity that ramps up on load and fades on done, so
// the whole line dissolves when the page finishes. Composited premultiplied
// OVER, transparent everywhere else.

struct LoadingUniforms {
    resolution : vec2<f32>,
    zone       : vec4<f32>,   // x, y, w, h of the surf zone (device px)
    time       : f32,
    intensity  : f32,         // 0 = hidden, 1 = fully lit
    thickness  : f32,         // line height in device px
    _pad       : f32,
    brand      : vec4<f32>,
};

@group(0) @binding(0) var<uniform> U : LoadingUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy;
    let zx = U.zone.x;
    let zy = U.zone.y;
    let zw = max(U.zone.z, 1.0);

    // Restrict to a `thickness`-tall strip along the top edge of the zone.
    let in_x = step(zx, p.x) * step(p.x, zx + zw);
    let in_y = step(zy, p.y) * step(p.y, zy + U.thickness);
    let mask = in_x * in_y;

    // A soft highlight sweeping across the width.
    let u = (p.x - zx) / zw;
    let sweep = fract(U.time * 0.6);
    let band = exp(-pow((u - sweep) / 0.14, 2.0));
    let lum = 0.35 + 0.65 * band;

    let a = U.intensity * lum * mask;
    return vec4<f32>(U.brand.rgb * a, a);
}
