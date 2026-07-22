//! Theme token indirection.
//!
//! The "cyber" template is embedded data (`theme.toml`). It is the single source
//! for every style value in the shell: colors, radii, periods, amplitudes. Those
//! values are resolved here into wgpu-uniform-ready numbers and into the settings
//! page's CSS custom properties - one truth, two render worlds. Nothing
//! style-related is hardcoded in shaders or Rust UI code.

// The full token set is defined here in Stage A and consumed incrementally by
// the Deep Field (Stage B), feathering (Stage C), and settings page (Stage D).
#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    pub name: String,
    pub colors: Colors,
    pub ring: Ring,
    pub page: Page,
    pub deep_field: DeepField,
    pub background: Background,
    pub command: Command,
    pub slots: Slots,
    pub updates: Updates,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Colors {
    pub background: String,
    pub brand: String,
    pub panel: String,
    pub panel_border: String,
    pub text: String,
    pub text_dim: String,
    pub accent: String,
    pub warn: String,
    /// Semantic result colours (CD-45, D-0065): like the Ampel lamps they
    /// carry meaning, so the user accent never recolours them.
    pub error: String,
    pub success: String,
    /// The Ampel lamp colors (CD-30): the graded protection control's
    /// green/yellow/red - semantic traffic-light colors, tokenized like
    /// everything else so a template can restyle them.
    pub ampel_green: String,
    pub ampel_yellow: String,
    pub ampel_red: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ring {
    pub radius: f32,
    pub stroke: f32,
    pub gap_degrees: f32,
    pub rotation_period: f32,
    pub inner_radius: f32,
    pub inner_stroke: f32,
    pub glow: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    pub corner_radius: f32,
    pub feather_width: f32,
    pub feather_exp: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeepField {
    pub breathing_period: f32,
    pub breathing_amplitude: f32,
    pub nebula_a_period: f32,
    pub nebula_b_period: f32,
    pub nebula_amplitude: f32,
    pub dust_amplitude: f32,
    pub dust_twinkle_period: f32,
    pub sweep_period_min: f32,
    pub sweep_period_max: f32,
    pub sweep_amplitude: f32,
}

/// Pulse Grid - background v2 (D-0012). Sizes are logical px (scaled by the DPI
/// factor at bake time); the seed drives deterministic board generation.
#[derive(Debug, Clone, Deserialize)]
pub struct Background {
    pub kind: String,
    pub seed: u64,
    pub lattice_cell: f32,
    pub lattice_dot: f32,
    pub lattice_glow: f32,
    pub trace_density: f32,
    pub trace_seg_min: i32,
    pub trace_seg_max: i32,
    pub trace_len_min: i32,
    pub trace_len_max: i32,
    pub trace_width: f32,
    pub trace_glow: f32,
    pub diagonal_chance: f32,
    pub long_run_chance: f32,
    pub pad_radius: f32,
    pub pad_thickness: f32,
    pub pad_glow: f32,
    pub solder_radius: f32,
    pub solder_glow: f32,
    pub bus_count: i32,
    pub bus_width: f32,
    pub bus_glow: f32,
    pub zone_shadow: f32,
    pub zone_feather: f32,
    pub glow_default: f32,
    // Depth layers (CD-06): mid/far derive from the near values above.
    pub mid_cell_scale: f32,
    pub far_cell_scale: f32,
    pub mid_count_scale: f32,
    pub far_count_scale: f32,
    pub mid_width_scale: f32,
    pub far_width_scale: f32,
    pub mid_bright: f32,
    pub far_bright: f32,
    // Component vocabulary (CD-06).
    pub chip_density: f32,
    pub chip_min_cells: i32,
    pub chip_max_cells: i32,
    pub chip_pin_glow: f32,
    pub chip_four_chance: f32,
    pub hub_density: f32,
    pub hub_attach_chance: f32,
    pub via_density: f32,
    pub via_min: i32,
    pub via_max: i32,
    pub pulse: PulseTokens,
}

/// The Pulse Grid "life" layer - travelling pulses and node flares.
#[derive(Debug, Clone, Deserialize)]
pub struct PulseTokens {
    pub count: i32,
    pub count_ref_width: f32,
    pub speed_min: f32,
    pub speed_max: f32,
    pub head_radius: f32,
    pub trail_steps: i32,
    pub trail_spacing: f32,
    pub head_glow: f32,
    pub trail_glow: f32,
    pub bus_speed_scale: f32,
    pub bus_size_scale: f32,
    // Per-depth-layer pulse scales (CD-06): near is the base (1.0).
    pub mid_count_scale: f32,
    pub far_count_scale: f32,
    pub mid_bright: f32,
    pub far_bright: f32,
    pub mid_speed_scale: f32,
    pub far_speed_scale: f32,
    pub mid_size_scale: f32,
    pub far_size_scale: f32,
    pub flare_interval_min: f32,
    pub flare_interval_max: f32,
    pub flare_max_radius: f32,
    pub flare_thickness: f32,
    pub flare_life: f32,
    pub flare_glow: f32,
}

/// Command palette (CD-07, D-0014). These dimensions are shared between the
/// page CSS (via [`Theme::to_css_vars`]) and the host-side view sizing.
#[derive(Debug, Clone, Deserialize)]
pub struct Command {
    pub input_height: f32,
    pub row_height: f32,
    pub list_pad: f32,
    pub max_results: i32,
    /// Favorites chip row height (CD-08 top bar). Shared with the page CSS.
    pub chip_row: f32,
    /// Floating command sets (CD-12, D-0021) - shared host<->page geometry.
    pub band_height: f32,
    pub launcher_top: f32,
    pub ensemble_top: f32,
    pub capsule_height: f32,
    pub orb_size: f32,
    pub tile_size: f32,
    pub tile_gap: f32,
}

/// Slot engine (CD-09, D-0017). Fixed-width content columns; these dimensions
/// are the single source for the pure layout math in [`crate::slots`] and the
/// host-side view sizing. All sizes are logical px (scaled by the DPI factor).
#[derive(Debug, Clone, Deserialize)]
pub struct Slots {
    pub width: f32,
    pub gutter: f32,
    pub min_margin: f32,
    /// Explicit vertical margins around the surf zone (CD-30 Task A): the slots
    /// span `zone_top .. window_height - zone_bottom`. Replaces the old centered
    /// `height_frac` (whose symmetric 15% margins were dead space).
    pub zone_top: f32,
    pub zone_bottom: f32,
    /// The per-column compression floor (CD-30): when the frame would overflow
    /// (e.g. the 2×-wide terminal), columns squeeze down to this - never close.
    pub slot_min_width: f32,
    pub max_count: u32,
    pub active_line: f32,
    pub placeholder_fill: f32,
    pub placeholder_glyph: f32,
    /// The product slot maximum (D-0022): the frame holds at most this many
    /// columns at any resolution. Capacity / unit math clamps against it. Must be
    /// `<= slots::MAX_SLOTS` (the compile-time per-view array ceiling).
    pub slot_max: u32,
    /// CD-11 (D-0020), revised D-0022: the widths of the **left** (Spine) zone,
    /// the flexible one - `side_zone_width` in the Full state, `side_rail_width`
    /// when the slots demand the width and it retreats to a rail.
    pub side_zone_width: f32,
    pub side_rail_width: f32,
    /// The **right** Multifunctional (MF) zone width steps (D-0022 permanent -
    /// never rails; CD-31/D-0048 discrete sizing): identical for every tab,
    /// stable, and reduced only when the window is too small to hold the step
    /// alongside the nominal columns - large → medium → small, never fluid.
    pub mf_zone_large: f32,
    pub mf_zone_medium: f32,
    pub mf_zone_small: f32,
}

/// Update awareness (CD-13, D-0023). The `feed_url` is the host's ONE allowlisted
/// outbound endpoint (the pinned CARVILON manifest); the rest are the info glyph's
/// token-driven appearance. `feed_url` is public product configuration, not a
/// secret. The `CYBERDESK_UPDATE_FEED` env var overrides `feed_url` for testing.
#[derive(Debug, Clone, Deserialize)]
pub struct Updates {
    pub feed_url: String,
    pub check_interval_hours: u32,
    /// Info glyph radius (logical px, DPI-scaled) - a small status light near the gear.
    pub glyph_radius: f32,
    /// Seconds per pulse cycle when updates are available (modest amplitude).
    pub pulse_period: f32,
}

impl Background {
    /// True when the template selects the Pulse Grid (Cyber default); false
    /// routes the render loop to the Deep Field (Calm variant).
    pub fn is_pulse_grid(&self) -> bool {
        self.kind == "pulse_grid"
    }
}

impl Theme {
    /// Load the embedded "cyber" template.
    pub fn load() -> Self {
        toml::from_str(include_str!("theme.toml")).expect("theme.toml is invalid")
    }

    /// Angular speed of the ring gap, radians per second.
    pub fn ring_rotation_speed(&self) -> f32 {
        std::f32::consts::TAU / self.ring.rotation_period.max(0.001)
    }

    /// CSS custom properties for the settings page, generated from the same
    /// tokens the wgpu side uses.
    pub fn to_css_vars(&self) -> String {
        format!(
            ":root {{\n\
             \x20 --bg: {bg};\n\
             \x20 --brand: {brand};\n\
             \x20 --panel: {panel};\n\
             \x20 --panel-border: {border};\n\
             \x20 --text: {text};\n\
             \x20 --text-dim: {text_dim};\n\
             \x20 --accent: {accent};\n\
             \x20 --warn: {warn};\n\
             \x20 --error: {error};\n\
             \x20 --success: {success};\n\
             \x20 --ampel-green: {ampel_g};\n\
             \x20 --ampel-yellow: {ampel_y};\n\
             \x20 --ampel-red: {ampel_r};\n\
             \x20 --corner-radius: {radius}px;\n\
             \x20 --cmd-input-height: {cmd_input}px;\n\
             \x20 --cmd-row-height: {cmd_row}px;\n\
             \x20 --cmd-list-pad: {cmd_pad}px;\n\
             \x20 --cmd-chip-row: {cmd_chip}px;\n\
             \x20 --cmd-band-height: {cmd_band}px;\n\
             \x20 --cmd-launcher-top: {cmd_ltop}px;\n\
             \x20 --cmd-ensemble-top: {cmd_etop}px;\n\
             \x20 --cmd-capsule-height: {cmd_caph}px;\n\
             \x20 --cmd-orb-size: {cmd_orb}px;\n\
             \x20 --cmd-tile-size: {cmd_tile}px;\n\
             \x20 --cmd-tile-gap: {cmd_tgap}px;\n\
             }}\n",
            bg = self.colors.background,
            brand = self.colors.brand,
            panel = self.colors.panel,
            border = self.colors.panel_border,
            text = self.colors.text,
            text_dim = self.colors.text_dim,
            accent = self.colors.accent,
            warn = self.colors.warn,
            error = self.colors.error,
            success = self.colors.success,
            ampel_g = self.colors.ampel_green,
            ampel_y = self.colors.ampel_yellow,
            ampel_r = self.colors.ampel_red,
            radius = self.page.corner_radius,
            cmd_input = self.command.input_height,
            cmd_row = self.command.row_height,
            cmd_pad = self.command.list_pad,
            cmd_chip = self.command.chip_row,
            cmd_band = self.command.band_height,
            cmd_ltop = self.command.launcher_top,
            cmd_etop = self.command.ensemble_top,
            cmd_caph = self.command.capsule_height,
            cmd_orb = self.command.orb_size,
            cmd_tile = self.command.tile_size,
            cmd_tgap = self.command.tile_gap,
        )
    }
}

/// Parse `#RRGGBB` into linear-order sRGB components (0..1). We render to a
/// non-sRGB target, so these values are written to the framebuffer as-is.
pub fn hex3(s: &str) -> [f32; 3] {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).unwrap_or(0);
    [
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
    ]
}

impl Colors {
    pub fn background_rgb(&self) -> [f32; 3] {
        hex3(&self.background)
    }
    pub fn brand_rgb(&self) -> [f32; 3] {
        hex3(&self.brand)
    }
    pub fn accent_rgb(&self) -> [f32; 3] {
        hex3(&self.accent)
    }
}
