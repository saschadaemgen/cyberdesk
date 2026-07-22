//! Build script - derive the resolved `arti-client` (Tor engine) version from the
//! committed `Cargo.lock` so the update checker (`src/updates.rs`) can report the
//! REAL running Tor engine version.
//!
//! Unlike CEF, which exposes compile-time constants (`cef::sys::CEF_VERSION_*`),
//! `arti-client` exposes no version constant. Rather than restate the version by
//! hand (which silently drifts on `cargo update`), we read the truth from the
//! lockfile and inject it as `ARTI_CLIENT_VERSION`, read via
//! `env!("ARTI_CLIENT_VERSION")` (D-0029). If the lockfile is absent or the package
//! isn't found, we emit `unknown` - this NEVER fails the build.
//!
//! Copyright (c) 2026 Sascha Daemgen IT and More Systems.
//! SPDX-License-Identifier: AGPL-3.0-only (open core; commercial Pro edition licensed apart)

use std::{env, fs, path::Path};

fn main() {
    // Re-run only when the lockfile changes (the version can only move on a resolve).
    println!("cargo:rerun-if-changed=Cargo.lock");
    let version = locked_version("arti-client").unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=ARTI_CLIENT_VERSION={version}");

    #[cfg(windows)]
    embed_icon();
}

/// Embed the application icon in the .exe as a Win32 resource (CD-44 Stage D),
/// so Explorer, the taskbar's pinned entry and the future installer all show
/// the CARVILON mark. The window/taskbar icon at RUNTIME is set separately by
/// the shell (winit, from `assets/cyberdesk.rgba`) - this is the file resource.
///
/// The resource is compiled with the Windows SDK's `rc.exe`, which the MSVC
/// toolchain this project already requires brings along. Like the version
/// probe above, it NEVER fails the build: without the tool the binary is
/// simply icon-less in Explorer, which is a cosmetic loss, not a broken build.
#[cfg(windows)]
fn embed_icon() {
    println!("cargo:rerun-if-changed=assets/cyberdesk.ico");
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let Ok(out_dir) = env::var("OUT_DIR") else {
        return;
    };
    let ico = Path::new(&manifest_dir).join("assets").join("cyberdesk.ico");
    if !ico.exists() {
        println!("cargo:warning=assets/cyberdesk.ico missing; building without an exe icon");
        return;
    }
    let rc_path = Path::new(&out_dir).join("cyberdesk.rc");
    let res_path = Path::new(&out_dir).join("cyberdesk.res");
    // IDI_APPICON = 1: the lowest icon ordinal is what Explorer and the shell
    // pick as the application icon.
    let rc = format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "\\\\"));
    if fs::write(&rc_path, rc).is_err() {
        return;
    }
    let Some(rc_exe) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found; building without an exe icon");
        return;
    };
    let status = std::process::Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
        _ => println!("cargo:warning=rc.exe failed; building without an exe icon"),
    }
}

/// Locate `rc.exe`: on PATH first (a Developer Command Prompt), else the
/// newest x64 Windows SDK build tools under the standard install root.
#[cfg(windows)]
fn find_rc_exe() -> Option<std::path::PathBuf> {
    if std::process::Command::new("rc.exe")
        .arg("/?")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Some("rc.exe".into());
    }
    let root = env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| "C:\\Program Files (x86)".into());
    let bin = Path::new(&root).join("Windows Kits").join("10").join("bin");
    let mut versions: Vec<_> = fs::read_dir(&bin)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("x64").join("rc.exe").exists())
        .collect();
    versions.sort();
    versions.pop().map(|p| p.join("x64").join("rc.exe"))
}

/// Return the `version` of the `[[package]]` block named `name` in `Cargo.lock`.
/// `None` if the lockfile can't be read or the package isn't present - the caller
/// falls back to `"unknown"` rather than break the build.
fn locked_version(name: &str) -> Option<String> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").ok()?;
    let lock = fs::read_to_string(Path::new(&manifest_dir).join("Cargo.lock")).ok()?;
    let want_name = format!("name = \"{name}\"");
    // Cargo.lock always writes `name` before `version` inside a `[[package]]`
    // block, so once we see our package's name line, its version is the next
    // `version = ` line before the block ends.
    let mut in_target = false;
    for line in lock.lines() {
        let l = line.trim();
        if l == "[[package]]" {
            in_target = false;
        } else if l == want_name {
            in_target = true;
        } else if in_target {
            if let Some(rest) = l.strip_prefix("version = ") {
                return Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}
