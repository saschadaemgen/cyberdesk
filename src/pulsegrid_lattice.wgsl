// Pulse Grid — micro lattice (CD-05), depth layers (CD-06).
//
// The faint dot grid that is the base weave of the board. Rendered once into the
// static bake target as a single fullscreen pass. Since CD-06 it carries THREE
// depth weaves in one pass (far / mid / near): for each pixel we find the
// nearest node of each lattice and sum their soft dots. Additive, so it sits
// under the traces baked on top of it. Finer, fainter cells recede; the near
// cell is the crisp front weave.

struct Lattice {
    brand      : vec4<f32>,   // dot color (brand family)
    resolution : vec2<f32>,
    aa         : f32,
    _pad0      : f32,
    // One weave per depth: (cell, dot_radius, glow, _). Order is cosmetic
    // (summed): [far, mid, near].
    layers     : array<vec4<f32>, 3>,
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
    var a = 0.0;
    for (var i = 0u; i < 3u; i = i + 1u) {
        let cell = L.layers[i].x;
        let dot_radius = L.layers[i].y;
        let glow = L.layers[i].z;
        let node = round(frag.xy / cell) * cell;
        let d = length(frag.xy - node);
        let cov = 1.0 - smoothstep(dot_radius - L.aa, dot_radius + L.aa, d);
        a = a + cov * glow;
    }
    return vec4<f32>(L.brand.rgb * a, a);
}
