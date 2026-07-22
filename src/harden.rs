//! Fingerprinting-hardening configuration model (CD-25, D-0040; CD-29, D-0045).
//!
//! CD-16 (D-0039) ships the hardening MECHANICS (`src/hardening.js`); this module is
//! the CONFIGURATION *over* them: the graded Ampel levels (CD-30: Off / Green /
//! Yellow / Red - Yellow and Red being the former Standard and Strict), the
//! per-vector custom flags, resolution of an effective per-window config, and the
//! weaken/strengthen classification the safety gate keys on. It never introduces a
//! new spoofing vector - the config only ENABLES/DISABLES and tunes the existing
//! coherent vectors, so EC-01 (no OS/UA/platform spoofing, coherence) holds for
//! every reachable configuration.
//!
//! CD-29 completes the vector surface: every measurable fingerprint vector is its
//! own visible, settable flag - the CD-16 six (canvas, WebGL readback, audio,
//! layout/text metrics, device profile, fonts) plus GPU identity (WebGL vendor /
//! renderer strings + WebGPU adapter, split out of `webgl` so readback noise and
//! identity clamp are independently controllable), clock/timing precision, media /
//! codec / voice profile, and math (transcendental-rounding) normalization. CD-32
//! (D-0049) adds the eleventh: window size - the inner-size read cluster reported
//! as the nearest common step of the CD-29 screen ladder. Old persisted CD-25
//! configs deserialize with the new vectors DEFAULTED ON (serde defaults) - an
//! upgrade never silently weakens protection.
//!
//! The [`Config`] is serialized to the exact JSON `hardening.js` reads at its
//! `__CYBERDESK_FP_CONFIG__` placeholder, and the SAME JSON rides the CreateBrowser
//! `extra_info` dictionary that carries a slot's effective config to its render
//! process (per-window; the seed stays session-global). Timezone normalization
//! (`TZ=UTC`, CD-16 `main.rs`) is process-global and is deliberately NOT part of this
//! config - it stays always-on and is surfaced honestly as such.

use serde::{Deserialize, Serialize};

/// A hardening preset level. CD-30 (Task C) turns the presets into the graded
/// **Ampel** (traffic light): `Green` < `Yellow` < `Red` is a strict protection
/// order - moving DOWN it is a weakening (gated), moving UP is immediate.
/// `Yellow` is the former `Standard` (every vector, moderate buckets) and `Red`
/// the former `Strict` (every vector, tight buckets - plus, at the shell level,
/// the CD-30 window-size lock); `Green` is the new everyday floor (the coherent
/// core without the aggressive clock/media/math clamps). The old names parse as
/// aliases so persisted CD-25/CD-29 configs keep their exact protection.
/// `Custom` carries per-vector flags; since CD-29 it is available per-window too.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Off,
    Green,
    Yellow,
    Red,
    Custom,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "off" => Some(Level::Off),
            "green" => Some(Level::Green),
            // Aliases (pre-CD-30 persisted values / older pages): the CONTENT is
            // identical, only the product name changed - never a silent weakening.
            "yellow" | "standard" => Some(Level::Yellow),
            "red" | "strict" => Some(Level::Red),
            "custom" => Some(Level::Custom),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Green => "green",
            Level::Yellow => "yellow",
            Level::Red => "red",
            Level::Custom => "custom",
        }
    }
}

/// serde default for vectors added after CD-25: an old persisted config that
/// predates a vector gets it ON - never a silent weakening on upgrade.
fn on_by_default() -> bool {
    true
}

