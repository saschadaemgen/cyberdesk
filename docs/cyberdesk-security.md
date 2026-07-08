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

## Supply chain

Pinned dependencies, cargo-audit and cargo-deny in the workflow, no GPL linking (D-0005), CEF version pinned exactly, large binaries never in the repo (fetch script).

## CRA (Cyber Resilience Act)

Reporting obligations from September 2026, full compliance December 2027. Built in from the start instead of retrofitted: update capability (signed updates planned), SBOM generation, incident logging (hash chain later), documented vulnerability disclosure path.

## Repo hygiene

Pre-push grep against real IPs, hostnames, and secrets before every push. Test data uses placeholders only (documentation IPs such as 203.0.113.x). Repo stays private.
