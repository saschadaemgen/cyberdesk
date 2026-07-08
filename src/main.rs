//! CARVILON CyberDesk — desktop shell entry point.
//!
//! Copyright (c) 2026 Sascha Daemgen IT and More Systems. All rights reserved.

// Release builds are GUI apps (no console window); debug keeps a console for
// logs. CEF sub-processes reuse this same executable.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app;
mod browser;
mod renderer;
mod settings;
mod store;
mod theme;

use std::process::ExitCode;

const HELP: &str = "\
CARVILON CyberDesk

USAGE:
    cyberdesk [OPTIONS]

OPTIONS:
    --windowed          Start in a 1600x900 dev window (default: borderless
                        fullscreen on the primary monitor).
    --capture <PATH>    Render a single ring frame off-screen to a PNG and exit
                        (visual self-test; does not open a window).
    -h, --help          Print this help.

Press ESC to quit.
";

fn main() -> ExitCode {
    // MUST run first: handle CEF sub-processes (renderer/GPU/utility). For a
    // sub-process this never returns; for the browser process it returns here.
    browser::run_subprocess_if_needed();

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
        // A representative moment in the rotation (gap off the vertical axis).
        renderer::capture(&path, 1600, 900, 3.0, &theme::Theme::load());
        println!("wrote {path}");
        return ExitCode::SUCCESS;
    }

    app::run(windowed);
    ExitCode::SUCCESS
}
