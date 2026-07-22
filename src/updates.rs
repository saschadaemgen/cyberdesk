//! Update awareness (CD-13 → CD-22). The info area shows a REAL up-to-date status for
//! every external dependency by comparing its INSTALLED version against a CLIENT-SIDE,
//! build-time-declared LATEST-KNOWN version ([`COMPONENTS`]) - no live server, no
//! network. Each component reads one of `up to date` / `update available` / `held back`
//! (the held-back overlay is the arti 0.44 case, D-0034), so the panel never shows a
//! bare "INSTALLED" that says nothing about whether the version is current.
//!
//! Installed versions keep their single existing sources and are never restated here:
//! arti from `Cargo.lock` via `build.rs` (D-0029), CEF from the crate's compile-time
//! constants, CyberDesk from `CARGO_PKG_VERSION`. Latest-known is declared in this
//! table and bumped whenever a dependency is - the same maintenance contract as the
//! CD-20 known-issues table, which keeps the honesty rule satisfied without a server.
//!
//! CD-22 RETIRED the live manifest fetch (CD-13/D-0023). The app's own self-update (a
//! real feed at `carvilon.com/updates/...` + hosting) is DEFERRED to a later ticket, so
//! CyberDesk shows clearly-marked DEMO data for now, and the failing fetch + the "Last
//! check failed" footer are gone. The panel is driven entirely from this client-side
//! table. The `update` state is the seed of the future notification rail (Season 7);
//! V1 informs only - it never downloads or installs (that arrives with the signed
//! pipeline, Season 6+).

// The info-panel surface (update_count / info_snapshot_json / init) is wired by the
// CD-13 glyph + panel; keep the API complete even where a build doesn't touch it all.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

// --- Version parsing / comparison -------------------------------------------

/// A tolerant dotted-numeric version: the digits before any `+build` metadata,
/// component by component. Handles our semver (`0.9.0`) and CEF's
/// `major.minor.patch+chromium-...` (only the `major.minor.patch` head matters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(Vec<u64>);

