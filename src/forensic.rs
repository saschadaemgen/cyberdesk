//! Anti-forensic on-disk hygiene (CD-34, D-0051) — the permanent browsing-residue
//! safety net.
//!
//! CD-33 (D-0050) stopped browsing content from being *written* to disk: views run
//! under an in-memory request context, and the app's own history moved to RAM. But
//! two things remained true:
//!
//!  1. Residue that older builds already wrote (the measured ~79 MB profile) is not
//!     removed by that fix — it only stops growing.
//!  2. If a future CEF version ever regressed and wrote browsing content to disk
//!     again, nothing would clean it up.
//!
//! This module closes both with a *standing* guarantee rather than a one-time
//! cleanup: on every launch, before CEF opens the profile, it purges the browsing
//! cache/profile directory. Legacy residue is cleared now, and any future accidental
//! leak survives at most one session.
//!
//! ## The allowlist is exactly one path
//!
//! The purge target is the CEF `root_cache_path` — `<exe_dir>/cyberdesk-cache` — and
//! nothing else. That directory is created and owned entirely by CEF for the browsing
//! profile, its caches, and Chromium component data; the app writes nothing of its own
//! there. Everything the ticket says must survive lives in a **different filesystem
//! tree** — `%LOCALAPPDATA%\CyberDesk\{state.db, tor\, logs\}` — which this module never
//! references. Purge target and protected data are therefore disjoint by construction.
//!
//! Deleting the whole directory (not a curated list of sub-paths) is the deliberate
//! choice: a sub-path allowlist would drift as CEF's on-disk layout changes between
//! versions and could silently miss a future leak — the exact failure mode CD-33 was
//! about. One known top-level directory, entirely ours-via-CEF, is both a strict
//! allowlist (of one entry) *and* the strongest guarantee. CEF recreates it cleanly on
//! init (proven by CD-33's probe, which ran from an empty dir every time).
//!
//! ## Why on-launch only
//!
//! The purge runs before `init_cef`. Once CEF opens the profile it holds OS locks on
//! those files, so a mid-session wipe would fail or corrupt. Before init is the only
//! safe moment — and it is sufficient: content this session browses lives in RAM, so
//! the on-disk profile only ever holds regenerable scaffolding until the next launch
//! clears it.
//!
//! Copyright (c) 2026 Sascha Daemgen IT and More Systems.
//! SPDX-License-Identifier: AGPL-3.0-only (open core; commercial Pro edition licensed apart)

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The single top-level directory name CEF is pointed at for the browsing profile /
/// cache (see [`browsing_cache_root`] and `browser::init_cef`). The purge allowlist is
/// exactly this directory under the executable's own folder.
pub const CACHE_DIR_NAME: &str = "cyberdesk-cache";

/// The one allowlisted purge target: the CEF `root_cache_path`, `<exe_dir>/cyberdesk-cache`.
///
/// This is the SINGLE definition of that path — `browser::init_cef` calls it too, so the
/// directory CEF writes to and the directory we purge can never drift apart. `None` if the
/// executable path can't be resolved (in which case the purge safely does nothing rather
/// than guess a path to delete).
pub fn browsing_cache_root() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CACHE_DIR_NAME)))
}

/// Defense-in-depth guard: is `path` a plausible browsing-cache directory that is safe to
/// remove wholesale? Even though [`browsing_cache_root`] derives the path correctly, a
/// future refactor bug must never be able to aim `remove_dir_all` at the exe directory, a
/// drive root, or anything else. If any check fails we do NOT delete — we flag it (per the
/// ticket's "if in doubt, do not delete").
///
/// The rules: absolute path, final component is exactly [`CACHE_DIR_NAME`], and it has a
/// real parent directory (so it can never be a filesystem root).
pub fn is_safe_purge_target(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().and_then(|n| n.to_str()) == Some(CACHE_DIR_NAME)
        && path.parent().is_some_and(|p| p.components().count() > 0)
}

/// The outcome of the last launch-time purge, for the settings readout. Truthful by
/// construction: every field is what actually happened, not a claim.
#[derive(Clone, Debug, Default)]
pub struct PurgeOutcome {
    /// Did the purge run (i.e. was the setting on)? False means residue accumulates.
    pub ran: bool,
    /// Bytes of residue found on disk at launch, before deleting.
    pub found_bytes: u64,
    /// Whether the target directory existed and was actually removed.
    pub cleared: bool,
    /// A non-fatal failure message (e.g. a locked file), if the purge could not fully
    /// complete. `None` on success or when there was nothing to do.
    pub error: Option<String>,
}

