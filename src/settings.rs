//! Live application settings — the single source of truth shared between the
//! render loop (main thread) and the settings IPC (CEF UI thread).
//!
//! The SQLite [`Store`] is owned here for the life of the process: Stage A
//! created and seeded it; Stage D hands it to the settings IPC for live writes.
//! The boolean toggles and the numeric glow-intensity are mirrored into
//! lock-free atomics so the render loop can read them every frame without
//! touching SQLite.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::store::Store;

/// The persisted key/value store (owned for the process lifetime).
fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::open()))
}

static FEATHER_EDGES: AtomicBool = AtomicBool::new(true);
static ANIMATED_BACKGROUND: AtomicBool = AtomicBool::new(true);
static STAY_FOREGROUND: AtomicBool = AtomicBool::new(true);
/// Glow intensity as a whole percent (50..=220). The authoritative default is
/// the `background.glow_default` token, applied in [`init`]; this literal is
/// only a pre-init placeholder.
static GLOW_INTENSITY: AtomicU32 = AtomicU32::new(115);

/// The settings keys the internal view is allowed to read and write. Anything
/// outside this list is rejected by [`set`] / [`set_glow_intensity`].
pub const KEY_FEATHER_EDGES: &str = "feather_edges";
/// The background on/off toggle. Renamed from `deep_field` in CD-05 (D-0012):
/// it now governs whichever background the template selects (Pulse Grid or
/// Deep Field), not the Deep Field specifically. The store migrates the old key.
pub const KEY_ANIMATED_BACKGROUND: &str = "animated_background";
pub const KEY_STAY_FOREGROUND: &str = "stay_foreground";
pub const KEY_GLOW_INTENSITY: &str = "glow_intensity";

/// Glow-intensity slider bounds (percent).
pub const GLOW_MIN: u32 = 50;
pub const GLOW_MAX: u32 = 220;

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
        "{{\"feather_edges\":{},\"animated_background\":{},\"stay_foreground\":{},\"glow_intensity\":{}}}",
        feather_edges(),
        animated_background(),
        stay_foreground(),
        glow_intensity_percent()
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
