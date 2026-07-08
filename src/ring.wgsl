// CARVILON CyberDesk — background + rotating logo ring.
//
// Rendered as a single fullscreen triangle; the fragment shader draws the
// dark background (#04070A) and the CARVILON mark: an open outer arc with a
// slowly rotating gap plus a *hollow* inner ring (NO filled centre dot),
// both in brand blue (#009FE3), with anti-aliasing and a subtle glow.

struct Uniforms {
    resolution : vec2<f32>,
    time       : f32,
    is_srgb    : u32,
};

@group(0) @binding(0) var<uniform> U : Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    // Oversized triangle covering the whole clip space.
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
    let bg   = vec3<f32>( 4.0,   7.0,  10.0) / 255.0;  // #04070A
    let blue = vec3<f32>( 0.0, 159.0, 227.0) / 255.0;  // #009FE3

    let res = U.resolution;
    let m   = min(res.x, res.y);
    // Centred, aspect-correct coordinates; unit length = min viewport side.
    let p  = (frag.xy - 0.5 * res) / m;
    let r  = length(p);
    let px = 1.0 / m;                       // one device pixel in these units

    var col = bg;

    // --- Outer ring: open arc with a slowly rotating gap --------------------
    let r_out  = 0.32;
    let hw_out = 0.010;
    let d_out  = abs(r - r_out) - hw_out;   // signed distance to the stroke

    // Angle (screen Y points down, so negate it for a CCW math orientation).
    let ang        = atan2(-p.y, p.x);      // -pi .. pi
    let gap_center = U.time * 0.28;         // slow rotation
    var da         = ang - gap_center;
    da             = atan2(sin(da), cos(da));           // wrap to -pi .. pi
    let gap_half   = radians(32.0);
    let ang_px     = px / max(r_out, 1e-4);             // angular pixel size
    let arc_mask   = smoothstep(gap_half - ang_px * 1.5, gap_half + ang_px * 1.5, abs(da));

    let cov_out = (1.0 - smoothstep(-px, px, d_out)) * arc_mask;

    // Subtle glow hugging the arc for a "cyber OS" feel (also gapped).
    let glow = exp(-max(abs(r - r_out) - hw_out, 0.0) / (px * 26.0)) * 0.22 * arc_mask;

    // --- Inner hollow ring (a ring, never a filled dot) ---------------------
    let r_in  = 0.058;
    let hw_in = 0.0072;
    let d_in  = abs(r - r_in) - hw_in;
    let cov_in = 1.0 - smoothstep(-px, px, d_in);

    let ink = clamp(max(max(cov_out, cov_in), glow), 0.0, 1.0);
    col = mix(col, blue, ink);

    // Non-sRGB (Bgra8Unorm) render target: write the sRGB brand values directly.
    // (`is_srgb` is retained for the off-screen --capture path.)
    if (U.is_srgb == 1u) {
        col = srgb_to_linear(col);
    }
    return vec4<f32>(col, 1.0);
}
