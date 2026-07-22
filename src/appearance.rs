//! Appearance: the accent colour and the template model (CD-45, D-0065).
//!
//! Two ideas live here, and one rule holds both together.
//!
//! ## Templates are data
//!
//! A template is a complete named token set: which background effect runs,
//! what the default accent is, and what the template's own options default
//! to. [`TEMPLATES`] is a table, not a match arm chain, so adding a template
//! later is a row plus its assets, never a refactor. Template 1 (Cyber) ships
//! as the default with the Pulse Grid; Calm rides the Deep Field background
//! that has existed as a token since D-0012.
//!
//! ## The accent has exactly one source
//!
//! The resolved accent is computed HERE, once, from the store (the user's
//! choice) falling back to the template default. Both consumers read that one
//! value: [`Resolved::css_vars`] feeds the `cyberdesk://` pages and
//! [`Resolved::accent_rgb`] feeds the wgpu uniforms for the background and
//! glow. A page and the shader therefore cannot disagree, because there is no
//! second place to disagree from.
//!
//! ## The rule: semantic colours are not accent colours
//!
//! The Ampel green/yellow/red, the Red bunker glow, and warning/error/success
//! carry MEANING. If an accent could recolour them, a protection indicator
//! would become ambiguous and the standing rule that a status display must
//! never lie would break. So the palette is split in the type system, not by
//! convention: [`SEMANTIC_VARS`] lists the CSS custom properties that are
//! owned by the semantic palette, [`accent_vars`] can only ever produce
//! accent-family properties, and `no_semantic_var_is_accent_themed` fails the
//! build's test run if the two sets ever intersect. A future template can
//! restyle its own surface freely; it cannot reach the status palette.

use crate::theme::Theme;

/// A curated accent preset. These are chosen to sit well on the dark surface
/// (bright enough to carry a glow, saturated enough to read as deliberate),
/// not sampled from a rainbow. The user may also define a custom colour.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub hex: &'static str,
}

/// The preset row. CARVILON blue is first and is the product default.
pub const PRESETS: &[Preset] = &[
    Preset { id: "carvilon", label: "CARVILON blue", hex: "#009FE3" },
    Preset { id: "ice", label: "Ice", hex: "#4FC3F7" },
    Preset { id: "teal", label: "Teal", hex: "#00C2A8" },
    Preset { id: "mint", label: "Mint", hex: "#3DDC97" },
    Preset { id: "violet", label: "Violet", hex: "#9B7BFF" },
    Preset { id: "magenta", label: "Magenta", hex: "#E86AC8" },
    Preset { id: "amber", label: "Amber", hex: "#F0A93B" },
    Preset { id: "copper", label: "Copper", hex: "#E8794B" },
    Preset { id: "steel", label: "Steel", hex: "#8FA8BF" },
];

/// Which background effect a template runs. The renderer already carries both
/// paths (D-0012); the template picks one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    PulseGrid,
    DeepField,
}

impl Effect {
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::PulseGrid => "pulse_grid",
            Effect::DeepField => "deep_field",
        }
    }
}

/// One template: a complete token set, expressed as data.
pub struct Template {
    pub id: &'static str,
    pub label: &'static str,
    /// One honest sentence for the picker.
    pub note: &'static str,
    pub effect: Effect,
    /// The accent this template comes up with when the user has not chosen one.
    pub default_accent: &'static str,
    /// Default background-effect intensity (0..200, percent of the template's
    /// own baseline).
    pub default_intensity: u8,
    /// Default glow strength (0..200, percent).
    pub default_glow: u8,
    /// Does the template animate by default?
    pub default_motion: bool,
}

/// The shipped templates. A new one is a row here plus its assets.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "cyber",
        label: "Template 1 (Cyber)",
        note: "The Pulse Grid circuit board, alive and glowing. The CyberDesk default.",
        effect: Effect::PulseGrid,
        default_accent: "#009FE3",
        default_intensity: 100,
        default_glow: 115,
        default_motion: true,
    },
    Template {
        id: "calm",
        label: "Template 2 (Calm)",
        note: "The Deep Field: a quiet, slow-drifting depth field instead of the circuit board.",
        effect: Effect::DeepField,
        default_accent: "#4FC3F7",
        default_intensity: 100,
        default_glow: 100,
        default_motion: true,
    },
];