/// The resolved, per-window effective hardening config. `on` gates injection
/// entirely (Off => the render process injects nothing); the vector flags gate
/// the matching IIFEs in `hardening.js`; `strict` tightens the entropy-reduction
/// buckets (single common value / coarser timer quantum). Field names match the
/// JSON keys `hardening.js` reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Config {
    pub on: bool,
    pub strict: bool,
    /// Canvas 2D / offscreen readback farbling.
    pub canvas: bool,
    /// WebGL readback (readPixels) farbling. The vendor/renderer identity clamp
    /// lives under `gpu` since CD-29.
    pub webgl: bool,
    /// GPU identity: UNMASKED_VENDOR/RENDERER clamp + WebGPU adapter neutralized.
    #[serde(default = "on_by_default")]
    pub gpu: bool,
    /// AudioContext readback farbling.
    pub audio: bool,
    /// Client rects + text-metrics farbling.
    pub metrics: bool,
    /// Device profile: CPU-core / memory buckets, touch points, battery, network
    /// hints, UA-CH high-entropy clamp.
    pub nav: bool,
    /// Standard font set: local fonts hidden behind the pinned common set; the
    /// explicit enumeration API reports none.
    pub fonts: bool,
    /// Clock precision: high-resolution timers quantized.
    #[serde(default = "on_by_default")]
    pub timing: bool,
    /// Media profile: codec / media-capability answers from a fixed common table;
    /// speech-synthesis voices hidden.
    #[serde(default = "on_by_default")]
    pub media: bool,
    /// Math normalization: transcendental low-mantissa rounding differences erased.
    #[serde(default = "on_by_default")]
    pub math: bool,
    /// Window size: the inner-size read cluster (`innerWidth`/`innerHeight`,
    /// the root `clientWidth`/`clientHeight`, `visualViewport`, `outerWidth`/
    /// `outerHeight` and the viewport-derived `matchMedia` features) reported
    /// as the nearest common step of the CD-29 screen ladder (CD-32, D-0049).
    /// The real window is never moved below Red; at Red the window already IS a
    /// common step, so the same reporting is the identity and reported == real.
    #[serde(default = "on_by_default")]
    pub viewport: bool,
}

/// The individually settable vector keys, in the canonical (serialization) order.
/// UI surfaces, the IPC vector parser and the weaken classifier all walk THIS list,
/// so adding a vector here is the single point of extension.
pub const VECTOR_KEYS: [&str; 11] = [
    "canvas", "webgl", "gpu", "audio", "metrics", "nav", "fonts", "timing", "media", "math",
    "viewport",
];

impl Config {
    /// The all-vectors, moderate-buckets config - the **Yellow** Ampel level
    /// (CD-30; formerly the `Standard` preset, const name kept for the fail-safe
    /// plumbing). This is the fail-safe default everywhere: an unconfigured
    /// browser is always protected at least this much.
    pub const STANDARD: Config = Config {
        on: true,
        strict: false,
        canvas: true,
        webgl: true,
        gpu: true,
        audio: true,
        metrics: true,
        nav: true,
        fonts: true,
        timing: true,
        media: true,
        math: true,
        viewport: true,
    };

    /// The **Green** Ampel level (CD-30): the coherent everyday core - WITHOUT
    /// the three aggressive clamps (clock/timing precision, media/codec
    /// normalization, math rounding) that can cause minor site quirks. Moderate
    /// buckets. Window-size reporting IS part of Green (CD-32, D-0049): it is the
    /// level rule that the real window is never moved below Red but the inner size
    /// is always reported as a common step, so Green and Yellow report alike.
    pub const GREEN: Config = Config {
        timing: false,
        media: false,
        math: false,
        ..Config::STANDARD
    };

    /// The all-vectors, tight-buckets config - the **Red** Ampel level (CD-30;
    /// formerly the `Strict` preset). The CD-30 window-size lock rides on the
    /// LEVEL at the shell layer, not on this config.
    pub const STRICT: Config = Config { strict: true, ..Config::STANDARD };

    /// Off - no injection at all.
    pub const OFF: Config = Config {
        on: false,
        strict: false,
        canvas: false,
        webgl: false,
        gpu: false,
        audio: false,
        metrics: false,
        nav: false,
        fonts: false,
        timing: false,
        media: false,
        math: false,
        viewport: false,
    };

