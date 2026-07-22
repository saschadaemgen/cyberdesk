//! Live application settings - the single source of truth shared between the
//! render loop (main thread) and the settings IPC (CEF UI thread).
//!
//! The SQLite [`Store`] is owned here for the life of the process: Stage A
//! created and seeded it; Stage D hands it to the settings IPC for live writes.
//! The boolean toggles and the numeric glow-intensity are mirrored into
//! lock-free atomics so the render loop can read them every frame without
//! touching SQLite.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use crate::harden;
use crate::store::Store;

/// The persisted key/value store (shared process-wide with the history/favorites
/// layer - see [`crate::store::shared`]).
fn store() -> &'static Mutex<Store> {
    crate::store::shared()
}

static FEATHER_EDGES: AtomicBool = AtomicBool::new(true);
static ANIMATED_BACKGROUND: AtomicBool = AtomicBool::new(true);
static STAY_FOREGROUND: AtomicBool = AtomicBool::new(true);
/// Purge any browsing cache/content residue from disk on every launch (CD-34,
/// D-0051). Default ON - it is the anti-forensic guarantee; disabling it lets
/// residue accumulate, so turning it off routes through the weakening gate (D-0040).
static PURGE_RESIDUE: AtomicBool = AtomicBool::new(true);
/// Glow intensity as a whole percent (50..=220). The authoritative default is
/// the `background.glow_default` token, applied in [`init`]; this literal is
/// only a pre-init placeholder.
static GLOW_INTENSITY: AtomicU32 = AtomicU32::new(115);
/// Search engine for the command-bar search fallback, as a small id (0=google,
/// 1=duckduckgo, 2=bing, 3=startpage, 4=brave). The factory default is
/// DuckDuckGo (CD-27, D-0043) - a de-Googled browser must not ship Google as
/// its default search; Google stays a selectable option.
static SEARCH_ENGINE: AtomicU8 = AtomicU8::new(DEFAULT_ENGINE);

/// The factory-default engine id: DuckDuckGo (CD-27, D-0043).
const DEFAULT_ENGINE: u8 = 1;
/// Whether the per-window Tor engine is available at all (CD-15). When off, the
/// toggle glyph does nothing and new windows never default to Tor.
static TOR_ENABLED: AtomicBool = AtomicBool::new(true);
/// Whether new windows open in Tor by default (CD-15). Off by default (clearnet).
static TOR_DEFAULT: AtomicBool = AtomicBool::new(false);
/// The GLOBAL fingerprinting-hardening Ampel level a window inherits (CD-25;
/// graded CD-30): 0=off, 1=green (the factory default - the coherent everyday
/// level), 2=yellow, 3=red, 4=custom. A per-window override lives in browser.rs
/// (`SLOT_HARDENING`); this is the default it falls back to.
static HARDENING_LEVEL: AtomicU8 = AtomicU8::new(1);
/// The custom per-vector flags used only when the level is `custom`. Read at
/// browser-create time and by the frame-state push (not per rendered frame), so a
/// Mutex is fine. Defaults to Standard (all vectors on).
static HARDENING_CUSTOM: Mutex<harden::Config> = Mutex::new(harden::Config::STANDARD);
/// The GLOBAL reported-screen-size preset (CD-29): 0=1280x720, 1=1600x900,
/// 2=1920x1080 (default - the most common real desktop resolution). This is the
/// COMMON value web slots report for `screen.*`; the actual viewport is never
/// faked (browser.rs `common_screen_for` keeps reported ≥ measured). A per-window
/// override lives in browser.rs (`SLOT_SCREEN`).
static SCREEN_PRESET: AtomicU8 = AtomicU8::new(DEFAULT_SCREEN_PRESET);

/// The resolved appearance (CD-45, D-0065): template + accent + the
/// template's options, cached so the render loop reads it without touching
/// the store. THE single source both the CSS fan-out and the shader uniforms
/// read; see `appearance.rs` for why that matters.
static APPEARANCE: Mutex<Option<crate::appearance::Resolved>> = Mutex::new(None);
/// The factory-default reported screen preset: 1920x1080 (id 2).
const DEFAULT_SCREEN_PRESET: u8 = 2;

