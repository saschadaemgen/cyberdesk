// Pulse Grid - composite (CD-05).
//
// The backmost layer of the frame: read the baked static circuit (full-res
// Rgba16Float, raw glow) and lay it over the base color, scaled by the live
// glow-intensity uniform. The zone shadow (Stage C) will dim the glow under
// content here; Stage A composites at full brightness everywhere.

struct Globals {
    base           : vec4<f32>,
    resolution     : vec2<f32>,
    glow_intensity : f32,
    zone_shadow    : f32,
    zone_feather   : f32,
    zone_count     : u32,
    _pad           : vec2<f32>,
    zones          : array<vec4<f32>, 8>,
};

@group(0) @binding(0) var<uniform> G : Globals;
@group(0) @binding(1) var bake : texture_2d<f32>;

fn sd_rect(p : vec2<f32>, center : vec2<f32>, half : vec2<f32>) -> f32 {
    let q = abs(p - center) - half;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

// Background attenuation: 1.0 in the margins, down to zone_shadow under content.
fn zone_atten(px : vec2<f32>) -> f32 {
    var atten = 1.0;
    let half_feather = max(G.zone_feather * 0.5, 1.0);
    for (var i = 0u; i < G.zone_count; i = i + 1u) {
        let z = G.zones[i];
        let d = sd_rect(px, z.xy + z.zw * 0.5, z.zw * 0.5);
        let f = smoothstep(-half_feather, half_feather, d);
        atten = min(atten, mix(G.zone_shadow, 1.0, f));
    }
    return atten;
}

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
    let px = vec2<i32>(i32(frag.x), i32(frag.y));
    let glow = textureLoad(bake, px, 0).rgb;
    let col = G.base.rgb + glow * G.glow_intensity * zone_atten(frag.xy);
    return vec4<f32>(col, 1.0);
}