impl Version {
    pub fn parse(s: &str) -> Version {
        // Drop CEF/semver build metadata after '+', a leading 'v', and take the
        // leading digits of each dotted component (so `7827` from `7827.201`
        // survives and any trailing `-rc1` etc. is ignored).
        let head = s.split('+').next().unwrap_or("").trim();
        let head = head.strip_prefix('v').unwrap_or(head);
        let parts = head
            .split('.')
            .map(|p| {
                let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect();
        Version(parts)
    }

    fn cmp(&self, other: &Version) -> std::cmp::Ordering {
        let n = self.0.len().max(other.0.len());
        for i in 0..n {
            let a = self.0.get(i).copied().unwrap_or(0);
            let b = other.0.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                std::cmp::Ordering::Equal => continue,
                non_eq => return non_eq,
            }
        }
        std::cmp::Ordering::Equal
    }

    /// True when `self` is strictly older than `other` (an update is available).
    pub fn is_older_than(&self, other: &Version) -> bool {
        self.cmp(other) == std::cmp::Ordering::Less
    }

    /// True when the two versions are numerically equal under zero-padding, so
    /// `0.44` and `0.44.0` match (derived `PartialEq` would not - different lengths).
    fn is_same(&self, other: &Version) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

// --- Client-side component table (CD-22, generalises the CD-20 known-issues table) ---

/// A held-back overlay on a component's latest-known version: the newest version IS
/// known but is deliberately NOT installed, with a user-facing reason + tracking note.
/// The special case of an "update available" that we intentionally do not take.
#[derive(Debug, Clone, Copy)]
pub struct HeldBack {
    /// Short, plain reason shown to the user (why the newer version is held back).
    pub reason: &'static str,
    /// Short tracking note - what unpins it.
    pub note: &'static str,
}

/// One external dependency's client-declared version facts. `latest_known` is the
/// newest version THIS BUILD knows about - updated whenever the dependency is bumped
/// (the honesty contract). Status is `installed` vs `latest_known`; the optional
/// `held_back` overlay takes precedence (it means the newer `latest_known` is known
/// but deliberately not installed). This generalises the CD-20 arti held-back table:
/// arti is simply the entry whose `latest_known` (0.44) is newer than installed AND
/// carries a `held_back` overlay.
#[derive(Debug, Clone, Copy)]
pub struct ComponentRelease {
    /// Component id, matching the snapshot ids (`cyberdesk`, `cef`, `tor`).
    pub id: &'static str,
    /// The newest version this build knows about (client-declared, build-time).
    pub latest_known: &'static str,
    /// Present iff `latest_known` is a version we know but deliberately do not install.
    pub held_back: Option<HeldBack>,
}

/// The client-side latest-known table. One entry per external component; the SINGLE
/// place a "latest-known" version is declared. Bump each entry when its dependency is
/// bumped (same contract as before). Installed versions are NOT restated here.
pub static COMPONENTS: &[ComponentRelease] = &[
    // CyberDesk - DEMO / PLACEHOLDER (CD-22, D-0036). The app's own self-update (a live
    // manifest feed at carvilon.com + hosting) is DEFERRED to a later ticket; until it
    // is built this is clearly-marked demo data so the panel shows a concrete status
    // instead of a bare "INSTALLED". Set equal to the shipped CARGO_PKG_VERSION → the
    // app reads "up to date" (honest: it does not fabricate a phantom update the user
    // could not get). REPLACE this whole entry with the real feed when it lands; bump
    // it alongside CARGO_PKG_VERSION until then. (To preview the "update available"
    // rendering, temporarily set a newer literal here.)
    ComponentRelease { id: "cyberdesk", latest_known: "0.1.0", held_back: None },
    // CEF core - latest-known = the CEF distribution we pin and vet (D-0002). Equal to
    // the installed crate constants (149.0.6) → "up to date", never a bare "INSTALLED".
    // Bump in lockstep with the CEF crate pin; declaring a newer CEF here before the
    // crate is bumped shows "update available" automatically.
    ComponentRelease { id: "cef", latest_known: "149.0.6", held_back: None },
    // Tor engine (arti) - 0.44.0 is known upstream but HELD BACK (bootstrap regression,
    // D-0034); installed stays 0.43.x. When arti is verified past the regression, in the
    // SAME commit bump `latest_known` and drop the `held_back` overlay (D-0034 revisit).
    ComponentRelease {
        id: "tor",
        latest_known: "0.44.0",
        held_back: Some(HeldBack {
            reason: "arti 0.44.0 has a bootstrap regression: the Tor consensus is fetched but never accepted, so bootstrap stalls at 15%.",
            note: "Pinned to 0.43.x until a later arti is verified to bootstrap on Windows (D-0034).",
        }),
    },
];

/// The declared record for a component id, or `None` if it is not tracked.
fn release_for(id: &str) -> Option<&'static ComponentRelease> {
    COMPONENTS.iter().find(|c| c.id == id)
}

/// The installed version for a component id, from its single existing source (never a
/// second source of truth): CyberDesk = `CARGO_PKG_VERSION`, CEF = crate constants,
/// arti = `Cargo.lock` via `build.rs` (D-0029).
fn installed_for(id: &str) -> String {
    match id {
        "cyberdesk" => current_cyberdesk_version().to_string(),
        "cef" => current_cef_version(),
        "tor" => current_tor_version().to_string(),
        _ => String::new(),
    }
}

/// The status of a component from `installed` vs its declared `latest_known`:
/// - `held_back` - a newer version is known but carries a held-back overlay,
/// - `update` - a newer version is known and is NOT held back,
/// - `current` - installed is at/above the latest-known version (up to date).
/// A component with no declaration is handled by [`component_json`] as `informational`.
fn status_for(installed: &str, rel: &ComponentRelease) -> &'static str {
    let inst = Version::parse(installed);
    let latest = Version::parse(rel.latest_known);
    if inst.is_older_than(&latest) {
        if rel.held_back.is_some() { "held_back" } else { "update" }
    } else {
        "current"
    }
}

// --- Version self-awareness -------------------------------------------------

/// This CyberDesk build's version (from Cargo).
pub fn current_cyberdesk_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The running CEF core version, `major.minor.patch`, from the pinned crate's
/// compile-time constants (verified against `cef 149.3.0`; there is no runtime
/// `cef_version_info` in this binding - the constants are the source of truth).
pub fn current_cef_version() -> String {
    format!(
        "{}.{}.{}",
        cef::sys::CEF_VERSION_MAJOR,
        cef::sys::CEF_VERSION_MINOR,
        cef::sys::CEF_VERSION_PATCH
    )
}

/// The running Chromium version, `major.minor.build.patch`.
pub fn current_chromium_version() -> String {
    format!(
        "{}.{}.{}.{}",
        cef::sys::CHROME_VERSION_MAJOR,
        cef::sys::CHROME_VERSION_MINOR,
        cef::sys::CHROME_VERSION_BUILD,
        cef::sys::CHROME_VERSION_PATCH
    )
}

