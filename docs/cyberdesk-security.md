# CyberDesk - Security

Project CARVILON CyberDesk - living document - Status: 2026-07-08

## Iron law

The surf zone (CEF) has no path to CARVILON functions (doors, cameras, time clock) by design. No IPC route exists from the web renderer to control commands. Separation by architecture, not by filter.

## Process boundaries and IPC

- Rust host and CEF renderers are separated by a hard process boundary; the Chromium sandbox stays active.
- IPC exclusively through an explicit allowlist of named commands (schema in cyberdesk-wire-format.md, emerging from CD-02).
- No generic eval or passthrough channels.

## Keys and authorization (planned, Season 6)

- Start authorization: passphrase or token -> Argon2id -> key -> encrypted app state is decrypted -> only then does the UI render.
- Zeroize for all key material. No keys in memory before authentication. No key material in the WebView, ever.

## NetGuard

Deny-by-default per zone, destination allowlist, certificate pinning (MITM detection), own DNS resolver (no leak past the system DNS), kill switch per connection, volume and connection counters. Anomaly signals: never-seen destination, beaconing cadence, volume spike outside the baseline, certificate change on a pinned destination, DNS outside the allowlist. Rule-based and explainable first, statistics later. Security alerts run as events through the Priority Engine - the same machinery as the doorbell.

## De-Googled by measurement (CD-17, D-0041)

The host opens no HTTP client of its own (D-0036). CD-17 silences the second
source of unsolicited traffic: the **Chromium engine's own phone-home to Google**.
Every phone-home vector - Safe Browsing (feature + per-navigation lookups),
component updater, variations/Finch seed fetch, connectivity/captive-portal
probes, network prediction, search suggest, domain reliability/NEL, translate,
enhanced spell check, autofill + password leak-check, navigation-error
link-doctor, optimization hints, GCM/push - is disabled via CEF command-line
switches and preferences, applied to **clearnet and Tor slots alike**. Secure DNS
(DoH) is pinned `off` so clearnet uses the OS resolver deterministically; Tor
slots resolve DNS remotely through the tunnel (CD-15). Switch/preference names are
verified against the pinned Chromium `149.0.7827.201` source (the enforcement
table is `src/degoogle.rs`).

**The claim is bounded and measured.** "De-Googled" here means the engine makes
**no unsolicited connection to Google or telemetry**, proven by a net-log capture
on idle and while browsing (the recipe is `cyberdesk-degoogle-audit.md`; the live
run is the maintainer's). It does **not** mean the user's own navigation is hidden
(that traffic goes where the user navigates), nor that zero bytes ever leave.
Necessary TLS infrastructure (OCSP/CRL to a **visited site's own CA**) is **not**
phone-home and is **not** disabled - certificate verification stays on. Metrics
(UMA) and crash upload are off by default (no `crash_reporter.cfg` ships, so no
`ServerURL` and no upload). CD-17 is the precursor and proof for the NetGuard
analyzer epic: host silent + engine silenced + proven.

## Supply chain

Pinned dependencies, cargo-audit and cargo-deny in the workflow, no GPL linking (D-0005), CEF version pinned exactly, large binaries never in the repo (fetch script).

## CRA (Cyber Resilience Act)

Reporting obligations from September 2026, full compliance December 2027. Built in from the start instead of retrofitted: update capability (signed updates planned), SBOM generation, incident logging (hash chain later), documented vulnerability disclosure path.

## Repo hygiene

Pre-push grep against real IPs, hostnames, and secrets before every push. Test data uses placeholders only (documentation IPs such as 203.0.113.x). Repo stays private.