/// Process-global record of the launch purge, written once by [`purge_on_launch`] (main
/// thread) and read by the footprint IPC (CEF UI thread).
fn last_purge() -> &'static Mutex<PurgeOutcome> {
    static P: OnceLock<Mutex<PurgeOutcome>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(PurgeOutcome::default()))
}

/// Sum the sizes of every regular file under `path` (recursive). Unreadable entries are
/// skipped rather than aborting — a footprint reading is advisory, never fatal. Returns 0
/// if `path` does not exist.
pub fn dir_size(path: &Path) -> u64 {
    fn walk(path: &Path, acc: &mut u64) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                walk(&entry.path(), acc);
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    *acc = acc.saturating_add(meta.len());
                }
            }
            // Symlinks are neither followed nor counted: the cache dir contains none, and
            // following one could wander outside the allowlisted tree.
        }
    }
    let mut total = 0;
    walk(path, &mut total);
    total
}

/// The guarded delete of one resolved `root`, returning a truthful outcome. Applies the
/// safety guard, measures, deletes, and (on partial failure) recomputes what remains — the
/// whole purge mechanism except reading the setting and resolving the path. Split out from
/// [`purge_on_launch`] so the mechanism is directly unit-testable against a synthetic tree,
/// including the refusal branch (a path that fails the guard must NOT be deleted). `ran` is
/// always true here — the opt-out is decided by the caller before we get a path.
///
/// Error strings are self-describing sentences (they double as the settings readout's
/// "Last launch" value and the log message), so the reader needs no extra framing.
fn purge_dir(root: &Path) -> PurgeOutcome {
    let mut outcome = PurgeOutcome { ran: true, ..Default::default() };

    if !is_safe_purge_target(root) {
        // Failed the guard — flag, never delete (per the ticket's "if in doubt, do not
        // delete"). The directory is left exactly as found.
        outcome.error = Some(format!("refused an unsafe cache path: {}", root.display()));
        return outcome;
    }
    if !root.exists() {
        // Clean already — nothing has landed here since the last launch.
        return outcome; // ran=true, found=0, cleared=false
    }

    outcome.found_bytes = dir_size(root);
    match std::fs::remove_dir_all(root) {
        Ok(()) => outcome.cleared = true,
        Err(e) => {
            // Partial or failed delete (e.g. a file locked by a leftover process). Record
            // the true remaining footprint so the readout never overstates the result.
            let remaining = dir_size(root);
            outcome.cleared = remaining == 0;
            outcome.error = Some(format!("could not fully clear residue: {e}"));
        }
    }
    outcome
}

/// Purge the browsing-cache/profile directory if the setting is on. Called once per launch,
/// on the main thread, BEFORE `browser::init_cef` — the only point at which CEF does not yet
/// hold the profile's files open.
///
/// Records the outcome for the settings readout. Never panics: a delete failure is logged
/// and surfaced honestly rather than taking the app down over disk hygiene.
pub fn purge_on_launch() {
    if !crate::settings::purge_residue() {
        // Opt-out (a weakening the user confirmed through the D-0040 gate). ran=false.
        tracing::info!("browsing-residue purge is OFF (opt-out); on-disk residue will accumulate");
        *last_purge().lock().unwrap() = PurgeOutcome::default();
        return;
    }

    let Some(root) = browsing_cache_root() else {
        // No exe path → no known target. Do nothing rather than guess (never a broad delete).
        tracing::warn!("cannot resolve the browsing-cache path; nothing purged (fail-safe)");
        *last_purge().lock().unwrap() = PurgeOutcome {
            ran: true,
            error: Some("could not resolve the browsing-cache path".to_string()),
            ..Default::default()
        };
        return;
    };

    let outcome = purge_dir(&root);
    match (&outcome.error, outcome.cleared, outcome.found_bytes) {
        (Some(e), _, _) => {
            tracing::error!(path = %root.display(), reason = %e, "browsing-residue purge did not complete")
        }
        (None, true, bytes) => tracing::info!(
            path = %root.display(),
            bytes,
            "purged browsing residue from disk (CEF recreates the profile clean)"
        ),
        (None, false, _) => {
            tracing::info!(path = %root.display(), "no browsing residue on disk (clean launch)")
        }
    }
    *last_purge().lock().unwrap() = outcome;
}

