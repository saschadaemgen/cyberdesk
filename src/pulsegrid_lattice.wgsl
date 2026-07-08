// Pulse Grid — micro lattice (CD-05).
//
// A very faint dot grid, the base weave of the board. Rendered once into the
// static bake target as a single fullscreen pass: for each pixel we find the
// nearest lattice node and draw a soft dot. Additive, so it sits under the
// traces baked on top of it.

struct Lattice {
    brand      : vec4<f32>,   // dot color (brand family)
    resolution : vec2<f32>,
    cell       : f32,         // lattice spacing (physical px)
    dot_radius : f32,
    glow       : f32,         // dot brightness
    aa         : f32,
    _pad       : vec2<f32>,
};

@group(0) @binding(0) var<uniform> L : Lattice;

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
    let node = round(frag.xy / L.cell) * L.cell;
    let d = length(frag.xy - node);
    let cov = 1.0 - smoothstep(L.dot_radius - L.aa, L.dot_radius + L.aa, d);
    let a = cov * L.glow;
    return vec4<f32>(L.brand.rgb * a, a);
}
