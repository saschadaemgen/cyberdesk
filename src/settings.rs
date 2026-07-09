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
/// Search engine for the command-bar search fallback, as a small id (0=google
/// default, 1=duckduckgo, 2=bing, 3=startpage).
static SEARCH_ENGINE: AtomicU8 = AtomicU8::new(0);
/// Whether the per-window Tor engine is available at all (CD-15). When off, the
/// toggle glyph does nothing and new windows never default to Tor.
static TOR_ENABLED: AtomicBool = AtomicBool::new(true);
/// Whether new windows open in Tor by default (CD-15). Off by default (clearnet).
static TOR_DEFAULT: AtomicBool = AtomicBool::new(false);

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
        _ => None,
    }
}
fn engine_name(id: u8) -> &'static str {
    match id {
        1 => "duckduckgo",
        2 => "bing",
        3 => "startpage",
        _ => "google",
    }
}

/// The selected search engine: `google` | `duckduckgo` | `bing` | `startpage`.
pub fn search_engine() -> &'static str {
    engine_name(SEARCH_ENGINE.load(Ordering::Relaxed))
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
    let engine = s.get(KEY_SEARCH_ENGINE).and_then(|v| engine_id(&v)).unwrap_or(0);
    SEARCH_ENGINE.store(engine, Ordering::Relaxed);
    TOR_ENABLED.store(s.get_bool(KEY_TOR_ENABLED, true), Ordering::Relaxed);
    TOR_DEFAULT.store(s.get_bool(KEY_TOR_DEFAULT, false), Ordering::Relaxed);
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
    format!(
        "{{\"feather_edges\":{},\"animated_background\":{},\"stay_foreground\":{},\"glow_intensity\":{},\"search_engine\":\"{}\",\"tor_enabled\":{},\"tor_default\":{}}}",
        feather_edges(),
        animated_background(),
        stay_foreground(),
        glow_intensity_percent(),
        search_engine(),
        TOR_ENABLED.load(Ordering::Relaxed),
        TOR_DEFAULT.load(Ordering::Relaxed)
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
