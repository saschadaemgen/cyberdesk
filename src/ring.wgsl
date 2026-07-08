// CARVILON CyberDesk — background + rotating logo ring.
//
// All colors and geometry come from theme tokens (see theme.toml); nothing
// style-related is hardcoded here. Rendered as a fullscreen triangle: dark
// background plus the CARVILON mark (open outer arc with a rotating gap + a
// hollow inner ring), anti-aliased with a subtle glow.

struct Uniforms {
    resolution : vec2<f32>,
    time       : f32,
    is_srgb    : u32,
    bg         : vec4<f32>,  // rgb background
    brand      : vec4<f32>,  // rgb ring color
    geom       : vec4<f32>,  // radius, stroke, gap_half_rad, rotation_speed
    inner      : vec4<f32>,  // inner_radius, inner_stroke, glow, _pad
};

@group(0) @binding(0) var<uniform> U : Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

fn srgb_to_linear(c : vec3<f32>) -> vec3<f32> {
    let cutoff = c <= vec3<f32>(0.04045);
    let low    = c / 12.92;
    let high   = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, cutoff);
}

@fragment
fn fs_main(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let bg    = U.bg.rgb;
    let brand = U.brand.rgb;

    let res = U.resolution;
    let m   = min(res.x, res.y);
    let p   = (frag.xy - 0.5 * res) / m;
    let r   = length(p);
    let px  = 1.0 / m;

    var col = bg;

    // --- Outer ring: open arc with a rotating gap ---------------------------
    let r_out    = U.geom.x;
    let hw_out   = U.geom.y;
    let gap_half = U.geom.z;
    let rot_spd  = U.geom.w;

    let d_out = abs(r - r_out) - hw_out;
    let ang        = atan2(-p.y, p.x);
    let gap_center = U.time * rot_spd;
    var da         = ang - gap_center;
    da             = atan2(sin(da), cos(da));
    let ang_px     = px / max(r_out, 1e-4);
    let arc_mask   = smoothstep(gap_half - ang_px * 1.5, gap_half + ang_px * 1.5, abs(da));
    let cov_out = (1.0 - smoothstep(-px, px, d_out)) * arc_mask;

    let glow = exp(-max(abs(r - r_out) - hw_out, 0.0) / (px * 26.0)) * U.inner.z * arc_mask;

    // --- Inner hollow ring (never a filled dot) -----------------------------
    let r_in   = U.inner.x;
    let hw_in  = U.inner.y;
    let d_in   = abs(r - r_in) - hw_in;
    let cov_in = 1.0 - smoothstep(-px, px, d_in);

    let ink = clamp(max(max(cov_out, cov_in), glow), 0.0, 1.0);

    if (U.is_srgb == 1u) {
        // Off-screen capture: opaque background + ring, sRGB-converted target.
        return vec4<f32>(srgb_to_linear(mix(col, brand, ink)), 1.0);
    }
    // Window: transparent premultiplied ring, composited over the Deep Field.
    return vec4<f32>(brand * ink, ink);
}