/// The embedded Tor engine (arti-client) version. Unlike CEF, arti-client exposes
/// NO compile-time version constant (verified against the pinned crate), so
/// `build.rs` reads the resolved version from the committed `Cargo.lock` and injects
/// it as `ARTI_CLIENT_VERSION` (D-0029) - the authoritative running version, not a
/// hand-restated literal that would drift on `cargo update`. This is the
/// arti-client CRATE version (the engine CyberDesk links), not the standalone
/// `arti` CLI nor the Tor network protocol version. `"unknown"` if the build script
/// could not read the lockfile.
pub fn current_tor_version() -> &'static str {
    env!("ARTI_CLIENT_VERSION")
}

// --- Glyph count ------------------------------------------------------------

/// Number of components with an actionable "update available" - the glyph reads this
/// lock-free. Held-back and up-to-date components do NOT light it (nothing to act on).
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// The number of components with a genuine update available - drives the info glyph
/// (fill + count). Zero for the shipped table (all up-to-date / held-back).
pub fn update_count() -> usize {
    COUNT.load(Ordering::Relaxed)
}

/// Derive the glyph count from the static client table + the compile-time installed
/// versions (CD-22). Pure and constant at runtime - no thread, no network, no store;
/// called once at startup. (Replaces the CD-13 background fetch worker, retired in
/// CD-22 - the app self-update feed returns in its own later ticket.)
pub fn init() {
    let n = COMPONENTS
        .iter()
        .filter(|rel| status_for(&installed_for(rel.id), rel) == "update")
        .count();
    COUNT.store(n, Ordering::Relaxed);
    tracing::info!(
        update_count = n,
        "info component statuses derived client-side (no fetch, CD-22)"
    );
}

// --- Info panel IPC payload -------------------------------------------------

/// Build one component's status object for the info snapshot (CD-22). Every tracked
/// component compares its installed version against its declared latest-known one and
/// reports a REAL status - `current` / `update` / `held_back` - never a bare
/// "informational" (which is reserved for an *undeclared* component: a defensive
/// fallback showing the bare version with no claim, since the three tracked ones are
/// always declared in [`COMPONENTS`]). `held_back` carries the `reason` + `note`.
fn component_json(id: &str, name: &str, detail: Option<String>) -> serde_json::Value {
    let installed = installed_for(id);
    let Some(rel) = release_for(id) else {
        return serde_json::json!({
            "id": id,
            "name": name,
            "version": installed,
            "latest": serde_json::Value::Null,
            "status": "informational",
            "detail": detail,
        });
    };
    let status = status_for(&installed, rel);
    let mut obj = serde_json::json!({
        "id": id,
        "name": name,
        "version": installed,
        "latest": rel.latest_known,
        "status": status,
        "detail": detail,
    });
    if status == "held_back"
        && let Some(hb) = &rel.held_back
    {
        obj["reason"] = serde_json::json!(hb.reason);
        obj["note"] = serde_json::json!(hb.note);
    }
    obj
}