// --- Per-session identity rotation (CD-29, D-0046) --------------------------
/// Fresh global identity (farble seed) each launch. Default ON - the safe,
/// unlinkable default. When OFF the seed persists across launches (a deliberately
/// stable cross-launch identity).
static ROTATE_ON_RESTART: AtomicBool = AtomicBool::new(true);
/// Automatically re-roll the global identity every [`ROTATE_INTERVAL_MIN`] minutes
/// (the Pulse Grid countdown showpiece). Default OFF.
static ROTATE_AUTO: AtomicBool = AtomicBool::new(false);
/// Also rotate the Tor circuit(s) on every identity rotation. Default OFF.
static ROTATE_NEW_CIRCUIT: AtomicBool = AtomicBool::new(false);
/// The automatic-rotation interval, in whole minutes (clamped to
/// [`ROTATE_INTERVAL_MIN_BOUND`]..=[`ROTATE_INTERVAL_MAX_BOUND`]).
static ROTATE_INTERVAL_MIN: AtomicU32 = AtomicU32::new(DEFAULT_ROTATE_INTERVAL);
const DEFAULT_ROTATE_INTERVAL: u32 = 15;
pub const ROTATE_INTERVAL_MIN_BOUND: u32 = 1;
pub const ROTATE_INTERVAL_MAX_BOUND: u32 = 180;

/// The settings keys the internal view is allowed to read and write. Anything
/// outside this list is rejected by [`set`] / [`set_glow_intensity`].
pub const KEY_FEATHER_EDGES: &str = "feather_edges";
/// The background on/off toggle. Renamed from `deep_field` in CD-05 (D-0012):
/// it now governs whichever background the template selects (Pulse Grid or
/// Deep Field), not the Deep Field specifically. The store migrates the old key.
pub const KEY_ANIMATED_BACKGROUND: &str = "animated_background";
pub const KEY_STAY_FOREGROUND: &str = "stay_foreground";
/// The on-launch browsing-residue purge toggle (CD-34, D-0051).
pub const KEY_PURGE_RESIDUE: &str = "purge_residue";
pub const KEY_GLOW_INTENSITY: &str = "glow_intensity";
/// The command-bar search-engine choice (CD-07). One of the ids below.
pub const KEY_SEARCH_ENGINE: &str = "search_engine";
/// Appearance (CD-45, D-0065): the selected template, the user accent colour
/// (absent = the template's default), and the template's own options.
pub const KEY_TEMPLATE: &str = "template";
pub const KEY_ACCENT: &str = "accent";
pub const KEY_BG_INTENSITY: &str = "bg_intensity";
pub const KEY_MOTION: &str = "motion";
/// Per-window Tor: the engine master switch and the new-window default (CD-15).
pub const KEY_TOR_ENABLED: &str = "tor_enabled";
pub const KEY_TOR_DEFAULT: &str = "tor_default";
/// Global fingerprinting-hardening preset + custom per-vector flags (CD-25).
pub const KEY_HARDENING_LEVEL: &str = "hardening_level";
pub const KEY_HARDENING_CUSTOM: &str = "hardening_custom";
/// Global reported-screen-size preset (CD-29).
pub const KEY_SCREEN_PRESET: &str = "screen_preset";
/// Per-session identity rotation (CD-29).
pub const KEY_ROTATE_ON_RESTART: &str = "rotate_on_restart";
pub const KEY_ROTATE_AUTO: &str = "rotate_auto";
pub const KEY_ROTATE_NEW_CIRCUIT: &str = "rotate_new_circuit";
pub const KEY_ROTATE_INTERVAL: &str = "rotate_interval_min";
/// The persisted global identity seed key (only meaningful when `rotate_on_restart`
/// is off - a stable cross-launch identity).
const KEY_IDENTITY_SEED: &str = "identity_seed";
/// The persisted seed's mint time (unix epoch ms, CD-30) - kept in step with the
/// seed so a restored identity reports its REAL age in the HUD.
const KEY_IDENTITY_SEED_BORN: &str = "identity_seed_born";

/// Glow-intensity slider bounds (percent).
pub const GLOW_MIN: u32 = 50;
pub const GLOW_MAX: u32 = 220;

/// Map a search-engine id string to its small numeric code (and back). The
/// allowlist is defined here - anything outside it is rejected.
fn engine_id(value: &str) -> Option<u8> {
    match value {
        "google" => Some(0),
        "duckduckgo" => Some(1),
        "bing" => Some(2),
        "startpage" => Some(3),
        "brave" => Some(4),
        _ => None,
    }
}
fn engine_name(id: u8) -> &'static str {
    match id {
        0 => "google",
        2 => "bing",
        3 => "startpage",
        4 => "brave",
        // The factory default - and the defense-in-depth fallback for any
        // out-of-range id: never silently Google (CD-27, D-0043).
        _ => "duckduckgo",
    }
}