pub const DEFAULT_TEMPLATE: &str = "cyber";

pub fn template(id: &str) -> &'static Template {
    TEMPLATES
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| &TEMPLATES[0])
}

/// The CSS custom properties owned by the SEMANTIC palette. They come from
/// the theme tokens and are never derived from the accent, whatever the user
/// picks and whatever template is active (CD-45 Task C).
pub const SEMANTIC_VARS: &[&str] = &[
    "--warn",
    "--error",
    "--success",
    "--ampel-green",
    "--ampel-yellow",
    "--ampel-red",
];

/// The fully resolved appearance: what the user chose, with template
/// defaults filled in. Built once per read from the settings store.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub template_id: String,
    pub accent: String,
    pub intensity: u8,
    pub glow: u8,
    pub motion: bool,
}

impl Resolved {
    /// The template record behind this state.
    pub fn template(&self) -> &'static Template {
        template(&self.template_id)
    }

    /// The accent as linear-order sRGB components, for the wgpu uniforms.
    /// THE one accent value the shader ever sees.
    pub fn accent_rgb(&self) -> [f32; 3] {
        crate::theme::hex3(&self.accent)
    }

    /// The background effect this template runs.
    pub fn effect(&self) -> Effect {
        self.template().effect
    }

    /// The accent-family CSS properties. Everything here is derived from the
    /// one accent value; nothing semantic can appear (see the test).
    pub fn accent_vars(&self) -> Vec<(String, String)> {
        vec![
            ("--brand".into(), self.accent.clone()),
            ("--accent".into(), self.accent.clone()),
        ]
    }

    /// The complete `:root` block for the `cyberdesk://` pages: the theme's
    /// own tokens with the accent family overridden by the resolved accent.
    /// The semantic tokens pass through from the theme untouched.
    pub fn css_vars(&self, theme: &Theme) -> String {
        let mut css = theme.to_css_vars();
        for (name, value) in self.accent_vars() {
            // The structural guard of Task C, in PRODUCTION code and not only
            // in a test: the accent fan-out cannot write a semantic property.
            // If a future template or a careless edit ever adds one to
            // `accent_vars`, the status colour still survives untouched here,
            // and the test below turns the mistake into a build failure.
            if SEMANTIC_VARS.contains(&name.as_str()) {
                debug_assert!(false, "accent fan-out tried to write {name}");
                continue;
            }
            css = rewrite_var(&css, &name, &value);
        }
        css
    }
}

