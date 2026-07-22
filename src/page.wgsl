// CARVILON CyberDesk - surf-zone page compositing.
//
// Draws the CEF off-screen page texture as a quad placed at the surf-zone
// rectangle (given in NDC via the uniform), sampled and composited over the
// shell (background + ring) with a rounded-corner mask. The edge is either
// feathered (a soft SDF-based alpha falloff, default) or the CD-02 hard rounded
// corner, toggled live via the `feather` uniform. The page is a citizen of our
// compositor, not a window on top.

struct PageUniform {
    rect_ndc : vec4<f32>,   // left, top, right, bottom in normalized device coords
    px_size  : vec2<f32>,   // page texture size in device pixels
    corner_radius : f32,    // rounded-corner radius in device pixels
    feather : f32,          // soft edge falloff width in px; 0 = CD-02 hard edge
    feather_exp : f32,      // falloff curve exponent (pow on the coverage t)
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

    // Rounded-corner coverage in the page's local pixel space. `sdf` is < 0
    // inside the rounded box, 0 at the edge, > 0 outside.
    let half = U.px_size * 0.5;
    let p = in.uv * U.px_size - half;
    let sdf = rounded_box_sdf(p, half, U.corner_radius);

    // Edge coverage, toggled live via the uniform (single pipeline, no recompile):
    //   feather > 0 : soft alpha falloff over `feather` px inward from the edge,
    //                 following the rounded contour. CD-06: a narrow band with a
    //                 steep pow curve - the page stays fully opaque except the
    //                 outermost pixels (a light, casual soften), not the wide
    //                 creamy gradient that read as a 3D/vignette curve.
    //   feather = 0 : CD-02 hard rounded edge (1.5px anti-alias straddling it).
    var mask : f32;
    if (U.feather > 0.5) {
        // t: 1 well inside the rounded box (smoothly AA'd), 0 at the edge.
        let t = 1.0 - smoothstep(-U.feather, 0.0, sdf);
        // pow(t, exp<1) lifts the band toward opaque and concentrates the fade
        // into the last pixels; pow keeps the smooth 0 at the very edge (AA).
        mask = pow(t, max(U.feather_exp, 0.01));
    } else {
        mask = 1.0 - smoothstep(-0.75, 0.75, sdf);
    }

    // CEF delivers premultiplied BGRA; scale premultiplied so the OVER blend
    // (One, OneMinusSrcAlpha) reveals the shell through the rounded corners.
    return vec4<f32>(color.rgb * mask, color.a * mask);
}
