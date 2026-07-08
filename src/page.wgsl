// CARVILON CyberDesk — surf-zone page compositing.
//
// Draws the CEF off-screen page texture as a quad placed at the surf-zone
// rectangle (given in NDC via the uniform), sampled and composited over the
// shell (background + ring) with a rounded-corner mask. The page is a citizen
// of our compositor, not a window on top.

struct PageUniform {
    rect_ndc : vec4<f32>,   // left, top, right, bottom in normalized device coords
    px_size  : vec2<f32>,   // page texture size in device pixels
    corner_radius : f32,    // rounded-corner radius in device pixels
    _pad : f32,
};

@group(0) @binding(0) var<uniform> U : PageUniform;
@group(1) @binding(0) var page_tex : texture_2d<f32>;
@group(1) @binding(1) var page_smp : sampler;

struct VOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> VOut {
    let l = U.rect_ndc.x;
    let t = U.rect_ndc.y;
    let r = U.rect_ndc.z;
    let b = U.rect_ndc.w;
    var pos = array<vec2<f32>, 6>(
        vec2<f32>(l, t), vec2<f32>(r, t), vec2<f32>(l, b),
        vec2<f32>(r, t), vec2<f32>(r, b), vec2<f32>(l, b),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    var out : VOut;
    out.pos = vec4<f32>(pos[vi], 0.0, 1.0);
    out.uv = uvs[vi];
    return out;
}

// Signed distance to a rounded box centered at the origin.
fn rounded_box_sdf(p : vec2<f32>, half : vec2<f32>, radius : f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in : VOut) -> @location(0) vec4<f32> {
    let color = textureSample(page_tex, page_smp, in.uv);

    // Rounded-corner coverage in the page's local pixel space.
    let half = U.px_size * 0.5;
    let p = in.uv * U.px_size - half;
    let sdf = rounded_box_sdf(p, half, U.corner_radius);
    let aa = 1.5;
    let mask = 1.0 - smoothstep(0.0, aa, sdf);

    // CEF delivers premultiplied BGRA; scale premultiplied so the OVER blend
    // (One, OneMinusSrcAlpha) reveals the shell through the rounded corners.
    return vec4<f32>(color.rgb * mask, color.a * mask);
}
