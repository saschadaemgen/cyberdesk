# CyberDesk - Security

Project CARVILON CyberDesk - living document - Status: 2026-07-13

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
switches and preferences, applied to **clearnet and Tor slots alike** — since CD-33
(D-0050) that means the shared ephemeral context and every per-slot Tor context, as
the preferences are per-context and clearnet no longer uses the global one. Secure DNS
(DoH) is pinned `off` so clearnet uses the OS resolver deterministically; Tor
slots resolve DNS remotely through the tunnel (CD-15) — the proxy is a
`socks5://` server, the Chromium proxy scheme that hands the **hostname** to the
proxy rather than resolving it locally, so a Tor slot's visited domains never enter
the OS DNS cache. `dns_over_https.mode` lives in local state, not the profile, so the
CD-33 context change does not touch it. Switch/preference names are
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

## Anonymity-set scope note (CD-28, D-0044)

**Internal engineering scope - never surface in product UI, marketing, or demos.**

A large shared crowd (Tor Browser's uniformity model) structurally helps only
against a **global passive network adversary** that correlates fingerprints
across the whole network: with millions of users reporting identical values,
no fingerprint singles one of them out. CyberDesk's coherent per-session
farbling (CD-16, D-0039) takes the other workable strategy: it breaks
**cross-site and cross-session linkage** - a tracker cannot match today's
fingerprint to tomorrow's - but it does not place the user inside a large
identical crowd. Against the adversaries the product targets (commercial
trackers, cross-site profilers, fingerprint-linkage across sessions), the
farbling model holds on its own merits.

Engineering consequence, not product copy: solve every fingerprint vector
(clamp stable signals, farble measured ones - the CD-29 sweep), and market
each solved vector. The two axes no software can build are crowd size (mass)
and audit reputation (time); they are scope notes here, nothing more.

## CD-29 bounded limits (internal engineering scope, D-0045/D-0046)

**Never surface in product UI, marketing, or demos** (D-0044). These are honest
implementation boundaries recorded for engineering, not product limitations.

- **Fonts are enforced at the JS measurement surface, not the DirectWrite
  backend.** A CEF embedder cannot restrict Chromium's system-font backend, so the
  standard-font guarantee is enforced by stripping non-standard families to the
  generic fallback (canvas `font`, CSS `font-family`/`font`/`setProperty`,
  `FontFaceSet.check`) and reporting no local fonts via `queryLocalFonts`. This
  covers the scripted canvas-measure AND the element-layout (`offsetWidth`) probes
  because both resolve families through `CSSStyleDeclaration`. The pinned standard set
  is the stock-Windows-11 font list; on the sole target platform every user returns
  the same answer. **Remaining step:** bundle the actual font bytes so the guarantee
  holds on a stripped Windows install or a future non-Win11 target (today it relies on
  those fonts being OS-present). A page's own `@font-face` web font (served from its
  origin) is intentionally untouched — only the user's LOCAL fonts are hidden.
- **Automatic rotation is presentation + basis re-seed, not a live-page reset.** It
  re-seeds the global identity for subsequent loads / new windows and drives the Pulse
  Grid countdown, but does not reload live pages (mid-page re-rolling is cosmetic; a
  live document keeps its create-time seed until it is respawned). The manual "new
  identity now" and on-restart are the immediate cross-session-linkage killers. This is
  stated accurately in the UI's honesty copy, so it is not a hidden limit.
- **Screen size cannot be smaller than the real viewport.** An unusually large
  single-column layout on a large monitor reports a larger common ladder rung
  (1440p/2160p) rather than the preset — the exact monitor pixels are withheld, but a
  very large window cannot be made to look small (that would be a detectable decoy).

## CD-32 window-size residual (internal engineering scope, D-0049)

**Never surface in product UI, marketing, or demos** (D-0044). Product copy stays
*tracking-resistance* and never claims the viewport is perfectly hidden below Red.

