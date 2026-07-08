// Pulse Grid — composite (CD-05).
//
// The backmost layer of the frame: read the baked static circuit (full-res
// Rgba16Float, raw glow) and lay it over the base color, scaled by the live
// glow-intensity uniform. The zone shadow (Stage C) will dim the glow under
// content here; Stage A composites at full brightness everywhere.

struct Globals {
    base           : vec4<f32>,
    resolution     : vec2<f32>,
    glow_intensity : f32,
    _pad           : f32,
};

@group(0) @binding(0) var<uniform> G : Globals;
@group(0) @binding(1) var bake : texture_2d<f32>;

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
    let col = G.base.rgb + glow * G.glow_intensity;
    return vec4<f32>(col, 1.0);
}
