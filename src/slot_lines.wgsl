// CARVILON CyberDesk - per-slot lines (CD-09): the loading line and the active
// accent, drawn for every slot in one instanced pass.
//
//   * Loading line - a thin brand bar along the slot's TOP edge with a highlight
//     that sweeps left→right while the slot loads; overall alpha is the host-side
//     loading intensity (ramps up on load, fades on done). Same look as the CD-08
//     single loading line, now per slot.
//   * Active accent - a thin brand line along the slot's BOTTOM edge, shown only
//     for the active slot (the one keyboard input and the top bar target).
//
// Both are brand-colored and premultiplied OVER; transparent everywhere else.
//
// Instance layout (per slot):
//   @location(0) rect   = (x, y, w, h) in device px
//   @location(1) params = (loading_intensity, active, accent_th_px, loading_th_px)

struct Globals {
    resolution : vec2<f32>,
    time       : f32,
    _pad       : f32,
    brand      : vec4<f32>,
};
@group(0) @binding(0) var<uniform> G : Globals;

struct VOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) uv     : vec2<f32>,   // 0..1 within the slot rect
    @location(1) size   : vec2<f32>,   // slot size (px)
    @location(2) params : vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi : u32,
    @location(0) rect : vec4<f32>,
    @location(1) params : vec4<f32>,
) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    let px = rect.xy + c * rect.zw;
    let ndc = vec2<f32>(px.x / G.resolution.x * 2.0 - 1.0, 1.0 - px.y / G.resolution.y * 2.0);
    var out : VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = c;
    out.size = rect.zw;
    out.params = params;
    return out;
}

@fragment
fn fs_main(in : VOut) -> @location(0) vec4<f32> {
    let px = in.uv * in.size;                 // px within the slot, (0,0) top-left
    let loading = in.params.x;
    let is_active = in.params.y;
    let acc_th = in.params.z;
    let load_th = in.params.w;

    // Top loading line with a sweeping highlight.
    let top_mask = step(px.y, load_th);
    let sweep = fract(G.time * 0.6);
    let band = exp(-pow((in.uv.x - sweep) / 0.14, 2.0));
    let lum = 0.35 + 0.65 * band;
    let load_a = loading * lum * top_mask;

    // Bottom active accent (only for the active slot).
    let bot_mask = step(in.size.y - acc_th, px.y);
    let acc_a = is_active * bot_mask;

    let a = clamp(load_a + acc_a, 0.0, 1.0);
    return vec4<f32>(G.brand.rgb * a, a);
}