/// The selected search engine:
/// `google` | `duckduckgo` | `bing` | `startpage` | `brave`.
pub fn search_engine() -> &'static str {
    engine_name(SEARCH_ENGINE.load(Ordering::Relaxed))
}

// --- Fingerprinting-hardening config (CD-25, D-0040) ------------------------

fn level_code(l: harden::Level) -> u8 {
    match l {
        harden::Level::Off => 0,
        harden::Level::Green => 1,
        harden::Level::Yellow => 2,
        harden::Level::Red => 3,
        harden::Level::Custom => 4,
    }
}
fn level_from_code(c: u8) -> harden::Level {
    match c {
        0 => harden::Level::Off,
        2 => harden::Level::Yellow,
        3 => harden::Level::Red,
        4 => harden::Level::Custom,
        // The factory default AND the fallback for any out-of-range code.
        _ => harden::Level::Green,
    }
}

/// The GLOBAL hardening preset level a window inherits.
pub fn hardening_level() -> harden::Level {
    level_from_code(HARDENING_LEVEL.load(Ordering::Relaxed))
}

/// The level a PERSISTED value boots as (CD-31, D-0048): the Red bunker mode -
/// with its window-size lock - is always a deliberate in-session choice, never
/// a state the user launches into unexpectedly. A persisted highest level
/// (an old "strict" or a saved "red") comes up as Yellow: the full ten-vector
/// protection at standard buckets, freely resizable. Freshly choosing Red
/// in-session still fully engages the lock and the transition; this only
/// shapes the boot. Pure, so the rule is unit-testable.
fn boot_level(persisted: harden::Level) -> harden::Level {
    match persisted {
        harden::Level::Red => harden::Level::Yellow,
        l => l,
    }
}

/// Resolve the hardening level a store boots at. A PERSISTED value governs,
/// shaped by [`boot_level`] (a persisted highest level comes up as Yellow - the
/// Red bunker is opt-in per session, CD-31/D-0048; a pre-CD-30 "standard" parses
/// to Yellow - identical content, so an explicit choice never silently weakens
/// on upgrade). When the key is ABSENT - a fresh install - the factory default
/// is GREEN (the coherent everyday level, CD-30/D-0047). Extracted from [`init`]
/// so the fresh-install default is unit-testable: the store must NOT seed this
/// key, or the Green fallback is dead and fresh installs silently boot at Yellow.
fn resolve_hardening_boot_level(s: &Store) -> harden::Level {
    s.get(KEY_HARDENING_LEVEL)
        .and_then(|v| harden::Level::parse(&v))
        .map(boot_level)
        .unwrap_or(harden::Level::Green)
}

/// The stored custom per-vector flags (meaningful only when the level is Custom).
pub fn hardening_custom() -> harden::Config {
    *HARDENING_CUSTOM.lock().unwrap()
}

/// The resolved GLOBAL effective config a window inherits when it has no per-window
/// override.
pub fn hardening_global_config() -> harden::Config {
    harden::resolve(hardening_level(), hardening_custom())
}

// --- Reported screen-size preset (CD-29) ------------------------------------

/// Map a screen-preset name to its code (and back). The allowlist is the small set
/// of common real resolutions the ticket pins.
pub fn screen_code(name: &str) -> Option<u8> {
    match name {
        "1280x720" => Some(0),
        "1600x900" => Some(1),
        "1920x1080" => Some(2),
        _ => None,
    }
}
pub fn screen_name(code: u8) -> &'static str {
    match code {
        0 => "1280x720",
        1 => "1600x900",
        // The factory default AND the fallback for any out-of-range id.
        _ => "1920x1080",
    }
}
/// A preset code's (width, height) in CSS px (DIP).
pub fn screen_dims(code: u8) -> (u32, u32) {
    match code {
        0 => (1280, 720),
        1 => (1600, 900),
        _ => (1920, 1080),
    }
}

/// The GLOBAL reported-screen-size preset code.
pub fn screen_preset_code() -> u8 {
    SCREEN_PRESET.load(Ordering::Relaxed)
}
/// The GLOBAL reported-screen-size preset name (for the settings snapshot).
pub fn screen_preset_name() -> &'static str {
    screen_name(screen_preset_code())
}
/// The GLOBAL reported-screen-size preset dimensions a window inherits.
pub fn screen_preset_dims() -> (u32, u32) {
    screen_dims(screen_preset_code())
}

