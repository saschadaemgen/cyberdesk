//! Live application settings — the single source of truth shared between the
//! render loop (main thread) and the settings IPC (CEF UI thread).
//!
//! The SQLite [`Store`] is owned here for the life of the process: Stage A
//! created and seeded it; Stage D hands it to the settings IPC for live writes.
//! The two boolean toggles are mirrored into lock-free atomics so the render
//! loop can read them every frame without touching SQLite.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::store::Store;

/// The persisted key/value store (owned for the process lifetime).
fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::open()))
}

static FEATHER_EDGES: AtomicBool = AtomicBool::new(true);
static DEEP_FIELD: AtomicBool = AtomicBool::new(true);
static STAY_FOREGROUND: AtomicBool = AtomicBool::new(true);

/// The settings keys the internal view is allowed to read and write. Anything
/// outside this list is rejected by [`set`].
pub const KEY_FEATHER_EDGES: &str = "feather_edges";
pub const KEY_DEEP_FIELD: &str = "deep_field";
pub const KEY_STAY_FOREGROUND: &str = "stay_foreground";

/// Open the store and load the persisted toggles into the atomics. Must be
/// called once on the main thread before CEF starts.
pub fn init() {
    let s = store().lock().unwrap();
    FEATHER_EDGES.store(s.get_bool(KEY_FEATHER_EDGES, true), Ordering::Relaxed);
    DEEP_FIELD.store(s.get_bool(KEY_DEEP_FIELD, true), Ordering::Relaxed);
    STAY_FOREGROUND.store(s.get_bool(KEY_STAY_FOREGROUND, true), Ordering::Relaxed);
}

pub fn feather_edges() -> bool {
    FEATHER_EDGES.load(Ordering::Relaxed)
}

pub fn deep_field() -> bool {
    DEEP_FIELD.load(Ordering::Relaxed)
}

pub fn stay_foreground() -> bool {
    STAY_FOREGROUND.load(Ordering::Relaxed)
}

/// Current settings as a JSON object string, for the `get_settings` IPC reply.
pub fn snapshot_json() -> String {
    format!(
        "{{\"feather_edges\":{},\"deep_field\":{},\"stay_foreground\":{}}}",
        feather_edges(),
        deep_field(),
        stay_foreground()
    )
}

/// Apply and persist a single boolean setting. Returns the reply JSON on
/// success, or an error message the IPC turns into a failure. Writes the atomic
/// (seen by the next rendered frame) and the SQLite row (survives restart).
pub fn set(key: &str, value: bool) -> Result<String, String> {
    let atomic = match key {
        KEY_FEATHER_EDGES => &FEATHER_EDGES,
        KEY_DEEP_FIELD => &DEEP_FIELD,
        KEY_STAY_FOREGROUND => &STAY_FOREGROUND,
        other => return Err(format!("unknown setting key: {other}")),
    };
    atomic.store(value, Ordering::Relaxed);
    store().lock().unwrap().set_bool(key, value);
    Ok(format!(
        "{{\"ok\":true,\"key\":\"{key}\",\"value\":{value}}}"
    ))
}
