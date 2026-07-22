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