/// Apply and persist the global reported-screen preset (validated against the
/// allowlist). A screen-size change is a fingerprint-config change: the caller
/// (the IPC) respawns inheriting slots so the new value takes effect on load.
pub fn set_screen_preset(value: &str) -> Result<String, String> {
    let code = screen_code(value).ok_or_else(|| format!("unknown screen preset: {value}"))?;
    SCREEN_PRESET.store(code, Ordering::Relaxed);
    store().lock().unwrap().set(KEY_SCREEN_PRESET, value);
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{KEY_SCREEN_PRESET}\",\"value\":\"{value}\"}}"
    ))
}

// --- Appearance: accent + template (CD-45, D-0065) --------------------------

/// Resolve the appearance from the store: the selected template supplies the
/// defaults, an explicit user choice overrides each one. Unknown ids and
/// malformed colours fall back rather than fail, so a store written by a
/// future build can never brick the picker.
fn resolve_appearance(s: &Store) -> crate::appearance::Resolved {
    use crate::appearance;
    let template_id = s
        .get(KEY_TEMPLATE)
        .filter(|id| appearance::TEMPLATES.iter().any(|t| t.id == id))
        .unwrap_or_else(|| appearance::DEFAULT_TEMPLATE.to_string());
    let t = appearance::template(&template_id);
    let accent = s
        .get(KEY_ACCENT)
        .and_then(|v| appearance::normalize_hex(&v))
        .unwrap_or_else(|| t.default_accent.to_string());
    let intensity = s
        .get(KEY_BG_INTENSITY)
        .and_then(|v| v.parse::<u8>().ok())
        .map(|v| v.min(200))
        .unwrap_or(t.default_intensity);
    let motion = s
        .get(KEY_MOTION)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(t.default_motion);
    crate::appearance::Resolved {
        template_id,
        accent,
        intensity,
        // Glow keeps its own long-standing setting and slider (CD-05); the
        // template only supplies the value a fresh install starts from, so
        // there is ONE glow control rather than two that could disagree.
        glow: glow_intensity_percent().min(200) as u8,
        motion,
    }
}

/// The resolved appearance. Cheap: the cache is filled at init and on every
/// change, so the render loop can call this per frame.
pub fn appearance() -> crate::appearance::Resolved {
    if let Some(a) = APPEARANCE.lock().unwrap().as_ref() {
        let mut a = a.clone();
        // Glow lives in its own setting, so keep the view of it current.
        a.glow = glow_intensity_percent().min(200) as u8;
        return a;
    }
    let a = resolve_appearance(&store().lock().unwrap());
    *APPEARANCE.lock().unwrap() = Some(a.clone());
    a
}

/// Re-resolve and cache the appearance (after any appearance write).
fn refresh_appearance() {
    let a = resolve_appearance(&store().lock().unwrap());
    *APPEARANCE.lock().unwrap() = Some(a);
}

/// The accent as shader components. THE one value the wgpu uniforms see.
pub fn accent_rgb() -> [f32; 3] {
    appearance().accent_rgb()
}

/// Does the active template run the Pulse Grid? (The renderer's background
/// selection: the template owns it now, D-0012 tokens supply the details.)
pub fn background_is_pulse_grid() -> bool {
    appearance().effect() == crate::appearance::Effect::PulseGrid
}

/// Select a template. Its own defaults apply to any option the user has not
/// explicitly set (the per-option keys are cleared, so switching templates
/// gives that template's look rather than the previous one's leftovers).
pub fn set_template(id: &str) -> Result<String, String> {
    if !crate::appearance::TEMPLATES.iter().any(|t| t.id == id) {
        return Err(format!("unknown template: {id}"));
    }
    let t = crate::appearance::template(id);
    {
        let s = store().lock().unwrap();
        s.set(KEY_TEMPLATE, id);
        s.delete(KEY_ACCENT);
        s.delete(KEY_BG_INTENSITY);
        s.delete(KEY_MOTION);
        // Glow keeps its own long-standing setting and slider (CD-05), so a
        // template switch applies that template's glow default explicitly
        // rather than leaving the previous template's value behind.
        s.set(KEY_GLOW_INTENSITY, &t.default_glow.to_string());
    }
    GLOW_INTENSITY.store(
        (t.default_glow as u32).clamp(GLOW_MIN, GLOW_MAX),
        Ordering::Relaxed,
    );
    refresh_appearance();
    Ok(format!("{{\"ok\":true,\"key\":\"{KEY_TEMPLATE}\",\"value\":\"{id}\"}}"))
}

