// CARVILON CyberDesk — drag overlay (CD-12): the shell-drawn favorite-drag
// ghost, the control-gutter drop zones, and the full-capacity slot highlight.
// One instanced pass of soft-glowing rounded rects (a circle is a rounded rect
// with corner_radius = half). Premultiplied OVER, in the placeholder/lines
// visual family.
//
// Instance layout:
//   @location(0) rect  = (x, y, w, h) device px
//   @location(1) color = (r, g, b, a)
//   @location(2) shape = (corner_radius_px, glow_softness_px, _, _)

struct Globals {
    resolution : vec2<f32>,
    _pad       : vec2<f32>,
};
@group(0) @binding(0) var<uniform> G : Globals;

struct VOut {
    @builtin(position) pos   : vec4<f32>,
    @location(0) local : vec2<f32>,   // px within the (expanded) quad, center origin
    @location(1) half  : vec2<f32>,   // rect half-extents (px, un-expanded)
    @location(2) color : vec4<f32>,
    @location(3) shape : vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi : u32,
    @location(0) rect : vec4<f32>,
    @location(1) color : vec4<f32>,
    @location(2) shape : vec4<f32>,
) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    let soft = shape.y;
    let half = rect.zw * 0.5;
    let center = rect.xy + half;
    // Expand the quad by the glow softness so the halo has room.
    let local = (c - vec2<f32>(0.5)) * (rect.zw + vec2<f32>(soft * 2.0));
    let px = center + local;
    let ndc = vec2<f32>(px.x / G.resolution.x * 2.0 - 1.0, 1.0 - px.y / G.resolution.y * 2.0);
    var out : VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local = local;
    out.half = half;
    out.color = color;
    out.shape = shape;
    return out;
}

fn rounded_box_sdf(p : vec2<f32>, half : vec2<f32>, radius : f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in : VOut) -> @location(0) vec4<f32> {
    let radius = min(in.shape.x, min(in.half.x, in.half.y));
    let soft = max(in.shape.y, 0.75);
    let d = rounded_box_sdf(in.local, in.half, radius);
    // Solid core inside; a soft glow fading over `soft` px outside.
    let mask = 1.0 - smoothstep(0.0, soft, d);
    let a = in.color.a * mask;
    return vec4<f32>(in.color.rgb * a, a);
}