/// The `get_info_items` reply: the honest per-component status list (CD-22), built
/// purely from the client table + the running (compile-time) versions. No live feed,
/// no fetch status - the failing manifest fetch and its "Last check failed" footer
/// were retired in CD-22 (the app self-update feed returns in its own later ticket).
pub fn info_snapshot_json() -> String {
    let components = vec![
        component_json("cyberdesk", "CyberDesk", None),
        component_json(
            "cef",
            "CEF core",
            Some(format!("Chromium {}", current_chromium_version())),
        ),
        component_json("tor", "Tor engine (arti)", None),
    ];

    serde_json::json!({ "components": components }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parses_semver_and_cef_formats() {
        assert!(Version::parse("0.1.0").is_older_than(&Version::parse("0.9.0")));
        assert!(!Version::parse("0.9.0").is_older_than(&Version::parse("0.9.0")));
        assert!(!Version::parse("1.0.0").is_older_than(&Version::parse("0.9.9")));
        // CEF format: only the head before '+' matters.
        let cur = "149.0.6";
        let rec = "150.0.1+chromium-150.0.7900.100";
        assert!(Version::parse(cur).is_older_than(&Version::parse(rec)));
        assert!(!Version::parse("150.0.1").is_older_than(&Version::parse(rec)));
        // Uneven component counts compare as if zero-padded.
        assert!(Version::parse("150").is_older_than(&Version::parse("150.0.1")));
        assert!(!Version::parse("150.0.0").is_older_than(&Version::parse("150")));
        // A leading 'v' and trailing junk are tolerated.
        assert_eq!(Version::parse("v2.3.4-rc1"), Version::parse("2.3.4"));
    }

    #[test]
    fn version_is_same_is_zero_padded() {
        assert!(Version::parse("0.44").is_same(&Version::parse("0.44.0")));
        assert!(Version::parse("0.44.0").is_same(&Version::parse("0.44")));
        assert!(!Version::parse("0.44.1").is_same(&Version::parse("0.44.0")));
    }

    // --- Unified installed-vs-latest-known status (CD-22) -------------------

    #[test]
    fn status_reflects_installed_vs_latest_known() {
        let plain = ComponentRelease { id: "x", latest_known: "1.2.0", held_back: None };
        assert_eq!(status_for("1.2.0", &plain), "current"); // equal → up to date
        assert_eq!(status_for("1.3.0", &plain), "current"); // installed newer → still up to date
        assert_eq!(status_for("1.1.0", &plain), "update"); // newer known → update available
        let hb = ComponentRelease {
            id: "x",
            latest_known: "1.2.0",
            held_back: Some(HeldBack { reason: "r", note: "n" }),
        };
        assert_eq!(status_for("1.1.0", &hb), "held_back"); // newer known but held back
        assert_eq!(status_for("1.2.0", &hb), "current"); // once installed catches up → normal
    }

    #[test]
    fn shipped_table_is_honest_for_all_three_components() {
        // Every tracked component is declared → no bare INSTALLED / informational.
        for id in ["cyberdesk", "cef", "tor"] {
            assert!(release_for(id).is_some(), "{id} must be declared in COMPONENTS");
        }
        // CEF: latest-known equals the pinned/installed crate version → up to date.
        let cef = release_for("cef").unwrap();
        assert_eq!(status_for(&current_cef_version(), cef), "current");
        assert!(cef.held_back.is_none());
        // arti: 0.44 known, held back, installed 0.43 → held_back with a reason (D-0034).
        let tor = release_for("tor").unwrap();
        assert_eq!(tor.latest_known, "0.44.0");
        let hb = tor.held_back.expect("arti keeps its held-back overlay");
        assert!(hb.reason.contains("15%"));
        assert!(hb.note.contains("0.43"));
        assert_eq!(status_for(current_tor_version(), tor), "held_back");
        // CyberDesk: demo placeholder equal to the shipped version → up to date, not bare.
        let cd = release_for("cyberdesk").unwrap();
        assert_eq!(status_for(current_cyberdesk_version(), cd), "current");
    }

    #[test]
    fn component_json_never_bare_installed_and_carries_held_back_reason() {
        // CEF: a real status (up to date), never "informational"; version == latest.
        let cef = component_json("cef", "CEF core", Some("Chromium x".into()));
        assert_eq!(cef["status"], "current");
        assert_ne!(cef["status"], "informational");
        assert_eq!(cef["version"], cef["latest"]);
        // arti: held back, with reason + note + the newer latest.
        let tor = component_json("tor", "Tor engine (arti)", None);
        assert_eq!(tor["status"], "held_back");
        assert_eq!(tor["latest"], "0.44.0");
        assert!(tor["reason"].as_str().unwrap().contains("15%"));
        assert!(tor["note"].as_str().unwrap().contains("0.43"));
        // CyberDesk demo: a concrete status (up to date), not a bare INSTALLED.
        let cd = component_json("cyberdesk", "CyberDesk", None);
        assert_eq!(cd["status"], "current");
        // An UNDECLARED component falls back to informational (defensive), bare version.
        let unknown = component_json("mystery", "Mystery", None);
        assert_eq!(unknown["status"], "informational");
        assert!(unknown["latest"].is_null());
    }

    #[test]
    fn update_glyph_counts_only_actionable_updates() {
        // The shipped table has no non-held-back "update available" → glyph idle.
        let n = COMPONENTS
            .iter()
            .filter(|r| status_for(&installed_for(r.id), r) == "update")
            .count();
        assert_eq!(n, 0, "demo / held-back / up-to-date must not light the update glyph");
        // A declared newer, non-held-back version WOULD count (the update path works).
        let synthetic = ComponentRelease { id: "x", latest_known: "9.9.9", held_back: None };
        assert_eq!(status_for("1.0.0", &synthetic), "update");
    }

    #[test]
    fn snapshot_lists_three_components_each_with_a_real_status() {
        let snap: serde_json::Value =
            serde_json::from_str(&info_snapshot_json()).expect("snapshot is valid JSON");
        let comps = snap["components"].as_array().expect("components array");
        assert_eq!(comps.len(), 3);
        // No component renders a bare informational - every one has a comparison result.
        for c in comps {
            let status = c["status"].as_str().unwrap();
            assert!(
                matches!(status, "current" | "update" | "held_back"),
                "component {} must show a real status, got {status}",
                c["id"]
            );
        }
        // The retired live-fetch fields are gone (no misleading fetch status).
        assert!(snap.get("have_feed").is_none());
        assert!(snap.get("feed_ok").is_none());
        assert!(snap.get("checked_ago").is_none());
        assert!(snap.get("items").is_none());
    }
}