/// Set the accent colour: a preset id or a custom `#RRGGBB`. Validated here,
/// because the value reaches both a stylesheet and a shader uniform.
pub fn set_accent(value: &str) -> Result<String, String> {
    let hex = crate::appearance::PRESETS
        .iter()
        .find(|p| p.id == value)
        .map(|p| p.hex.to_string())
        .or_else(|| crate::appearance::normalize_hex(value))
        .ok_or_else(|| format!("not a colour: {value}"))?;
    store().lock().unwrap().set(KEY_ACCENT, &hex);
    refresh_appearance();
    Ok(format!("{{\"ok\":true,\"key\":\"{KEY_ACCENT}\",\"value\":\"{hex}\"}}"))
}

/// Background-effect intensity, percent of the template baseline (0..=200).
pub fn set_bg_intensity(percent: i64) -> Result<String, String> {
    let clamped = percent.clamp(0, 200) as u8;
    store()
        .lock()
        .unwrap()
        .set(KEY_BG_INTENSITY, &clamped.to_string());
    refresh_appearance();
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{KEY_BG_INTENSITY}\",\"value\":{clamped}}}"
    ))
}

/// Background motion on or off (a still background still renders, it simply
/// stops animating).
pub fn set_motion(on: bool) -> Result<String, String> {
    store()
        .lock()
        .unwrap()
        .set(KEY_MOTION, if on { "1" } else { "0" });
    refresh_appearance();
    Ok(format!("{{\"ok\":true,\"key\":\"{KEY_MOTION}\",\"value\":{on}}}"))
}

// --- Identity rotation (CD-29) ----------------------------------------------

/// Fresh global identity each launch (default). When off, the seed persists.
pub fn rotate_on_restart() -> bool {
    ROTATE_ON_RESTART.load(Ordering::Relaxed)
}
/// Is automatic rotation on?
pub fn rotate_auto() -> bool {
    ROTATE_AUTO.load(Ordering::Relaxed)
}
/// Also rotate the Tor circuit(s) on a rotation?
pub fn rotate_new_circuit() -> bool {
    ROTATE_NEW_CIRCUIT.load(Ordering::Relaxed)
}
/// The automatic-rotation interval in whole minutes (bounded).
pub fn rotate_interval_min() -> u32 {
    ROTATE_INTERVAL_MIN.load(Ordering::Relaxed)
}

/// Apply and persist the automatic-rotation interval (whole minutes, clamped).
pub fn set_rotate_interval(minutes: i64) -> Result<String, String> {
    let clamped =
        minutes.clamp(ROTATE_INTERVAL_MIN_BOUND as i64, ROTATE_INTERVAL_MAX_BOUND as i64) as u32;
    ROTATE_INTERVAL_MIN.store(clamped, Ordering::Relaxed);
    store()
        .lock()
        .unwrap()
        .set(KEY_ROTATE_INTERVAL, &clamped.to_string());
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{KEY_ROTATE_INTERVAL}\",\"value\":{clamped}}}"
    ))
}

/// The persisted global identity seed, if any (for the stable cross-launch
/// identity). Since CD-40 (D-0058) the seed is a sealed-vault tenant when a
/// vault exists: it keys the fingerprint farbling, so it is linkage material.
/// With a vault present the plaintext row NEVER answers (fail-closed - a
/// locked/bypassed vault yields None and the caller mints a fresh seed).
pub fn persisted_identity_seed() -> Option<String> {
    if crate::vault::has_vault() {
        return crate::vault::sealed_get(KEY_IDENTITY_SEED);
    }
    store().lock().unwrap().get(KEY_IDENTITY_SEED)
}
/// Persist the global identity seed (for the stable cross-launch identity).
/// Routed into the sealed vault state when a vault exists (no-op while it is
/// not unlocked - never a plaintext fallback).
pub fn store_identity_seed(seed: &str) {
    if crate::vault::has_vault() {
        crate::vault::sealed_set(KEY_IDENTITY_SEED, seed);
        return;
    }
    store().lock().unwrap().set(KEY_IDENTITY_SEED, seed);
}
/// The persisted seed's mint time (unix epoch ms), if any (CD-30). Sealed
/// alongside the seed when a vault exists (see [`persisted_identity_seed`]).
pub fn persisted_identity_born() -> Option<u64> {
    let raw = if crate::vault::has_vault() {
        crate::vault::sealed_get(KEY_IDENTITY_SEED_BORN)
    } else {
        store().lock().unwrap().get(KEY_IDENTITY_SEED_BORN)
    };
    raw.and_then(|v| v.parse::<u64>().ok())
}
/// Persist the seed's mint time alongside the seed (CD-30).
pub fn store_identity_born(ms: u64) {
    if crate::vault::has_vault() {
        crate::vault::sealed_set(KEY_IDENTITY_SEED_BORN, &ms.to_string());
        return;
    }
    store()
        .lock()
        .unwrap()
        .set(KEY_IDENTITY_SEED_BORN, &ms.to_string());
}

