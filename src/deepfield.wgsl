// CARVILON CyberDesk — Deep Field background.
//
// A procedural, texture-free living background rendered at half resolution and
// upscaled. Four layers, all token-driven, all within a tight amplitude budget
// (brightness delta ~6-8%, perceived motion < ~20 px/s, no flicker):
//   1. a breathing base glow,
//   2. two slowly drifting nebula gradients,
//   3. sparse drifting dust with gentle twinkle,
//   4. a rare, faint scan sweep crossing the full width.

const TAU : f32 = 6.28318530718;

struct FieldUniforms {
    resolution : vec2<f32>,
    time       : f32,
    _pad       : f32,
    base       : vec4<f32>,  // base background rgb
    brand      : vec4<f32>,  // brand rgb (nebula / glow / sweep tint)
    breathing  : vec4<f32>,  // period, amplitude, _, _
    nebula     : vec4<f32>,  // a_period, b_period, amplitude, _
    dust       : vec4<f32>,  // amplitude, twinkle_period, _, _
    sweep      : vec4<f32>,  // period_min, period_max, amplitude, _
};

@group(0) @binding(0) var<uniform> U : FieldUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

fn hash21(p : vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 345.45));
    q += dot(q, q + 34.345);
    return fract(q.x * q.y);
}

fn hash11(x : f32) -> f32 {
    return fract(sin(x * 91.3458) * 47453.5453);
}

fn value_noise(p : vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p : vec2<f32>) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < 3; i = i + 1) {
        v += amp * value_noise(q);
        q = q * 2.03;
        amp *= 0.5;
    }
    return v;
}

// Sparse twinkling dust (grid-cell star field, no branching).
fn dust_layer(frag : vec2<f32>, t : f32, twinkle_period : f32) -> f32 {
    let cell_size = 96.0;
    let cell = floor(frag / cell_size);
    let h = hash21(cell);
    let has = step(0.86, h);                       // ~14% of cells
    let jitter = vec2<f32>(hash21(cell + 1.7), hash21(cell + 4.3));
    let center = (cell + jitter) * cell_size;
    let d = length(frag - center);
    let star = smoothstep(2.6, 0.0, d);
    let twinkle = 0.55 + 0.45 * sin(TAU * t / twinkle_period + h * 30.0);
    return has * star * twinkle;
}

// Rare scan sweep crossing left to right; interval drifts within [min, max].
fn sweep_band(x : f32, t : f32, p_min : f32, p_max : f32) -> f32 {
    let period = mix(p_min, p_max, 0.5 + 0.5 * sin(t * 0.017 + 1.3));
    let dur = 2.6;                                  // seconds to cross
    let frac = fract(t / period);
    let dur_frac = dur / period;
    let gate = step(frac, dur_frac);
    let progress = frac / max(dur_frac, 1e-4);
    let band = exp(-pow((x - progress) / 0.045, 2.0));
    return gate * band;
}

@fragment
fn fs_main(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let res  = U.resolution;
    let uv   = frag.xy / res;
    let m    = min(res.x, res.y);
    let p    = (frag.xy - 0.5 * res) / m;
    let t    = U.time;
    let base  = U.base.rgb;
    let brand = U.brand.rgb;

    var col = base;

    // 1. breathing central glow
    let breath = 1.0 + U.breathing.y * sin(TAU * t / U.breathing.x);
    let glow = (1.0 - smoothstep(0.0, 0.95, length(p)));
    col += brand * glow * 0.055 * breath;

    // 2. two drifting nebula gradients
    let drift_a = vec2<f32>(t / U.nebula.x, t / (U.nebula.x * 1.3));
    let drift_b = vec2<f32>(-t / U.nebula.y, t / (U.nebula.y * 0.8));
    let neb = fbm(p * 1.6 + drift_a) * 0.6 + fbm(p * 2.3 + drift_b) * 0.4;
    col += brand * (neb - 0.5) * U.nebula.z;

    // 3. sparse twinkling dust (brand-tinted, lifted toward white)
    let dust = dust_layer(frag.xy, t, U.dust.y);
    col += mix(brand, vec3<f32>(1.0), 0.5) * dust * U.dust.x;

    // 4. rare scan sweep
    let sweep = sweep_band(uv.x, t, U.sweep.x, U.sweep.y);
    col += brand * sweep * U.sweep.z;

    return vec4<f32>(max(col, vec3<f32>(0.0)), 1.0);
}
