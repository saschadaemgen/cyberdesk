//! Theme token indirection.
//!
//! The "cyber" template is embedded data (`theme.toml`). It is the single source
//! for every style value in the shell: colors, radii, periods, amplitudes. Those
//! values are resolved here into wgpu-uniform-ready numbers and into the settings
//! page's CSS custom properties — one truth, two render worlds. Nothing
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

/// Pulse Grid — background v2 (D-0012). Sizes are logical px (scaled by the DPI
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

/// The Pulse Grid "life" layer — travelling pulses and node flares.
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
}

/// Slot engine (CD-09, D-0017). Fixed-width content columns; these dimensions
/// are the single source for the pure layout math in [`crate::slots`] and the
/// host-side view sizing. All sizes are logical px (scaled by the DPI factor).
#[derive(Debug, Clone, Deserialize)]
pub struct Slots {
    pub width: f32,
    pub gutter: f32,
    pub min_margin: f32,
    pub height_frac: f32,
    pub max_count: u32,
    pub active_line: f32,
    pub placeholder_fill: f32,
    pub placeholder_glyph: f32,
    /// CD-11 (D-0020) side-zone widths: `side_zone_width` in the Full state,
    /// `side_rail_width` when the slots demand the width and the sides retreat.
    pub side_zone_width: f32,
    pub side_rail_width: f32,
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
             \x20 --corner-radius: {radius}px;\n\
             \x20 --cmd-input-height: {cmd_input}px;\n\
             \x20 --cmd-row-height: {cmd_row}px;\n\
             \x20 --cmd-list-pad: {cmd_pad}px;\n\
             \x20 --cmd-chip-row: {cmd_chip}px;\n\
             }}\n",
            bg = self.colors.background,
            brand = self.colors.brand,
            panel = self.colors.panel,
            border = self.colors.panel_border,
            text = self.colors.text,
            text_dim = self.colors.text_dim,
            accent = self.colors.accent,
            warn = self.colors.warn,
            radius = self.page.corner_radius,
            cmd_input = self.command.input_height,
            cmd_row = self.command.row_height,
            cmd_pad = self.command.list_pad,
            cmd_chip = self.command.chip_row,
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
