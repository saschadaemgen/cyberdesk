// Pulse Grid — instanced SDF sprite (CD-05).
//
// One pipeline draws every circuit primitive and every live pulse/flare. Each
// instance is a line, a filled disk, or a hollow ring; the vertex shader expands
// a unit quad to the primitive's bounds and the fragment shader computes soft
// SDF coverage. Output is premultiplied and blended additively, so overlapping
// glow accumulates.
//
// The same shader serves two pipelines:
//   * the static bake (target Rgba16Float, globals.glow_intensity = 1),
//   * the live layer (target Bgra8Unorm, globals.glow_intensity = the slider).
// Because coverage is multiplied by glow_intensity here, the bake stores raw
// glow (intensity 1) and the composite re-applies intensity to the baked
// texture — the two paths stay consistent.

struct Globals {
    base           : vec4<f32>,   // background base color (composite only)
    resolution     : vec2<f32>,   // physical px
    glow_intensity : f32,         // 1.0 for the bake, slider value for the life pass
    _pad           : f32,
};

@group(0) @binding(0) var<uniform> G : Globals;

struct VOut {
    @builtin(position) pos    : vec4<f32>,
    @location(0)       local  : vec2<f32>,  // fragment position in physical px
    @location(1)       color  : vec4<f32>,
    @location(2)       params : vec4<f32>,   // kind, half_width/thickness, radius, aa
    @location(3)       p0     : vec2<f32>,
    @location(4)       p1     : vec2<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi : u32,
    @location(0) p0     : vec2<f32>,
    @location(1) p1     : vec2<f32>,
    @location(2) color  : vec4<f32>,
    @location(3) params : vec4<f32>,
) -> VOut {
    // Two-triangle unit quad, corners in [0,1]².
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    let kind = params.x;
    let aa = params.w;

    var world : vec2<f32>;
    if (kind < 0.5) {
        // Line: an oriented quad from p0..p1, expanded by half-width + aa, with
        // caps extending half-width beyond each endpoint.
        let d = p1 - p0;
        let len = length(d);
        let dir = select(vec2<f32>(1.0, 0.0), d / len, len > 1e-4);
        let nrm = vec2<f32>(-dir.y, dir.x);
        let hw = params.y + aa;
        let a2 = p0 - dir * hw;
        let b2 = p1 + dir * hw;
        let along = mix(a2, b2, c.x);
        world = along + nrm * (c.y * 2.0 - 1.0) * hw;
    } else {
        // Disk / ring: an axis-aligned quad around the centre.
        let ext = params.z + params.y + aa; // radius + thickness + aa
        world = p0 + (c * 2.0 - 1.0) * ext;
    }

    let ndc = vec2<f32>(
        world.x / G.resolution.x * 2.0 - 1.0,
        1.0 - world.y / G.resolution.y * 2.0,
    );

    var o : VOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.local = world;
    o.color = color;
    o.params = params;
    o.p0 = p0;
    o.p1 = p1;
    return o;
}

fn seg_dist(p : vec2<f32>, a : vec2<f32>, b : vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let t = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * t);
}

@fragment
fn fs_main(in : VOut) -> @location(0) vec4<f32> {
    let kind = in.params.x;
    let aa = in.params.w;
    var cov : f32;
    if (kind < 0.5) {
        let d = seg_dist(in.local, in.p0, in.p1);
        cov = 1.0 - smoothstep(in.params.y - aa, in.params.y + aa, d);
    } else if (kind < 1.5) {
        let d = length(in.local - in.p0);
        cov = 1.0 - smoothstep(in.params.z - aa, in.params.z + aa, d);
    } else {
        let e = abs(length(in.local - in.p0) - in.params.z);
        cov = 1.0 - smoothstep(in.params.y - aa, in.params.y + aa, e);
    }

    let a = cov * in.color.a * G.glow_intensity;
    return vec4<f32>(in.color.rgb * a, a);
}
