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
    pub flare_interval_min: f32,
    pub flare_interval_max: f32,
    pub flare_max_radius: f32,
    pub flare_thickness: f32,
    pub flare_life: f32,
    pub flare_glow: f32,
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
