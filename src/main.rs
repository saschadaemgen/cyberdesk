//! CARVILON CyberDesk — desktop shell entry point.
//!
//! Copyright (c) 2026 Sascha Daemgen IT and More Systems.
//! SPDX-License-Identifier: AGPL-3.0-only (open core; commercial Pro edition licensed apart)

// Release builds are GUI apps (no console window); debug keeps a console for
// logs. CEF sub-processes reuse this same executable.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app;
mod browser;
mod degoogle;
mod forensic;
mod fsprobe;
mod harden;
mod logging;
mod memory;
mod pulsegrid;
mod renderer;
mod settings;
mod slots;
mod store;
mod theme;
mod tor;
mod updates;

use std::process::ExitCode;

const HELP: &str = "\
CARVILON CyberDesk

USAGE:
    cyberdesk [OPTIONS]

OPTIONS:
    --windowed          Start in a 1600x900 dev window (default: borderless
                        fullscreen on the primary monitor).
    --capture <PATH>    Render a single shell-background frame (the Pulse Grid)
                        off-screen to a PNG and exit (visual self-test; does not
                        open a window).
    -h, --help          Print this help.

Press ESC to quit.
";

fn main() -> ExitCode {
    // Timezone normalization (CD-16, D-0039): force the whole process tree to UTC
    // BEFORE anything (Chromium/ICU, logging, threads) initialises. This is the
    // COHERENT way to hide the local timezone — Chromium's `Date` and `Intl` both
    // read the timezone from ICU, which honors the `TZ` env var on every platform
    // (Windows included), so every timezone-derived value agrees (no JS patching, no
    // contradiction). Set here so the browser process detects UTC (its TimeZoneMonitor
    // then propagates UTC to every renderer) and every child inherits it. The rolling
    // log is already UTC (tracing's `SystemTime` timer), so this does not change it.
    //
    // SAFETY: first statement in `main`, single-threaded — no other thread can be
    // reading the environment concurrently (the edition-2024 `set_var` hazard).
    unsafe {
        std::env::set_var("TZ", "UTC");
    }

    // MUST run first: handle CEF sub-processes (renderer/GPU/utility). For a
    // sub-process this never returns; for the browser process it returns here.
    browser::run_subprocess_if_needed();

    // File logging for the browser process (CD-15 HOTFIX) — before anything else so
    // the whole lifecycle (incl. arti bootstrap) is captured. Sub-processes returned
    // above, so only the main process writes the log.
    logging::init();
    tracing::info!("cyberdesk starting");

    let mut windowed = false;
    let mut capture: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--windowed" => windowed = true,
            "--capture" => match args.next() {
                Some(path) => capture = Some(path),
                None => {
                    eprintln!("error: --capture requires a <PATH> argument");
                    return ExitCode::FAILURE;
                }
            },
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown argument '{other}'\n");
                print!("{HELP}");
                return ExitCode::FAILURE;
            }
        }
    }

    if let Some(path) = capture {
        // Default to the dev-window size; `CYBERDESK_CAPTURE_SIZE=WxH` overrides
        // it (e.g. `5120x1440` to eyeball the ultrawide Pulse Grid headlessly).
        let (cw, ch) = std::env::var("CYBERDESK_CAPTURE_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
            })
            .unwrap_or((1600u32, 900u32));
        renderer::capture(&path, cw, ch, &theme::Theme::load());
        println!("wrote {path}");
        return ExitCode::SUCCESS;
    }

    // Headless forensic probe (CD-33): env-gated verification harness, never part
    // of a normal run. Drives the real CEF path off-screen so a filesystem scan can
    // prove what browsing leaves behind. Must precede the update worker (no network
    // beyond the probe's own page) and app::run (no window).
    if fsprobe::run_if_requested() {
        return ExitCode::SUCCESS;
    }

    // Start the background update-awareness worker (CD-13, D-0023): the host's one
    // pinned outbound check, on startup + every interval. Browser process only
    // (sub-processes returned above); never on the --capture path. It never blocks
    // — a bad feed is silent.
    updates::init();

    app::run(windowed);
    ExitCode::SUCCESS
}