/// Apply and persist the global hardening config (CD-25). `level` is one of
/// off/standard/strict/custom; `vectors` supplies the per-vector flags for custom.
/// A WEAKENING change (any vector dropped, or turned off) is refused without
/// `confirm` - the host re-validates the two-confirmation safety gate rather than
/// trusting the page to have run it. Strengthening is always allowed. Returns the
/// reply JSON on success.
pub fn set_hardening(
    level: &str,
    vectors: Option<harden::Config>,
    confirm: bool,
) -> Result<String, String> {
    let lvl = harden::Level::parse(level).ok_or_else(|| format!("unknown hardening level: {level}"))?;
    let current = hardening_global_config();
    let new_custom = if lvl == harden::Level::Custom {
        vectors.unwrap_or_else(hardening_custom)
    } else {
        hardening_custom()
    };
    let target = harden::resolve(lvl, new_custom);
    if harden::is_weakening(&current, &target) && !confirm {
        return Err("weakening hardening requires confirmation".to_string());
    }
    HARDENING_LEVEL.store(level_code(lvl), Ordering::Relaxed);
    let store = store().lock().unwrap();
    store.set(KEY_HARDENING_LEVEL, level);
    if lvl == harden::Level::Custom {
        *HARDENING_CUSTOM.lock().unwrap() = new_custom;
        store.set(KEY_HARDENING_CUSTOM, &new_custom.to_json());
    }
    Ok(format!("{{\"ok\":true,\"key\":\"{KEY_HARDENING_LEVEL}\",\"value\":\"{level}\"}}"))
}

/// Open the store and load the persisted settings into the atomics. Must be
/// called once on the main thread before CEF starts.
pub fn init() {
    let default_glow = (crate::theme::Theme::load().background.glow_default.round() as i64)
        .clamp(GLOW_MIN as i64, GLOW_MAX as i64) as u32;
    let s = store().lock().unwrap();
    FEATHER_EDGES.store(s.get_bool(KEY_FEATHER_EDGES, true), Ordering::Relaxed);
    ANIMATED_BACKGROUND.store(s.get_bool(KEY_ANIMATED_BACKGROUND, true), Ordering::Relaxed);
    STAY_FOREGROUND.store(s.get_bool(KEY_STAY_FOREGROUND, true), Ordering::Relaxed);
    // CD-34: default ON - the anti-forensic guarantee is the safe default. Read before
    // the launch purge (app::run / fsprobe) consults it.
    PURGE_RESIDUE.store(s.get_bool(KEY_PURGE_RESIDUE, true), Ordering::Relaxed);
    let glow = s
        .get(KEY_GLOW_INTENSITY)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default_glow)
        .clamp(GLOW_MIN, GLOW_MAX);
    GLOW_INTENSITY.store(glow, Ordering::Relaxed);
    let engine = s
        .get(KEY_SEARCH_ENGINE)
        .and_then(|v| engine_id(&v))
        .unwrap_or(DEFAULT_ENGINE);
    SEARCH_ENGINE.store(engine, Ordering::Relaxed);
    TOR_ENABLED.store(s.get_bool(KEY_TOR_ENABLED, true), Ordering::Relaxed);
    TOR_DEFAULT.store(s.get_bool(KEY_TOR_DEFAULT, false), Ordering::Relaxed);
    let level = resolve_hardening_boot_level(&s);
    HARDENING_LEVEL.store(level_code(level), Ordering::Relaxed);
    if let Some(j) = s.get(KEY_HARDENING_CUSTOM) {
        *HARDENING_CUSTOM.lock().unwrap() = harden::Config::from_json(&j);
    }
    let screen = s
        .get(KEY_SCREEN_PRESET)
        .and_then(|v| screen_code(&v))
        .unwrap_or(DEFAULT_SCREEN_PRESET);
    SCREEN_PRESET.store(screen, Ordering::Relaxed);
    ROTATE_ON_RESTART.store(s.get_bool(KEY_ROTATE_ON_RESTART, true), Ordering::Relaxed);
    ROTATE_AUTO.store(s.get_bool(KEY_ROTATE_AUTO, false), Ordering::Relaxed);
    ROTATE_NEW_CIRCUIT.store(s.get_bool(KEY_ROTATE_NEW_CIRCUIT, false), Ordering::Relaxed);
    let interval = s
        .get(KEY_ROTATE_INTERVAL)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_ROTATE_INTERVAL)
        .clamp(ROTATE_INTERVAL_MIN_BOUND, ROTATE_INTERVAL_MAX_BOUND);
    ROTATE_INTERVAL_MIN.store(interval, Ordering::Relaxed);
}