    /// The vector flags in [`VECTOR_KEYS`] order.
    pub fn vector_flags(&self) -> [bool; VECTOR_KEYS.len()] {
        [
            self.canvas,
            self.webgl,
            self.gpu,
            self.audio,
            self.metrics,
            self.nav,
            self.fonts,
            self.timing,
            self.media,
            self.math,
            self.viewport,
        ]
    }

    /// Set one vector flag by key. Returns false for an unknown key.
    pub fn set_vector(&mut self, key: &str, on: bool) -> bool {
        let f = match key {
            "canvas" => &mut self.canvas,
            "webgl" => &mut self.webgl,
            "gpu" => &mut self.gpu,
            "audio" => &mut self.audio,
            "metrics" => &mut self.metrics,
            "nav" => &mut self.nav,
            "fonts" => &mut self.fonts,
            "timing" => &mut self.timing,
            "media" => &mut self.media,
            "math" => &mut self.math,
            "viewport" => &mut self.viewport,
            _ => return false,
        };
        *f = on;
        true
    }

    pub fn any_vector(&self) -> bool {
        self.vector_flags().iter().any(|&v| v)
    }

    /// Build a custom config from an IPC `vectors` JSON object: every key from
    /// [`VECTOR_KEYS`] present as a boolean is applied; absent keys stay ON
    /// (fail-protected, matching the serde upgrade rule). `on` follows whether any
    /// vector survives; custom is never `strict`.
    pub fn from_vectors_value(v: &serde_json::Value) -> Config {
        let mut cfg = Config::STANDARD;
        for key in VECTOR_KEYS {
            if let Some(b) = v.get(key).and_then(|x| x.as_bool()) {
                cfg.set_vector(key, b);
            }
        }
        Config { on: cfg.any_vector(), strict: false, ..cfg }
    }

    /// Serialize to the compact JSON `hardening.js` reads / `extra_info` carries.
    /// Never fails in practice; falls back to the Standard literal.
    pub fn to_json(self) -> String {
        serde_json::to_string(&self).unwrap_or_else(|_| STANDARD_JSON.to_string())
    }

    /// Parse a config JSON (from `extra_info` / the store). Fail-safe: a
    /// malformed/absent config resolves to Standard, so a browser we somehow failed
    /// to configure is still PROTECTED, never silently Off. A CD-25-era JSON that
    /// predates some vectors gets them ON via the serde defaults.
    pub fn from_json(s: &str) -> Config {
        serde_json::from_str(s).unwrap_or(Config::STANDARD)
    }
}

/// The Standard config as a JSON literal (the fail-safe fallback / render default).
/// Must stay byte-identical to `Config::STANDARD.to_json()` (unit-tested).
pub const STANDARD_JSON: &str = "{\"on\":true,\"strict\":false,\"canvas\":true,\"webgl\":true,\"gpu\":true,\"audio\":true,\"metrics\":true,\"nav\":true,\"fonts\":true,\"timing\":true,\"media\":true,\"math\":true,\"viewport\":true}";

/// Resolve a level (+ the custom flags, used only when `level == Custom`)
/// into an effective [`Config`].
pub fn resolve(level: Level, custom: Config) -> Config {
    match level {
        Level::Off => Config::OFF,
        Level::Green => Config::GREEN,
        Level::Yellow => Config::STANDARD,
        Level::Red => Config::STRICT,
        // Custom is moderate-strength (never `strict`) with the user's chosen
        // vectors; `on` follows whether any vector survives.
        Level::Custom => Config {
            on: custom.any_vector(),
            strict: false,
            ..custom
        },
    }
}