- **Below Red, reported inner size ≠ the real render area.** The real window is
  deliberately never moved (the user's layout stays free), so the inner-size cluster
  — `innerWidth`/`innerHeight`, the root `clientWidth`/`clientHeight`,
  `visualViewport`, `outerWidth`/`outerHeight`, and the viewport-derived
  `matchMedia` features — is *reported* as the nearest common step of the CD-29
  ladder. The cluster is internally coherent (one shared delta, so no member can
  contradict another — the Brave trap), but **CSS layout still uses the real
  viewport**: a page that measures the rendered pixels of a full-width element, or
  reads `documentElement.scrollWidth`, can still tell reported from real. This is a
  deliberate, bounded tradeoff — a weak, transient, low-entropy vector (users resize
  constantly) traded for layout freedom — and it is **fully closed at Red**, where
  the real window snaps to a common resolution and reported == real.
- **A JS-driven layout can disagree with the page's own CSS breakpoints.** The same
  root cause: `matchMedia` answers for the reported size (it must, or it would
  contradict `innerWidth`), while CSS `@media` rules still evaluate against the real
  viewport, which an embedder cannot rewrite. The paired ladder height makes the
  vertical component of this the visible one (a 1200×1278 column reports 1280×720).
  Accepted below Red; absent at Red, where the delta is zero.
- **Media-query units we do not shift.** `px`/`em`/`rem` and the absolute units are
  converted and shifted. `vw`/`vh` are left alone and are coherent by nature on their
  own axis (self-referential — the same answer for real and reported); `ch`/`ex` and
  `calc()` thresholds are left unshifted, a vanishingly rare incoherence recorded
  rather than guessed at.

## CD-33 anti-forensic residuals (internal engineering scope, D-0050)

**Never surface in product UI, marketing, or demos** (D-0044). What CyberDesk may
accurately say is that it **leaves no browsing trace on disk** — that is now
substantively true for the realistic (Tier-1) attacker and is verified, not asserted.
What follows is what it must **not** claim.

The tiered model this ticket was built against:

- **Tier 1 — the realistic attacker**: someone who later uses the machine, or a
  user-level forensic tool; no kernel or physical tricks. **Defeated**, by two
  independent properties: browsing content is never written to disk, and the OS zeroes
  freed physical pages before any other process can read them. This is the tier that
  matters and the one CD-33 targets.
- **Tier 2 — kernel-privileged or physical live-machine attacker** (kernel driver, raw
  physical-RAM read, cold-boot, DMA): **not closable by any userspace application**,
  ours included. Stated honestly, never oversold — and note the same attacker reads
  live memory *during* use anyway, so this is not a gap CD-33 could have closed. The
  process-isolated security core (separate epic) shrinks the window; it does not
  remove it.

Residuals, precisely:

- **Chromium's C++ heap is not force-wiped on free.** We do not control Chromium's
  allocator, so decoded images and page DOM can persist in freed heap memory for the
  process's lifetime. Covered in practice by the three properties that *do* hold: the
  residue never reaches disk (the whole of Task A), the OS zeroes freed pages before
  reuse (which is what defeats Tier 1), and the future process-kill shrinks the Tier-2
  window. Zeroize applies to *our* memory, not Chromium's.
- **The profile directory still exists** under `root_cache_path`. CEF persists
  installation-specific data there by design and Chromium instantiates its primary
  profile regardless of what our views use. Post-fix it is **empty scaffolding**: a
  `History` file with zero rows, a `Cookies` file with zero rows, and no occurrence of
  any visited host anywhere beneath it (measured). It is not browsing content, but it
  is not *nothing* — a scan will still show Chromium-shaped filenames.
- **Pre-existing residue is not retroactively purged.** The fix stops the writing; it
  does not delete what earlier builds already wrote (on the development machine that
  was 79 MB of cache, 21 URLs / 254 visits, and 36 cookies). `state.db`'s history *is*
  purged by the v7 migration, but the CEF profile residue needs a one-time wipe.
- **The pagefile is addressed by keeping secrets out of it, not by disk encryption.**
  Disk encryption is transparent on a running, unlocked machine and is therefore *not*
  the control against a running-system attacker; the control is that sensitive data
  never reaches the disk in the first place. See the note below on what secrets
  currently exist.
- **Tor state persists by design** (`%LOCALAPPDATA%\CyberDesk\tor\state`). Entry-guard
  persistence is an anonymity *feature* — rotating guards every session raises exposure
  to a malicious guard. It is Tor's own security state, deliberately distinct from
  browsing content, and is not a forensic defect. It does, however, evidence *that* Tor
  was used (not where you went).
- **Session restore is opt-in and honest.** "Quit & Save" persists layout, per-slot
  mode, and URLs — never cookies, cache, or content — so a restored session brings back
  the tabs but **not** the login state; you come back logged out. Plain Quit persists
  nothing. Storing this metadata encrypted-at-rest is open (it is currently plaintext
  `state.db`), and it is the one place a visited URL can reach disk — by explicit user
  action.
- **`favorites` is on disk by intent**, and a favorite is a URL. It records what the
  user chose to keep, not where they have been; this is the bookmark/history split every
  ephemeral browser makes. Worth knowing it is there.

## Supply chain

Pinned dependencies, cargo-audit and cargo-deny in the workflow, no GPL linking (D-0005), CEF version pinned exactly, large binaries never in the repo (fetch script).

## CRA (Cyber Resilience Act)

Reporting obligations from September 2026, full compliance December 2027. Built in from the start instead of retrofitted: update capability (signed updates planned), SBOM generation, incident logging (hash chain later), documented vulnerability disclosure path.

## Repo hygiene

Pre-push grep against real IPs, hostnames, and secrets before every push. Test data uses placeholders only (documentation IPs such as 203.0.113.x). Repo stays private.