/// Replace the value of one `--name: value;` declaration inside a `:root`
/// block, leaving every other declaration exactly as it was.
fn rewrite_var(css: &str, name: &str, value: &str) -> String {
    let needle = format!("{name}:");
    css.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            // Match the property NAME exactly: "--brand:" must not match
            // "--brand-dim:" (the prefix check would otherwise fire).
            if trimmed.starts_with(&needle) {
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}{name}: {value};")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Is `hex` a syntactically valid `#RRGGBB` colour? The custom picker's
/// value is user input and reaches both a stylesheet and a shader uniform,
/// so it is validated at the door rather than trusted.
pub fn valid_hex(hex: &str) -> bool {
    let h = hex.as_bytes();
    h.len() == 7 && h[0] == b'#' && h[1..].iter().all(|c| c.is_ascii_hexdigit())
}

/// Normalize a user-supplied accent to `#RRGGBB` uppercase, or None if it is
/// not a colour at all.
pub fn normalize_hex(hex: &str) -> Option<String> {
    let hex = hex.trim();
    if !valid_hex(hex) {
        return None;
    }
    Some(format!("#{}", hex[1..].to_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE structural guarantee of Task C: the accent fan-out can never touch
    /// a semantic property. If someone adds `--warn` to `accent_vars`, or
    /// renames a semantic token into the accent family, this fails.
    #[test]
    fn no_semantic_var_is_accent_themed() {
        for t in TEMPLATES {
            let r = Resolved {
                template_id: t.id.into(),
                accent: "#FF00FF".into(),
                intensity: t.default_intensity,
                glow: t.default_glow,
                motion: t.default_motion,
            };
            for (name, _) in r.accent_vars() {
                assert!(
                    !SEMANTIC_VARS.contains(&name.as_str()),
                    "accent fan-out must never own the semantic property {name}"
                );
            }
        }
    }

    /// A loud accent leaves every semantic colour exactly as the theme
    /// defined it: the Ampel lamps, the warning colour, and with them the Red
    /// bunker signal keep their meaning (CD-45 Task C, acceptance 3).
    #[test]
    fn accent_never_recolours_the_status_palette() {
        let theme = Theme::load();
        let base = theme.to_css_vars();
        let r = Resolved {
            template_id: "cyber".into(),
            accent: "#FF00FF".into(),
            intensity: 100,
            glow: 100,
            motion: true,
        };
        let themed = r.css_vars(&theme);
        for var in SEMANTIC_VARS {
            let before = declared_value(&base, var);
            let after = declared_value(&themed, var);
            assert_eq!(
                before, after,
                "{var} changed with the accent: {before:?} -> {after:?}"
            );
        }
        // And the accent family DID change, so the test is not vacuous.
        assert_eq!(declared_value(&themed, "--brand").as_deref(), Some("#FF00FF"));
        assert_eq!(declared_value(&themed, "--accent").as_deref(), Some("#FF00FF"));
        assert_ne!(declared_value(&base, "--brand"), declared_value(&themed, "--brand"));
    }

    fn declared_value(css: &str, name: &str) -> Option<String> {
        css.lines()
            .find_map(|l| l.trim().strip_prefix(&format!("{name}:")))
            .map(|v| v.trim().trim_end_matches(';').to_string())
    }

    /// Rewriting one property must not touch a property whose name merely
    /// starts with the same characters.
    #[test]
    fn rewrite_var_matches_whole_property_names() {
        let css = ":root {\n  --brand: #111111;\n  --brand-dim: #222222;\n}";
        let out = rewrite_var(css, "--brand", "#ABCDEF");
        assert!(out.contains("--brand: #ABCDEF;"));
        assert!(out.contains("--brand-dim: #222222;"), "neighbour was rewritten: {out}");
    }

    /// The template table is the model: every row is complete and usable, and
    /// the default exists (a picker can never land on nothing).
    #[test]
    fn templates_are_well_formed_data() {
        assert!(TEMPLATES.iter().any(|t| t.id == DEFAULT_TEMPLATE));
        for t in TEMPLATES {
            assert!(!t.label.is_empty() && !t.note.is_empty(), "{} lacks copy", t.id);
            assert!(valid_hex(t.default_accent), "{} has a bad accent", t.id);
            assert!(t.default_intensity <= 200 && t.default_glow <= 200);
        }
        // An unknown id resolves to the default rather than panicking: a
        // store written by a future build must never brick the picker.
        assert_eq!(template("nonexistent").id, TEMPLATES[0].id);
    }

    #[test]
    fn presets_are_valid_and_distinct() {
        for p in PRESETS {
            assert!(valid_hex(p.hex), "{} is not #RRGGBB", p.id);
        }
        for (i, a) in PRESETS.iter().enumerate() {
            for b in &PRESETS[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.hex, b.hex, "{} and {} are the same colour", a.id, b.id);
            }
        }
    }

    #[test]
    fn custom_colours_are_validated_at_the_door() {
        assert_eq!(normalize_hex("#00ff88").as_deref(), Some("#00FF88"));
        assert_eq!(normalize_hex("  #009FE3 ").as_deref(), Some("#009FE3"));
        assert!(normalize_hex("009FE3").is_none(), "missing #");
        assert!(normalize_hex("#00FE3").is_none(), "too short");
        assert!(normalize_hex("#GGGGGG").is_none(), "not hex");
        assert!(normalize_hex("red").is_none());
        assert!(normalize_hex("#009FE3; --warn: red").is_none(), "no injection");
    }
}
