//! Headless forensic probe (CD-33) — drives the REAL CEF init + browser-creation
//! path with no window, so a filesystem scan can prove what browsing does or does
//! not leave on disk.
//!
//! This is a verification harness, not a product surface: it is reachable only via
//! the `CYBERDESK_FS_PROBE=<url>` environment variable and never appears in
//! `--help`. It deliberately calls `browser::init_cef` / `browser::create_browser_url`
//! rather than re-implementing them — what it verifies is exactly what ships.
//!
//! Copyright (c) 2026 Sascha Daemgen IT and More Systems. All rights reserved.

use crate::browser::{self, Role};
use std::time::{Duration, Instant};

/// Env var naming the URL to load. Unset (the normal case) = probe never runs.
pub const PROBE_ENV: &str = "CYBERDESK_FS_PROBE";
/// Seconds to let the page load/settle before shutdown. `CYBERDESK_FS_PROBE_SECS`.
const DEFAULT_DWELL_SECS: u64 = 12;

/// Run the probe if `CYBERDESK_FS_PROBE` names a URL; returns whether it ran.
///
/// Off-screen (OSR) only — no winit window, no renderer, no Pulse Grid. Loads the
/// URL in slot 0 exactly as the shell would, dwells, then shuts CEF down cleanly so
/// every profile write CEF intends to flush has happened before the scan.
pub fn run_if_requested() -> bool {
    let Ok(url) = std::env::var(PROBE_ENV) else {
        return false;
    };
    if url.is_empty() {
        return false;
    }
    let dwell = std::env::var("CYBERDESK_FS_PROBE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_DWELL_SECS);

    println!("[fsprobe] init_cef");
    crate::settings::init();
    browser::init_identity_seed();
    browser::init_cef();

    // MTML: CEF drives its own UI thread — just wait for the context callback.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !browser::context_ready() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("[fsprobe] context_ready={}", browser::context_ready());

    // OSR needs a size; the parent HWND is monitor/DPI info only, so 0 is fine.
    browser::set_view_geometry(Role::Slot(0), 1280, 720, 1.0);
    println!("[fsprobe] loading {url}");
    browser::create_browser_url(Role::Slot(0), 0, &url);

    std::thread::sleep(Duration::from_secs(dwell));

    println!("[fsprobe] shutdown_cef");
    browser::shutdown_cef();
    println!("[fsprobe] done");
    true
}