/// Is the Tor engine available (the master switch)?
pub fn tor_enabled() -> bool {
    TOR_ENABLED.load(Ordering::Relaxed)
}

/// Should a new window open in Tor by default? (Only meaningful when the engine
/// is enabled.)
pub fn tor_default() -> bool {
    TOR_ENABLED.load(Ordering::Relaxed) && TOR_DEFAULT.load(Ordering::Relaxed)
}

pub fn feather_edges() -> bool {
    FEATHER_EDGES.load(Ordering::Relaxed)
}

pub fn animated_background() -> bool {
    ANIMATED_BACKGROUND.load(Ordering::Relaxed)
}

pub fn stay_foreground() -> bool {
    STAY_FOREGROUND.load(Ordering::Relaxed)
}

/// Purge browsing residue from disk on launch? (CD-34, D-0051.) Read by
/// [`crate::forensic::purge_on_launch`] before CEF starts.
pub fn purge_residue() -> bool {
    PURGE_RESIDUE.load(Ordering::Relaxed)
}

/// Glow intensity as a whole percent (50..=220).
pub fn glow_intensity_percent() -> u32 {
    GLOW_INTENSITY.load(Ordering::Relaxed)
}

/// Glow intensity as a render multiplier (0.50..=2.20).
pub fn glow_intensity() -> f32 {
    glow_intensity_percent() as f32 / 100.0
}

/// Current settings as a JSON object string, for the `get_settings` IPC reply.
pub fn snapshot_json() -> String {
    // `fp_custom` is injected raw (it is already a JSON object, not a string).
    format!(
        "{{\"feather_edges\":{},\"animated_background\":{},\"stay_foreground\":{},\"purge_residue\":{},\"glow_intensity\":{},\"search_engine\":\"{}\",\"tor_enabled\":{},\"tor_default\":{},\"fp_preset\":\"{}\",\"fp_custom\":{},\"screen_preset\":\"{}\",\"rotate_on_restart\":{},\"rotate_auto\":{},\"rotate_new_circuit\":{},\"rotate_interval_min\":{},\"template\":\"{}\",\"accent\":\"{}\",\"bg_intensity\":{},\"motion\":{}}}",
        feather_edges(),
        animated_background(),
        stay_foreground(),
        purge_residue(),
        glow_intensity_percent(),
        search_engine(),
        TOR_ENABLED.load(Ordering::Relaxed),
        TOR_DEFAULT.load(Ordering::Relaxed),
        hardening_level().as_str(),
        hardening_custom().to_json(),
        screen_preset_name(),
        rotate_on_restart(),
        rotate_auto(),
        rotate_new_circuit(),
        rotate_interval_min(),
        appearance().template_id,
        appearance().accent,
        appearance().intensity,
        appearance().motion,
    )
}

/// Apply and persist a single boolean setting. Returns the reply JSON on
/// success, or an error message the IPC turns into a failure. Writes the atomic
/// (seen by the next rendered frame) and the SQLite row (survives restart).
pub fn set(key: &str, value: bool) -> Result<String, String> {
    let atomic = match key {
        KEY_FEATHER_EDGES => &FEATHER_EDGES,
        KEY_ANIMATED_BACKGROUND => &ANIMATED_BACKGROUND,
        KEY_STAY_FOREGROUND => &STAY_FOREGROUND,
        KEY_PURGE_RESIDUE => &PURGE_RESIDUE,
        KEY_TOR_ENABLED => &TOR_ENABLED,
        KEY_TOR_DEFAULT => &TOR_DEFAULT,
        KEY_ROTATE_ON_RESTART => &ROTATE_ON_RESTART,
        KEY_ROTATE_AUTO => &ROTATE_AUTO,
        KEY_ROTATE_NEW_CIRCUIT => &ROTATE_NEW_CIRCUIT,
        other => return Err(format!("unknown setting key: {other}")),
    };
    atomic.store(value, Ordering::Relaxed);
    store().lock().unwrap().set_bool(key, value);
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{key}\",\"value\":{value}}}"
    ))
}