/// Is moving from `current` to `target` a WEAKENING? This is what the safety gate
/// keys on: a weakening needs the honest warning + two confirmations;
/// strengthening (re-enabling a vector, tightening buckets, turning protection
/// on) is ungated. A weakening is any of:
/// * turning protection off,
/// * dropping any active vector (e.g. Yellow → Green drops timing/media/math), or
/// * loosening the tight `strict` buckets (Red → Yellow). CD-30 revised this -
///   the Ampel makes Green < Yellow < Red a strict protection ORDER, so every
///   step down the ladder is gated, including leaving Red.
pub fn is_weakening(current: &Config, target: &Config) -> bool {
    if current.on && !target.on {
        return true;
    }
    if current.on && current.strict && !target.strict {
        return true;
    }
    current
        .vector_flags()
        .iter()
        .zip(target.vector_flags().iter())
        .any(|(&cur, &tgt)| cur && !tgt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_resolve_matches_failsafe_literal() {
        // The Yellow (former Standard) config must be byte-identical to the
        // fail-safe literal, so an unconfigured browser and a Yellow one behave
        // identically.
        assert_eq!(Config::STANDARD.to_json(), STANDARD_JSON);
        assert_eq!(resolve(Level::Yellow, Config::OFF).to_json(), STANDARD_JSON);
        assert!(resolve(Level::Yellow, Config::OFF).on);
        assert!(!resolve(Level::Yellow, Config::OFF).strict);
    }

    #[test]
    fn off_resolves_to_no_injection() {
        assert!(!resolve(Level::Off, Config::STANDARD).on);
        assert!(!resolve(Level::Off, Config::STANDARD).any_vector());
    }

    #[test]
    fn strict_tightens_buckets() {
        let s = resolve(Level::Red, Config::OFF);
        assert!(s.on && s.strict && s.any_vector());
    }

    #[test]
    fn green_is_the_core_without_the_aggressive_clamps() {
        // Green = everything except clock/timing, media/codec and math rounding,
        // at moderate buckets (CD-30 Task C).
        let g = resolve(Level::Green, Config::OFF);
        assert!(g.on && !g.strict);
        assert!(g.canvas && g.webgl && g.gpu && g.audio && g.metrics && g.nav && g.fonts);
        assert!(!g.timing && !g.media && !g.math);
        // CD-32 (D-0049): reporting a common inner size is the level rule for
        // everything below Red, so Green and Yellow report alike - only the real
        // window snapping is Red's.
        assert!(g.viewport, "Green reports a common window size too");
    }

    #[test]
    fn pre_cd30_level_names_parse_as_the_same_protection() {
        // A persisted "standard"/"strict" keeps its exact content under the new
        // Ampel names - an upgrade never silently changes protection.
        assert_eq!(Level::parse("standard"), Some(Level::Yellow));
        assert_eq!(Level::parse("strict"), Some(Level::Red));
        assert_eq!(Level::parse("green"), Some(Level::Green));
        assert_eq!(Level::parse("yellow"), Some(Level::Yellow));
        assert_eq!(Level::parse("red"), Some(Level::Red));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("custom"), Some(Level::Custom));
        assert_eq!(Level::parse("bunker"), None);
        // The canonical serialization uses the new names.
        assert_eq!(Level::Yellow.as_str(), "yellow");
        assert_eq!(Level::Red.as_str(), "red");
    }

    #[test]
    fn every_step_down_the_ampel_ladder_is_a_weakening() {
        let off = resolve(Level::Off, Config::OFF);
        let green = resolve(Level::Green, Config::OFF);
        let yellow = resolve(Level::Yellow, Config::OFF);
        let red = resolve(Level::Red, Config::OFF);
        // Down = gated…
        assert!(is_weakening(&red, &yellow), "Red→Yellow loosens the tight buckets");
        assert!(is_weakening(&yellow, &green), "Yellow→Green drops timing/media/math");
        assert!(is_weakening(&green, &off), "Green→Off turns protection off");
        assert!(is_weakening(&red, &green) && is_weakening(&red, &off));
        assert!(is_weakening(&yellow, &off));
        // …up = immediate.
        assert!(!is_weakening(&off, &green));
        assert!(!is_weakening(&green, &yellow));
        assert!(!is_weakening(&yellow, &red));
        assert!(!is_weakening(&off, &red) && !is_weakening(&green, &red));
    }

    #[test]
    fn custom_is_standard_strength_with_chosen_vectors() {
        let custom = Config { canvas: false, ..Config::STANDARD };
        let r = resolve(Level::Custom, custom);
        assert!(r.on); // some vectors still on
        assert!(!r.strict); // custom is never strict
        assert!(!r.canvas && r.webgl);
    }

    #[test]
    fn custom_all_off_is_effectively_off() {
        let r = resolve(Level::Custom, Config::OFF);
        assert!(!r.on); // no vector survives -> nothing to inject
    }

    #[test]
    fn weakening_classification_covers_every_vector() {
        // Off from anything-on = weaken.
        assert!(is_weakening(&Config::STANDARD, &Config::OFF));
        // Dropping ANY single vector = weaken; re-enabling it = strengthen.
        for key in VECTOR_KEYS {
            let mut dropped = Config::STANDARD;
            assert!(dropped.set_vector(key, false), "unknown key {key}");
            assert!(is_weakening(&Config::STANDARD, &dropped), "{key} off must weaken");
            assert!(!is_weakening(&dropped, &Config::STANDARD), "{key} on must not weaken");
        }
        // Yellow -> Red = strengthen; Red -> Yellow = a weakening since CD-30
        // (the Ampel ladder is a strict protection order - leaving Red is gated).
        assert!(!is_weakening(&Config::STANDARD, &Config::STRICT));
        assert!(is_weakening(&Config::STRICT, &Config::STANDARD));
        // Off -> Yellow = strengthen.
        assert!(!is_weakening(&Config::OFF, &Config::STANDARD));
        // An OFF config's inert `strict` bit is irrelevant: coming from Off,
        // nothing can be a weakening.
        let off_strict = Config { strict: true, ..Config::OFF };
        assert!(!is_weakening(&off_strict, &Config::STANDARD));
    }

    #[test]
    fn from_json_failsafe_is_standard() {
        assert_eq!(Config::from_json("garbage"), Config::STANDARD);
        assert_eq!(Config::from_json(STANDARD_JSON), Config::STANDARD);
    }

    #[test]
    fn cd25_era_json_upgrades_with_new_vectors_on() {
        // A persisted CD-25 custom config (six vectors, canvas dropped) must parse
        // with every CD-29 vector ON - an upgrade never silently weakens.
        let old = "{\"on\":true,\"strict\":false,\"canvas\":false,\"webgl\":true,\"audio\":true,\"metrics\":true,\"nav\":true,\"fonts\":true}";
        let cfg = Config::from_json(old);
        assert!(!cfg.canvas && cfg.webgl);
        assert!(cfg.gpu && cfg.timing && cfg.media && cfg.math, "new vectors default ON");
    }

    #[test]
    fn vectors_value_parses_partial_objects_fail_protected() {
        let v: serde_json::Value =
            serde_json::from_str("{\"timing\":false,\"gpu\":false}").unwrap();
        let cfg = Config::from_vectors_value(&v);
        assert!(!cfg.timing && !cfg.gpu);
        assert!(cfg.canvas && cfg.media && cfg.math, "absent keys stay ON");
        assert!(cfg.on && !cfg.strict);
        // All-off resolves to not-on.
        let all_off: serde_json::Value = serde_json::from_str(
            "{\"canvas\":false,\"webgl\":false,\"gpu\":false,\"audio\":false,\"metrics\":false,\"nav\":false,\"fonts\":false,\"timing\":false,\"media\":false,\"math\":false,\"viewport\":false}",
        )
        .unwrap();
        assert!(!Config::from_vectors_value(&all_off).on);
    }

    #[test]
    fn vector_keys_and_flags_stay_in_lockstep() {
        // Every key round-trips through set_vector -> vector_flags at the same
        // index - the invariant the UI/IPC walk relies on.
        for (i, key) in VECTOR_KEYS.iter().enumerate() {
            let mut cfg = Config::STANDARD;
            cfg.set_vector(key, false);
            let flags = cfg.vector_flags();
            assert!(!flags[i], "{key} must map to flag index {i}");
            assert_eq!(flags.iter().filter(|&&f| !f).count(), 1);
        }
        assert!(!Config::STANDARD.clone().set_vector("bogus", false));
    }
}