/// Format a byte count as a short human-readable string (for the settings readout).
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    // The loop stops on the true value, but `{:.1}` ROUNDS: a value just under the next
    // boundary (e.g. 1048575 B → 1023.999 KB) would print "1024.0 KB". Promote once more so
    // it reads "1.0 MB" instead. Round to the same 1 decimal the format uses before testing.
    if u < UNITS.len() - 1 && (v * 10.0).round() / 10.0 >= 1024.0 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// The live on-disk browsing footprint + last-purge result, as JSON for the
/// `get_residue_footprint` IPC. Truthful by construction — both numbers are measured.
///
/// `on_disk_*` is the CURRENT size of the browsing-cache directory: the working profile
/// CEF scaffolds while running (regenerable, holding no browsing content — that lives in
/// RAM) and cleared at the next launch. `last_purge` is what the launch purge actually did.
pub fn footprint_json() -> String {
    let on_disk = browsing_cache_root().map(|p| dir_size(&p)).unwrap_or(0);
    let lp = last_purge().lock().unwrap().clone();
    let err = match &lp.error {
        Some(e) => serde_json::Value::String(e.clone()),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "enabled": crate::settings::purge_residue(),
        "on_disk_bytes": on_disk,
        "on_disk_human": human_bytes(on_disk),
        "last_purge": {
            "ran": lp.ran,
            "found_bytes": lp.found_bytes,
            "found_human": human_bytes(lp.found_bytes),
            "cleared": lp.cleared,
            "error": err,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The safety guard accepts a well-formed cache path and rejects the dangerous
    /// look-alikes a refactor bug might produce — the parent (exe) dir, a differently
    /// named sibling, or a relative path. Paths are built from an absolute base so the
    /// test is portable (a `/…` literal is not absolute on Windows, the target OS).
    #[test]
    fn safety_guard_accepts_only_the_cache_dir() {
        let exe_dir = std::env::temp_dir().join("cd34-target").join("release");
        let good = exe_dir.join(CACHE_DIR_NAME);

        // Accept: absolute, named exactly, with a real parent.
        assert!(is_safe_purge_target(&good));

        // Reject: the exe dir itself (wrong final component — this is what a bug that
        // dropped the `.join(CACHE_DIR_NAME)` would produce, and it must never delete).
        assert!(!is_safe_purge_target(&exe_dir));
        // Reject: a differently named sibling.
        assert!(!is_safe_purge_target(&good.with_file_name("cyberdesk-cache-other")));
        // Reject: a relative path (never absolute → refused).
        assert!(!is_safe_purge_target(Path::new(CACHE_DIR_NAME)));
        // Reject: a filesystem/drive root (no correctly-named final component).
        #[cfg(windows)]
        assert!(!is_safe_purge_target(Path::new(r"C:\")));
        #[cfg(unix)]
        assert!(!is_safe_purge_target(Path::new("/")));
    }

    /// The real derived cache path passes the guard and is named as expected — so the
    /// path we hand to `remove_dir_all` in production is one the guard would accept. Asserts
    /// `Some` outright: `current_exe` always resolves in the test binary, so a `None` here
    /// would be a real regression, not a case to skip past (the old `if let` hid that).
    #[test]
    fn derived_cache_root_is_a_safe_target() {
        let root = browsing_cache_root().expect("current_exe resolves in the test binary");
        assert!(is_safe_purge_target(&root));
        assert_eq!(root.file_name().and_then(|n| n.to_str()), Some(CACHE_DIR_NAME));
    }

    /// Drives the real purge mechanism (`purge_dir`, not a raw std call): a full purge of a
    /// synthetic cache tree removes everything under it and reports it honestly, while a
    /// SIBLING tree (standing in for the Tor state / session / config dirs, which live
    /// beside the cache in the real layout) is untouched. The core property: the purge
    /// deletes its target and nothing else.
    #[test]
    fn purge_dir_clears_the_cache_tree_and_leaves_siblings_intact() {
        let base = std::env::temp_dir().join(format!("cd34-test-{}", std::process::id()));
        let cache = base.join(CACHE_DIR_NAME);
        let protected = base.join("CyberDesk-protected"); // stands in for state.db/tor/logs

        // A realistic browsing tree: nested profile dirs + files.
        std::fs::create_dir_all(cache.join("Default").join("Cache").join("Cache_Data")).unwrap();
        std::fs::write(cache.join("Default").join("History"), b"visited-something").unwrap();
        std::fs::write(cache.join("Default").join("Cache").join("Cache_Data").join("data_0"), vec![0u8; 4096]).unwrap();
        std::fs::write(cache.join("Local State"), b"chromium-local-state").unwrap();

        // A protected sibling with its own data.
        std::fs::create_dir_all(&protected).unwrap();
        std::fs::write(protected.join("state.db"), b"favorites+session+seed").unwrap();

        let outcome = purge_dir(&cache);

        assert!(outcome.ran);
        assert!(outcome.cleared, "the cache tree must be reported cleared");
        assert!(outcome.found_bytes > 4096, "found_bytes must reflect the residue size");
        assert!(outcome.error.is_none());
        assert!(!cache.exists(), "the cache tree must be gone");
        assert!(protected.join("state.db").exists(), "the protected sibling must survive");
        assert_eq!(
            std::fs::read(protected.join("state.db")).unwrap(),
            b"favorites+session+seed",
            "protected data must be byte-for-byte intact"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    /// The refusal branch: `purge_dir` handed a path that FAILS the safety guard (not named
    /// `cyberdesk-cache`) must NOT delete it — it flags and leaves the directory exactly as
    /// found. This is the mutation-catcher the coverage review asked for: inverting the
    /// guard, or deleting the wrong path, would fail here.
    #[test]
    fn purge_dir_refuses_a_non_allowlisted_path() {
        let base = std::env::temp_dir().join(format!("cd34-refuse-{}", std::process::id()));
        let not_cache = base.join("important-user-data"); // fails the name check
        std::fs::create_dir_all(&not_cache).unwrap();
        std::fs::write(not_cache.join("keepme"), b"do-not-delete").unwrap();

        let outcome = purge_dir(&not_cache);

        assert!(!outcome.cleared, "a guard-failing path must never be reported cleared");
        assert!(outcome.error.is_some(), "the refusal must be surfaced");
        assert!(not_cache.join("keepme").exists(), "the directory must be left intact");
        assert_eq!(std::fs::read(not_cache.join("keepme")).unwrap(), b"do-not-delete");

        std::fs::remove_dir_all(&base).ok();
    }

    /// `dir_size` sums nested files and returns 0 for an absent path.
    #[test]
    fn dir_size_sums_recursively_and_zero_when_absent() {
        let base = std::env::temp_dir().join(format!("cd34-size-{}", std::process::id()));
        assert_eq!(dir_size(&base), 0, "absent path is 0 bytes");
        std::fs::create_dir_all(base.join("a").join("b")).unwrap();
        std::fs::write(base.join("a").join("f1"), vec![0u8; 1000]).unwrap();
        std::fs::write(base.join("a").join("b").join("f2"), vec![0u8; 2000]).unwrap();
        assert_eq!(dir_size(&base), 3000);
        std::fs::remove_dir_all(&base).ok();
    }

    /// The footprint readout emits valid JSON carrying the exact keys the settings
    /// page reads — locking the wire contract so a rename can't silently break the UI.
    #[test]
    fn footprint_json_has_the_readout_contract() {
        let j: serde_json::Value = serde_json::from_str(&footprint_json()).unwrap();
        assert!(j.get("enabled").unwrap().is_boolean());
        assert!(j.get("on_disk_bytes").unwrap().is_u64());
        assert!(j.get("on_disk_human").unwrap().is_string());
        let lp = j.get("last_purge").unwrap();
        assert!(lp.get("ran").unwrap().is_boolean());
        assert!(lp.get("found_bytes").unwrap().is_u64());
        assert!(lp.get("found_human").unwrap().is_string());
        assert!(lp.get("cleared").unwrap().is_boolean());
        assert!(lp.get("error").is_some()); // null or a string, but always present
    }

    /// Human formatting is honest at the unit boundaries — including the rounding edge
    /// just below a boundary, which must promote (1048575 B is "1.0 MB", never "1024.0 KB").
    #[test]
    fn human_bytes_reads_naturally() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(79 * 1024 * 1024), "79.0 MB");
        // The boundary the review flagged: one byte below 1 MiB must not print "1024.0 KB".
        assert_eq!(human_bytes(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024 - 1), "1.0 GB");
        // A value genuinely in the KB range still reads as KB (no over-promotion).
        assert_eq!(human_bytes(1500), "1.5 KB");
    }
}