/// Apply and persist the numeric glow-intensity setting (clamped to
/// `GLOW_MIN..=GLOW_MAX`). Stored as a string in the key/value store.
pub fn set_glow_intensity(percent: i64) -> Result<String, String> {
    let clamped = percent.clamp(GLOW_MIN as i64, GLOW_MAX as i64) as u32;
    GLOW_INTENSITY.store(clamped, Ordering::Relaxed);
    store()
        .lock()
        .unwrap()
        .set(KEY_GLOW_INTENSITY, &clamped.to_string());
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{KEY_GLOW_INTENSITY}\",\"value\":{clamped}}}"
    ))
}

/// Apply and persist the search-engine setting (validated against the allowlist).
/// Returns the reply JSON on success, or an error the IPC turns into a failure.
pub fn set_search_engine(value: &str) -> Result<String, String> {
    let id = engine_id(value).ok_or_else(|| format!("unknown search engine: {value}"))?;
    SEARCH_ENGINE.store(id, Ordering::Relaxed);
    store().lock().unwrap().set(KEY_SEARCH_ENGINE, value);
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{KEY_SEARCH_ENGINE}\",\"value\":\"{value}\"}}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ENGINE, KEY_HARDENING_LEVEL, boot_level, engine_id, engine_name,
        resolve_hardening_boot_level,
    };
    use crate::harden::Level;
    use crate::store::Store;

    /// A fresh install must boot the hardening Ampel at GREEN - the coherent
    /// everyday factory default (CD-30, D-0047). Regression guard for the stale
    /// CD-25 store seed (`hardening_level` = "standard"), which pre-seeded the
    /// key so the Green `.unwrap_or` fallback was dead and fresh installs
    /// silently came up at Yellow (Green + the timing/media/math clamps). The
    /// key must stay ABSENT on a fresh store; a later explicit choice still
    /// governs, and a persisted highest level still boots as Yellow.
    #[test]
    fn fresh_install_boots_hardening_at_green_not_yellow() {
        let s = Store::open_in_memory();
        // The seed is gone: the key is absent, so the Green default governs.
        assert_eq!(s.get(KEY_HARDENING_LEVEL), None);
        assert_eq!(resolve_hardening_boot_level(&s), Level::Green);
        // An explicit persisted choice is still honored (never overridden by Green).
        s.set(KEY_HARDENING_LEVEL, "yellow");
        assert_eq!(resolve_hardening_boot_level(&s), Level::Yellow);
        s.set(KEY_HARDENING_LEVEL, "off");
        assert_eq!(resolve_hardening_boot_level(&s), Level::Off);
        // A persisted highest level still boots as the resizable Yellow, not the bunker.
        s.set(KEY_HARDENING_LEVEL, "red");
        assert_eq!(resolve_hardening_boot_level(&s), Level::Yellow);
        s.set(KEY_HARDENING_LEVEL, "strict");
        assert_eq!(resolve_hardening_boot_level(&s), Level::Yellow);
    }

    /// CD-31 (D-0048): the Red bunker (window-size lock) is an in-session
    /// choice - any persisted highest level boots as Yellow (full ten-vector
    /// protection, resizable); every other level boots exactly as saved. The
    /// old "strict" name parses to Red first, so it is covered by the same rule.
    #[test]
    fn persisted_red_or_strict_boots_as_yellow_never_the_locked_bunker() {
        assert_eq!(boot_level(Level::Red), Level::Yellow);
        assert_eq!(boot_level(Level::parse("strict").unwrap()), Level::Yellow);
        assert_eq!(boot_level(Level::parse("red").unwrap()), Level::Yellow);
        // Everything below the bunker boots unchanged.
        assert_eq!(boot_level(Level::Yellow), Level::Yellow);
        assert_eq!(boot_level(Level::Green), Level::Green);
        assert_eq!(boot_level(Level::Custom), Level::Custom);
        assert_eq!(boot_level(Level::Off), Level::Off);
    }

    /// Every allowlisted engine round-trips id <-> name; anything else is
    /// rejected by the id side, and the name side resolves the factory default
    /// AND any out-of-range id to DuckDuckGo - never silently Google (CD-27,
    /// D-0043).
    #[test]
    fn engine_allowlist_round_trips_and_default_is_duckduckgo() {
        for name in ["google", "duckduckgo", "bing", "startpage", "brave"] {
            assert_eq!(engine_name(engine_id(name).unwrap()), name);
        }
        assert_eq!(engine_id("altavista"), None);
        assert_eq!(engine_id(""), None);
        assert_eq!(engine_name(DEFAULT_ENGINE), "duckduckgo");
        assert_eq!(engine_name(250), "duckduckgo");
    }
}
