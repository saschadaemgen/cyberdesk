//! Live application settings — the single source of truth shared between the
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
/// layer — see [`crate::store::shared`]).
fn store() -> &'static Mutex<Store> {
    crate::store::shared()
}

static FEATHER_EDGES: AtomicBool = AtomicBool::new(true);
static ANIMATED_BACKGROUND: AtomicBool = AtomicBool::new(true);
static STAY_FOREGROUND: AtomicBool = AtomicBool::new(true);
/// Glow intensity as a whole percent (50..=220). The authoritative default is
/// the `background.glow_default` token, applied in [`init`]; this literal is
/// only a pre-init placeholder.
static GLOW_INTENSITY: AtomicU32 = AtomicU32::new(115);
/// Search engine for the command-bar search fallback, as a small id (0=google,
/// 1=duckduckgo, 2=bing, 3=startpage, 4=brave). The factory default is
/// DuckDuckGo (CD-27, D-0043) — a de-Googled browser must not ship Google as
/// its default search; Google stays a selectable option.
static SEARCH_ENGINE: AtomicU8 = AtomicU8::new(DEFAULT_ENGINE);

/// The factory-default engine id: DuckDuckGo (CD-27, D-0043).
const DEFAULT_ENGINE: u8 = 1;
/// Whether the per-window Tor engine is available at all (CD-15). When off, the
/// toggle glyph does nothing and new windows never default to Tor.
static TOR_ENABLED: AtomicBool = AtomicBool::new(true);
/// Whether new windows open in Tor by default (CD-15). Off by default (clearnet).
static TOR_DEFAULT: AtomicBool = AtomicBool::new(false);
/// The GLOBAL fingerprinting-hardening preset a window inherits (CD-25): 0=off,
/// 1=standard (default), 2=strict, 3=custom. A per-window override lives in
/// browser.rs (`SLOT_HARDENING`); this is the default it falls back to.
static HARDENING_LEVEL: AtomicU8 = AtomicU8::new(1);
/// The custom per-vector flags used only when the level is `custom`. Read at
/// browser-create time and by the frame-state push (not per rendered frame), so a
/// Mutex is fine. Defaults to Standard (all vectors on).
static HARDENING_CUSTOM: Mutex<harden::Config> = Mutex::new(harden::Config::STANDARD);
/// The GLOBAL reported-screen-size preset (CD-29): 0=1280x720, 1=1600x900,
/// 2=1920x1080 (default — the most common real desktop resolution). This is the
/// COMMON value web slots report for `screen.*`; the actual viewport is never
/// faked (browser.rs `common_screen_for` keeps reported ≥ measured). A per-window
/// override lives in browser.rs (`SLOT_SCREEN`).
static SCREEN_PRESET: AtomicU8 = AtomicU8::new(DEFAULT_SCREEN_PRESET);
/// The factory-default reported screen preset: 1920x1080 (id 2).
const DEFAULT_SCREEN_PRESET: u8 = 2;

/// The settings keys the internal view is allowed to read and write. Anything
/// outside this list is rejected by [`set`] / [`set_glow_intensity`].
pub const KEY_FEATHER_EDGES: &str = "feather_edges";
/// The background on/off toggle. Renamed from `deep_field` in CD-05 (D-0012):
/// it now governs whichever background the template selects (Pulse Grid or
/// Deep Field), not the Deep Field specifically. The store migrates the old key.
pub const KEY_ANIMATED_BACKGROUND: &str = "animated_background";
pub const KEY_STAY_FOREGROUND: &str = "stay_foreground";
pub const KEY_GLOW_INTENSITY: &str = "glow_intensity";
/// The command-bar search-engine choice (CD-07). One of the ids below.
pub const KEY_SEARCH_ENGINE: &str = "search_engine";
/// Per-window Tor: the engine master switch and the new-window default (CD-15).
pub const KEY_TOR_ENABLED: &str = "tor_enabled";
pub const KEY_TOR_DEFAULT: &str = "tor_default";
/// Global fingerprinting-hardening preset + custom per-vector flags (CD-25).
pub const KEY_HARDENING_LEVEL: &str = "hardening_level";
pub const KEY_HARDENING_CUSTOM: &str = "hardening_custom";
/// Global reported-screen-size preset (CD-29).
pub const KEY_SCREEN_PRESET: &str = "screen_preset";

/// Glow-intensity slider bounds (percent).
pub const GLOW_MIN: u32 = 50;
pub const GLOW_MAX: u32 = 220;

/// Map a search-engine id string to its small numeric code (and back). The
/// allowlist is defined here — anything outside it is rejected.
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
        // The factory default — and the defense-in-depth fallback for any
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
        harden::Level::Standard => 1,
        harden::Level::Strict => 2,
        harden::Level::Custom => 3,
    }
}
fn level_from_code(c: u8) -> harden::Level {
    match c {
        0 => harden::Level::Off,
        2 => harden::Level::Strict,
        3 => harden::Level::Custom,
        _ => harden::Level::Standard,
    }
}

/// The GLOBAL hardening preset level a window inherits.
pub fn hardening_level() -> harden::Level {
    level_from_code(HARDENING_LEVEL.load(Ordering::Relaxed))
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

/// Apply and persist the global hardening config (CD-25). `level` is one of
/// off/standard/strict/custom; `vectors` supplies the per-vector flags for custom.
/// A WEAKENING change (any vector dropped, or turned off) is refused without
/// `confirm` — the host re-validates the two-confirmation safety gate rather than
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
    let level = s
        .get(KEY_HARDENING_LEVEL)
        .and_then(|v| harden::Level::parse(&v))
        .unwrap_or(harden::Level::Standard);
    HARDENING_LEVEL.store(level_code(level), Ordering::Relaxed);
    if let Some(j) = s.get(KEY_HARDENING_CUSTOM) {
        *HARDENING_CUSTOM.lock().unwrap() = harden::Config::from_json(&j);
    }
    let screen = s
        .get(KEY_SCREEN_PRESET)
        .and_then(|v| screen_code(&v))
        .unwrap_or(DEFAULT_SCREEN_PRESET);
    SCREEN_PRESET.store(screen, Ordering::Relaxed);
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
        "{{\"feather_edges\":{},\"animated_background\":{},\"stay_foreground\":{},\"glow_intensity\":{},\"search_engine\":\"{}\",\"tor_enabled\":{},\"tor_default\":{},\"fp_preset\":\"{}\",\"fp_custom\":{},\"screen_preset\":\"{}\"}}",
        feather_edges(),
        animated_background(),
        stay_foreground(),
        glow_intensity_percent(),
        search_engine(),
        TOR_ENABLED.load(Ordering::Relaxed),
        TOR_DEFAULT.load(Ordering::Relaxed),
        hardening_level().as_str(),
        hardening_custom().to_json(),
        screen_preset_name(),
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
        KEY_TOR_ENABLED => &TOR_ENABLED,
        KEY_TOR_DEFAULT => &TOR_DEFAULT,
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
    use super::{DEFAULT_ENGINE, engine_id, engine_name};

    /// Every allowlisted engine round-trips id <-> name; anything else is
    /// rejected by the id side, and the name side resolves the factory default
    /// AND any out-of-range id to DuckDuckGo — never silently Google (CD-27,
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
