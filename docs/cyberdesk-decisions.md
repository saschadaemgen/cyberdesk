# CyberDesk - Decisions

Newest decision on top. Format: D number - date - decision - reasoning.

## D-0039 - 2026-07-12 - Fingerprinting hardening is coherent tracking-resistance, not anonymity; no OS/UA/platform spoofing (CD-16)

*Decision.* CyberDesk's fingerprinting hardening (CD-16) is scoped as
**tracking-resistance**: coherent **per-session farbling** of readback vectors
(canvas 2D, WebGL readback, audio, client rects, text metrics) so a site cannot link
one session to the next, plus **entropy reduction** on stable attributes
(`hardwareConcurrency`, `deviceMemory`, fonts) and **timezone normalization** to UTC.
The OS, User-Agent, `navigator.platform`, `oscpu`, CPU/OS strings, and language are
left **real and mutually consistent** — no spoofing. It is labelled honestly as
tracking-resistance (settings "Tracking resistance" section + README), never as
Tor-Browser-grade anti-fingerprinting anonymity. It is **always on** and applies
**identically to clearnet and Tor slots** — no toggle, no tier (this supersedes the
older "strong tier auto-engages in Tor windows" phrasing of D-0027).

*Why.* Per EC-01 and the anonymity-set reality: a low-population browser cannot provide
fingerprinting anonymity, and half-done spoofing (Brave's UA/platform mismatch) makes
users *more* unique. The honest, achievable win is breaking cross-session/cross-site
**linkability** with **zero cross-surface contradictions**.

*Mechanism (crate/source-first).* No stable Chromium **pref** covers canvas/WebGL/audio/
rect farbling — Brave patches Blink/C++; a CEF embedder patches the JS surface it owns.
So the sole mechanism is a document-start injection (`src/hardening.js`, embedded via
`include_str!`) executed in the render-side `on_context_created` **before any page
script**, for **web frames only** (`should_harden`: everything except `cyberdesk://`,
`devtools://`, `chrome*://`). Our own `cyberdesk://` UI is never farbled and keeps its
`window.cefQuery` bridge; web frames get the hardening and no bridge. `getParameter`
standardizes only the two `UNMASKED_*_WEBGL` strings to a common **Windows-coherent**
ANGLE/D3D11 Intel GPU (agrees with the untouched Windows UA/platform); every other enum
and all real capabilities pass through.

*Seed channel + determinism.* A single 16-byte OS-random seed (`getrandom`) is generated
once in the **browser** process and appended to **every** child command line as
`--cyberdesk-fp-seed` (`on_before_child_process_launch`); each render process reads it
back from argv (`render_seed`, with a random fallback), so all renderers share one seed.
A fresh launch ⇒ a fresh seed ⇒ a different fingerprint (**cross-session unlinkable**);
within a launch the seed is fixed. The JS derives a per-**first-party-origin** key from
the seed (`location.ancestorOrigins` recovers the top origin even for cross-origin
iframes), so a tracker embedded on two different first parties reads different noise.
Every farble is a **pure function of (origin key, input)**, re-seeded per call and walked
in a fixed order → repeated reads are byte-identical (stable, no flicker, undetectable by
double-read); live audio buffers are farbled once via a `WeakSet`; scalar jitter is keyed
on the value so a single-rect element's `getBoundingClientRect` and `getClientRects()[0]`
stay mutually consistent.

*Timezone via `TZ=UTC` (not JS).* `main` sets `TZ=UTC` as its first statement, before
Chromium/ICU/threads init. This is the **coherent** lever: Chromium's `Date` **and**
`Intl` both read the timezone from ICU, which honors `TZ` on every platform (Windows
included), so every timezone-derived value agrees with **no JS patching and no possible
contradiction**; the browser process detects UTC and its TimeZoneMonitor propagates UTC
to renderers. The rolling log is already UTC (tracing's `SystemTime`), so it is
unaffected. UTC timezone with a real (non-UTC-region) language is **not** a contradiction
— Tor Browser reports UTC for everyone regardless of locale.

*Reasoned deviations (recorded).*
1. **Font enumeration is only partially hardened.** The explicit Local Font Access API
   (`queryLocalFonts`) is neutralized (returns empty), and the metric farbling breaks the
   cross-session **linkability** of width-probe fingerprints — but sub-pixel noise cannot
   hide *which* fonts exist (font presence is a whole-pixel width delta). Full font-set
   standardization needs the Chromium **font backend** (unreachable from document-start
   JS) and is **deferred**, like screen letterboxing.
2. **Worker-context fingerprinting is not covered.** Document-start injection reaches the
   document realm, not Web Worker globals (OffscreenCanvas/analyser inside a worker); a
   C++ patch would be required. The dominant, document-context case is covered.
3. **Screen/window letterboxing deferred** (ticket §4 default — higher breakage).
4. **No anti-detection `toString` masking** of the patched functions: "canvas is patched"
   is a binary signal identical for every CyberDesk user every session, so it does not aid
   cross-session **linking**; leaving it unmasked is simpler, robust, honest, and leaks no
   seed (the seed/keys live only in closure variables, never in any function's source).

*Coherence + no regression.* UA/platform/oscpu/language untouched; WebGL strings and
timezone agree with the real Windows OS; no Brave-style mismatch. WebRTC (CD-15) is **not**
re-implemented and **not** regressed (`disable-quic` + per-context `ip_handling_policy`
stand; the injection touches no WebRTC surface).

*Verification.* A headless Node `vm` harness runs the **actual** `hardening.js` against a
DOM mock and proves: determinism (repeated reads byte-identical; reproducible across runs
at a fixed seed), **cross-session unlinkability** (different seed ⇒ different canvas /
WebGL / audio / rect / text signatures), entropy reduction (cores 12→8, memory→8, empty
`queryLocalFonts`, standardized WebGL strings), coherence (`bcr == getClientRects()[0]`,
sub-visual jitter, inaudible audio delta), and no-throw on missing globals. The live
fingerprint-test runs (coveryourtracks / browserleaks, two-session compare, Tor slots ==
clearnet slots, WebRTC still no-leak) are the maintainer's, per the ticket.

*Dependency.* `getrandom` (OS CSPRNG, minimal, already in the tree via `rand`/arti) — a
randomness library, **not** an HTTP client, so NetGuard (D-0004) is unaffected.

## D-0038 - 2026-07-12 - rustls CryptoProvider must be an explicit `ring` dependency, installed at startup — never left transitive (CD-24; amends D-0036)

*Decision.* The rustls `CryptoProvider` for arti's TLS runtime is an EXPLICIT direct
dependency (`ring`) and is installed at startup via `CryptoProvider::install_default()`
in the Tor engine thread (`tor::run`, first statement, before any arti/TLS construction).
It must never depend on being pulled in transitively by another crate. `Cargo.toml` gains
`rustls = { version = "0.23", default-features = false, features = ["ring"] }` (which
unifies with arti's runtime rustls 0.23.41, so `rustls::crypto::ring` compiles and `ring`
enters the runtime graph), and `tor::install_crypto_provider()` runs
`rustls::crypto::ring::default_provider().install_default()`, treating an
already-installed provider (`Err`) as success. **Not** `aws-lc-rs` — it needs a
C-toolchain path we do not want, and `ring` is the provider already proven working on
this machine.

*The regression (live, CD-22 build).* arti's `TokioRustlsRuntime` needs a process-level
rustls provider. rustls 0.23.41 with **no** provider feature compiled in panics at
provider auto-detection ("Could not automatically determine the process-level
CryptoProvider from Rustls crate features", `rustls-0.23.41/src/crypto/mod.rs:249`). The
provider was historically supplied by `ring`, pulled in transitively via `ureq` (the
Season-1 manifest-fetch HTTP client) and auto-installed. CD-22 (D-0036) removed `ureq`
to honor NetGuard — correct, and it stands — but that silently removed `ring` from the
runtime graph, so the Tor engine thread panicked right after "tor state/cache dirs ready"
and before any bootstrap, taking Tor completely down (SOCKS listeners still bound, but no
arti client behind them).

*Confirmed crate-source-first.* `cargo tree -i ring -e normal` showed `ring` had left the
runtime graph (it remained only as a build-dependency of `download-cef`'s `ureq v3`, and
edition-2024's resolver keeps build-dep and normal-dep features separate). `futures-rustls
0.26` — the runtime path to rustls via `tor-rtcompat`/arti — declares its rustls dep
`default-features = false, features = ["std"]`, i.e. **no provider**. rustls 0.23.41 gates
`pub mod ring` behind `#[cfg(feature = "ring")]`, so the explicit provider install requires
the `ring` feature. After the fix, `cargo tree -i ring -e normal` shows `ring → rustls
0.23.41 → cyberdesk` (single rustls version, unified).

*Amendment to D-0036.* The `ureq` removal STANDS — NetGuard is preserved: `ring` is a
cryptography library, not an HTTP client, so the shipped binary still opens no HTTP client
of its own. The end state is strictly better than before CD-22: the crypto provider is now
explicit and self-installed instead of accidentally transitive, so this coupling can never
regress on a future dependency change. (`aws-lc-rs` stays off — no C-toolchain TLS path.)

## D-0037 - 2026-07-12 - Tor UI status is derived from current engine state and refreshed on every change (CD-23)

*Decision.* The Tor status shown in the UI (the MF Tor-tab status line and the
per-window anonymity indicator) must reflect the engine's CURRENT state, refreshed
whenever it changes and obtainable on demand from a query — it must not be driven solely
by a one-shot push that fires only on user actions. Two concrete guarantees:
- **Refresh on change.** `about_to_wait` compares `tor::status()` against the status
  last carried in a frame push (`Shell.tor_status_pushed`) and re-pushes the frame on a
  transition. The engine reaches READY on a BACKGROUND thread with no user action, so
  without this the frame push (which carries `tor_status` to the per-window anonymity
  indicator in `command.js`) only fires on user actions / while the command band is
  engaged, leaving the indicator latched on "Connecting" while Tor is actually usable.
- **Query returns the live value.** The `get_frame` pull re-stamps the current
  `tor::status()` into the cached payload (`browser::current_frame_state` /
  `restamp_tor_status`), so any (re)created / (re)subscribing consumer (a reloaded
  command band, a new ensemble) gets the correct current state, never a stale
  "Connecting".

*Confirmed mechanism (code-first, per the ticket).* `tor::init()` never resets the
`STATUS` atomic — its "engine already started" branch is a `compare_exchange` no-op that
leaves `STATUS` at `READY`, and `tor::status()` reads that atomic directly. So the Rust
status was already correct, and the MF Tor tab (which polls `tor_status` every second in
`mfzone.js`) was already right. The stuck indicator was the per-window anonymity orb
(`command.js` `.tor-orb`), which is driven by the `tor_status` field of the on-change
frame push (`app.rs::push_frame`) plus the `get_frame` pull. `push_frame` only ran on
user actions and per-frame while the band was engaged, so a `bootstrapping→ready`
transition that happened with the band closed / no user action was never pushed — the
orb and the cached `get_frame` payload kept the last-pushed value. The repeated
`tor::init` calls in the log are legitimate, idempotent triggers (six call sites: slot
creation + Tor toggle), NOT a loop, so no lifecycle change was made.

*Why.* Deriving the status from current engine state (refresh-on-change + a live query)
rather than a latched push makes every consumer — already-loaded and (re)created —
agree with reality and with each other, for any variant of "the status was only
refreshed by a one-shot event". The Tor engine/bootstrap stack (arti 0.43, D-0034) is
untouched: this is purely a UI state-propagation fix.

## D-0036 - 2026-07-12 - External-component update status is client-side; app self-update deferred to demo data (CD-22)

*Decision.* The info area shows a REAL up-to-date status for every external dependency
(CEF, arti) by comparing the installed version against a **client-side, build-time-
declared latest-known version** per component (`updates::COMPONENTS`), yielding `up to
date` / `update available` / `held back`. This generalises the CD-20 known-issues table:
the arti held-back entry becomes the special case — `latest_known` (0.44) newer than
installed (0.43) AND carrying a held-back overlay `{reason, note}` (D-0034). A bare
"INSTALLED" with no comparison is not acceptable for any component with a declarable
latest-known version. Latest-known values are updated whenever a dependency is bumped
(the same maintenance contract as the known-issues table), which keeps the honesty rule
satisfied without a live server. Installed versions keep their single existing sources
(arti from `Cargo.lock` via `build.rs` D-0029; CEF from the crate's compile-time
constants; CyberDesk from `CARGO_PKG_VERSION`) — no second source of truth is added.

*Concretely.* CEF declares `latest_known = "149.0.6"` (= the pinned/installed crate
version, verified crate-source-first) → **up to date**. arti declares `latest_known =
"0.44.0"` + a held-back overlay → **held back** (unchanged behaviour, now via the
unified model). CyberDesk declares a clearly-marked **demo** `latest_known = "0.1.0"`
(= the shipped version) → **up to date**, an honest placeholder that does not fabricate
a phantom update. The glyph counts only genuine (non-held-back) `update available`
components; the shipped table has none, so the glyph is idle.

*App self-update deferred.* The CyberDesk app's own self-update (a live manifest feed at
`carvilon.com/updates/...` + hosting) is **deferred** to a dedicated later ticket. Until
it is built, the app shows the demo data above so it renders a concrete status rather
than a bare "INSTALLED". The failing live manifest fetch and the "Last check failed"
footer are **retired**: the background fetch worker, the `Manifest` structs, the
`dismiss_item` / `check_updates` IPC commands, AND the `ureq` runtime HTTP client
(D-0023's reasoned lean-dependency exception) are removed; the panel is driven entirely
from the client-side table and is read-only (one command, `get_info_items`). The shipped
binary now opens **no HTTP client of its own** — NetGuard (D-0004) holds without
exception until the self-update feed returns (the transitive `ureq` left in `Cargo.lock`
is a build-time-only CEF-download tool, never linked into the running shell). The
manifest JSON format is kept in the wire-format doc as the reference for the future
feed. When it returns, the client table stays the source of truth for held-back
versions (the pin is a build-time decision, D-0034).

*Why.* For an update area, a bare "INSTALLED" says nothing about whether the version is
current, old, or superseded — useless. Deriving the status from a client-declared
latest-known version answers "am I on the current version?" per component, honestly and
without a live server, and cleanly retires a fetch that can only fail until the feed
exists.

## D-0035 - 2026-07-11 - Shell session lifecycle: default two-slot startup, two quit modes, session captures per-slot mode (CD-21)

*Decision.* CyberDesk's session lifecycle is now defined end to end.
- **Default startup** (empty session, first run, or after a plain quit): two browser
  slots side by side — clearnet (**left**) + Tor (**right**), both loading the own
  start page `cyberdesk://start/`. Clamped to what the frame can hold (big-monitor
  focus): two where they fit, else a single clearnet slot on a narrow monitor. The
  right slot is Tor only when the engine master switch (`tor_enabled`) is on;
  otherwise both are clearnet (an honest fallback — a disabled engine cannot open a
  Tor window). Side assignment left-clearnet / right-Tor is the planning-chat choice.
- **Own start page renders in a Tor slot** (the CD-14 asset existed and `//start`
  was already dispatched, so this was NOT a missing page). Root cause: the internal
  `cyberdesk://` scheme-handler-factory was registered only on the GLOBAL request
  context; a Tor slot runs under its OWN per-slot `CefRequestContext`, which does not
  inherit it, so `cyberdesk://start/` returned `ERR_UNKNOWN_URL_SCHEME` there ("no
  usable start page in the Tor window"). Fix: register the same in-process factory on
  each per-slot Tor context in `build_tor_context`. The page is served in-process with
  ZERO network egress, so it renders before/without arti being bootstrapped, and
  fail-closed still holds (nothing leaves the machine).
- **Two application-quit controls**, labelled **"Quit"** and **"Quit & Save"**, live
  as persistent chrome in the permanent top-right **MF-zone** view (`cyberdesk://
  mfzone/`), shown under every tab. They are deliberately distinct from the CD-18
  per-slot close icon (which closes one window). **Quit** = no save (default layout
  next launch); **Quit & Save** = persist the full state, restored exactly next
  launch. Escape-in-settings and the OS window-close remain as silent accelerators
  (they quit without saving); they are no longer the only quit path.
- **Session (schema v6, session_slots RETURNS)** captures per slot: mode (Tor /
  clearnet), URL, width, active, and display order. Restore is exact and is a
  **one-shot, opt-in** flow: `save_session` writes the rows and sets a `meta`
  `session_savequit` flag ONLY on "Quit & Save"; `take_saved_session` restores once
  and CONSUMES the flag, so a subsequent plain quit or a crash boots the default
  layout. A restored Tor slot returns as a REAL Tor slot (its mode set before the
  browser is created, so it is spawned under its proxied context). Unknown/old
  schemas migrate to an empty table → default layout, never a crash.

*Placement note (flagged for vision-law).* The shell has **no text-rendering engine**
(only SDF shapes + a 7-segment digit, `info_glyph.wgsl`), so the German/English quit
labels cannot be shell-drawn — they must live in an HTML internal view. The permanent
MF-zone view is the least-invasive home (always composited, text-capable, input-routed
as `Role::MfZone` with no focus-steal, top-right region, zero new view/geometry/render
plumbing). This is a minimal new pattern (persistent chrome inside the MF zone rather
than a free-floating glyph); it should be checked against `docs/cyberdesk-vision-law.md`
once that file lands (still uncommitted, Season-1 open item).

*Language note (deviation from the ticket, honoring the binding repo rule).* The ticket
requested German labels ("Beenden" / "Beenden & sichern"). `CLAUDE.md` sets a permanent
rule — "English everywhere in the repo… Admin UI language is English." On Sascha's
explicit instruction the buttons ship in **English** ("Quit" / "Quit & Save"), keeping
the permanent rule intact.

*Privacy (reconciles with D-0025).* CD-14/D-0025 removed website persistence entirely.
CD-21 reintroduces it **opt-in only**: nothing is written unless the user chooses
"Quit & Save". Even then, a **Tor slot's URL is never written to disk** (it restores on
the start page, still as a Tor slot), and internal/blank slots persist an empty URL
(`memory::is_recordable`) — only real clearnet site URLs reach `state.db`, and only on
that explicit opt-in. On Sascha's instruction Tor slots restore to the start page
rather than their exact URL, so nothing browsed under Tor touches the local disk.

*Why.* The prior behaviour lost window mode and arrangement on restart, the start page
did not render (worst in the Tor window), and the only way to quit was Escape inside
settings. This defines an intuitive, honest lifecycle with explicit user control over
whether an arrangement is kept.

## D-0034 - 2026-07-11 - arti pinned to 0.43.x; arti 0.44 has a bootstrap regression; arti/`tor-*` crates must be versioned in lockstep (supersedes the D-0033 "compression fixed it" conclusion)

**Decision.** Pin all arti and `tor-*` crates to `0.43.x`. Do not move to `0.44.x`
until a later arti release is verified to bootstrap on Windows. `arti-client` and
`tor-rtcompat` are BOTH direct dependencies and must always carry the same version —
never mix arti versions in the graph. The working `Cargo.toml` lines:
`arti-client = { version = "0.43", default-features = false, features = ["tokio", "rustls", "compression"] }`
with `tor-rtcompat = "0.43"` (and any other `tor-*` direct deps on 0.43). Verify a
single version with `cargo tree -i tor-rtcompat` before building.

*The bug (observed on Sascha's Windows machine, arti 0.44.0).* Tor bootstrap reaches
15% ("connecting successfully; directory is fetching a consensus") and never advances.
Everything up to that point is healthy: channels build (TLS handshake completes,
VERSIONS / CERTS / AUTH_CHALLENGE / NETINFO exchanged), the one-hop directory circuit
builds (CREATE_FAST → CREATED_FAST). The consensus IS downloaded — ~1576 inbound RELAY
data cells (~785 KB, consensus-sized) arrive, with Sendme flow-control cells sent to
keep the stream window open — and `tor-dirmgr` logs `received 1 useful responses from
our requests`, so the document is received and parsed. But the consensus is then
**never accepted**: dirmgr stays in "Looking for a consensus", status stays at 15%, the
idle circuits are reaped ("Circuit expired for being too dirty or old"), arti issues a
`FirstHopClockSkew` query, and after 90 s the bootstrap times out with no error path
(because this is not an error path — it is a silent non-acceptance).

*Causes ruled OUT, each tested, not assumed.*
- Network blocking — the official Tor Browser bootstraps fine on the same machine.
- Missing `rustls` CryptoProvider — refuted crate-source-first (ring is the single
  provider, auto-installed; it would panic earlier).
- Missing `compression` feature — added and confirmed present in the resolved graph via
  `cargo tree`; behaviour was byte-for-byte identical afterwards, so `compression` was a
  real missing default but **NOT** this cause. **This supersedes the D-0033 conclusion**
  that `compression` fixed the stall: D-0033 still stands in that `compression` is
  REQUIRED (keep it), it just was not what unblocked bootstrap — the arti version was.
- Clock skew — measured `+13 s` via `w32tm /stripchart` on the real machine, far inside
  arti's tolerance. The `FirstHopClockSkew` line is a routine query, not proof of skew.

*How the root cause was isolated — bisection against the SimpleGoX reference.* SimpleGoX
(also Rust + arti, run on the same machine, using `native-tls` + `PreferredRuntime`) was
version-bisected: it bootstraps cleanly on arti `0.41` and `0.43`, and reproduces the
identical 15% stall on `0.44`. Because SimpleGoX uses `native-tls` and `PreferredRuntime`,
the TLS backend and the runtime are both ruled out — the ONLY variable that flips
working/broken is the arti version. Conclusion: arti 0.44 has a bootstrap regression in
which the consensus is fetched but never accepted. CyberDesk on 0.43 bootstraps to Ready
and passes the full leak checklist (Tor exit IP confirmed, no WebRTC IP leak, per-window
clearnet isolation confirmed).

*Mechanical lesson (cost 178 compile errors once).* Pinning only `arti-client` to 0.43
while `tor-rtcompat` stayed at 0.44 put two `tor_rtcompat` versions in the graph,
producing a trait-bound mismatch (`Runtime` / `UdpProvider` not satisfied for
`TokioRustlsRuntime`, "multiple different versions of crate tor_rtcompat"). Pin
`tor-rtcompat` to 0.43 as well and verify a single version with `cargo tree -i
tor-rtcompat` before building.

*Info-area consequence (CD-20).* Because 0.44 exists upstream but is deliberately not
installed, the update area must NOT (a) call 0.43 "current" (a newer version exists) nor
(b) push the user toward the broken 0.44. CD-20 adds a distinct **`held_back`** version
state driven by a client-side known-issues table (`updates::KNOWN_ISSUES`) seeded with
exactly this arti 0.44.0 entry. The pin is a build-time decision (`Cargo.toml`), so the
annotation's source of truth is the client table, not the live manifest.

*Revisit trigger.* When arti `0.45` (or later) ships, test bootstrap-to-Ready on Windows
before bumping; if it works, bump all arti/`tor-*` in lockstep and **remove the arti
entry from the CD-20 known-issues table (`updates::KNOWN_ISSUES`) in the same commit** so
the info area returns arti to a normal state automatically.

## D-0033 - 2026-07-10 - CD-15 Tor fix: the arti `compression` feature is REQUIRED for bootstrap (confirmed root cause of the consensus-fetch stall)

The confirmed root cause of the Tor bootstrap stall that HOTFIX 2/3 chased —
cross-checked against the working SimpleGoX arti integration (which bootstraps cold
in ~30 s with `compression` present).

**Root cause.** We built `arti-client` with `default-features = false, features =
["tokio","rustls"]`, which DROPPED `compression` — a DEFAULT arti-client feature.
Verified crate-source-first (arti-client 0.44.0 `Cargo.toml`): `compression =
["tor-dirmgr/compression"]` and `default = ["compression", …]`. Tor directory caches
serve the **consensus compressed** (zstd / lzma / zlib); arti advertises it accepts
those encodings, the cache replies compressed, and without `compression` compiled in
arti has **no code path to inflate the body**. This is exactly the traced signature
(D-0032): channels open, handshakes complete, dir circuits build → 15%, the request
goes out, the compressed response can't be consumed, the fetch never completes, and
the idle circuits expire unused at ~60 s — with no error line, because the capability
simply isn't there. It is why official Tor Browser connects on the same machine and
ours did not: a feature-gate difference, nothing else. (The HOTFIX 3
`first_hop_clock_skew` H2 was a local-sandbox red herring, correctly flagged as
confounded — the sandbox has no reachable Tor network.)

**Fix + REQUIREMENT (do not regress).** Add `compression` to the arti-client
features. It is **required for Tor bootstrap** — dropping it silently re-introduces
this exact 15% stall. This joins the D-0031 arti requirements: keep `arti-client` /
`tor-rtcompat` at `default-features = false, features = ["tokio","rustls",
"compression"]`; keep `type Client = TorClient<TokioRustlsRuntime>`; keep the runtime
`new_multi_thread().enable_all()`. **Build note:** `compression` pulls in the zstd/xz
codecs (`zstd-sys` is C-backed), so the build needs a **C toolchain (MSVC)** — already
present because CEF requires it; if a build ever errors on the codecs, that C
toolchain is the cause (do NOT work around it by dropping `compression`). Fail-closed
unchanged. HOTFIX 3's dir-fetch tracing stays (harmless, useful). This unblocks the
real CD-15 acceptance: Sascha's networked run reaching Ready + the leak checklist.

## D-0032 - 2026-07-10 - CD-15-HOTFIX-3: trace the directory-fetch layer; stall pinned to the "circuit built → consensus request" handoff (never issued); no arti bump possible

HOTFIX 2 narrowed the Tor stall to the consensus fetch (arti reaches "15%:
connecting successfully; directory is fetching a consensus" then stalls). HOTFIX 3
made that step observable and audited arti's dir-fetch path crate-source-first.

**The silent step, now traced.** The crate that actually issues the HTTP-over-Tor
directory request — `tor_dirclient` — was not in our trace filter, and its whole
lifecycle logs at TRACE (`get_resource` is `#[instrument(level="trace")]`). Added
`tor_dirclient` + `tor_memquota` to the `CYBERDESK_TOR_TRACE` target set, and — the
key fix — `CYBERDESK_TOR_TRACE=1` now maps to **trace** (debug was insufficient to
see the silent step; pass `=debug` explicitly for the lower verbosity). `tor_dirmgr`
already covers its `::state`/`::bootstrap` submodules by prefix.

**The exact arti call chain after circuits build** (all in the pinned 0.44.0
sources): `tor_dirmgr::bootstrap::fetch_single` → `tor_dirclient::get_resource`
(lib.rs:159, the sole request issuer) → `circ_mgr.get_or_launch_dir()` (get the
one-hop dir circuit) → `req.check_circuit()` → `tunnel.first_hop_clock_skew()`
(tor_proto circuit.rs:512, a reactor ctrl message + `rx.await`) →
`begin_dir_stream()` (5 s, optimistic) → `send_request` (writes the GET) → read
(10 s). There are exactly four awaits between "circuit built" and "request on the
wire"; only ONE is unbounded and silent: `first_hop_clock_skew()`.

**What the trace showed (reproduced locally, same signature as Sascha's log).**
Channels open to 3 fallback dir relays, Tor link handshakes complete, the one-hop
dir circuits BUILD ("received CREATED_FAST" → "Handshake complete; circuit
created") — then `tor_dirclient` never advances to `begin_dir_stream`, so the
consensus request is NEVER issued and the built circuit is reaped unused at ~60 s
("DESTROY: too dirty or old"). Discriminated to **H2**: `get_resource` reaches
`check_circuit → first_hop_clock_skew()`; the circuit reactor RECEIVES the
`FirstHopClockSkew { answer }` ctrl message but never completes the `answer` sender,
and no `BEGIN_DIR` cell is ever sent (0 markers). The H1 signature (a
`tor_circmgr` ~60 s "All tunnel attempts failed due to timeout" warn) was ABSENT —
consistent with the silent, no-retry character.

**Honest scope — no root-cause code fix shipped, deliberately (do-not-guess rule).**
(a) No version bump is possible: arti-client 0.44.0 is the LATEST published release
(Arti 2.5.0, 2026-06-30 — no 0.45.0/0.44.1), verified on crates.io + the arti
CHANGELOG mirror; no changelog documents a matching dir-fetch/bootstrap fix (none
can — nothing newer exists). (b) No config gate: our `tor_config()` sets only the
state/cache dirs (D-0028); everything else is `TorClientConfig` defaults;
`tor_memquota` is disabled by default; the fallback-dir ORPorts (:8443/:9001) are
not restricted; the RW store lock is owned (reaching "fetching a consensus" proves
it). (c) The hang is inside arti's reactor, and since arti 0.44.0 bootstraps
normally for the general population, the local H2 repro is very likely CONFOUNDED by
the sandbox (future clock / restricted network) — so it is NOT asserted as Sascha's
cause. Sascha's `CYBERDESK_TOR_TRACE=1` (now trace) log will pin his exact case
(H1 vs H2 vs request-sent-no-response), which decides the fix; if it is the arti
`first_hop_clock_skew` reactor hang, that is an arti issue to file (no fix our side,
no newer arti). **arti runtime/TLS/version requirements (D-0031) stand; the pin is
0.44.0 = latest.** Fail-closed intact throughout.

## D-0031 - 2026-07-10 - CD-15-HOTFIX-2: deep arti bootstrap instrumentation; runtime/TLS proven correct, stall narrowed to the consensus fetch; runtime hard-pinned to rustls

Tor never reached Ready on Sascha's machine, yet official Tor Browser connects and
clearnet browsing works there — so the network is fine and this is our arti
integration. The old log (arti at info only) showed one line — "Looking for a
consensus" — then 90 s of silence and a timeout that wrongly blamed the network.

**Crate-source-first audit refuted the obvious suspects.** A workflow read
tor-rtcompat 0.44 + rustls 0.23.41: the rustls CryptoProvider is NOT missing — the
single unified rustls is compiled with exactly one provider (`ring`, forced on by
ureq; `aws-lc-rs` absent from the lock), so `ClientConfig::builder()` auto-installs
it and never panics; and if it couldn't, it would panic BEFORE "looking for a
consensus", which we do reach. TLS is not the cause.

**Deep instrumentation (Stage A) — now the diagnostic is conclusive.** (1) A
`CYBERDESK_TOR_TRACE=1` env toggle raises the arti/tor crates (`arti_client`,
`tor_dirmgr`, `tor_chanmgr`, `tor_proto`, `tor_circmgr`, `tor_guardmgr`,
`tor_netdir`, `tor_netdoc`) from info to debug/trace — the steps after "looking for
a consensus" (channel connect, guard pick, TLS handshake, circuit build) live below
info, which is why the old log looked like "one attempt then silence". (2) Subscribe
to arti's `bootstrap_events()` stream (`create_bootstrapped` split into
`create_unbootstrapped_async()` + `bootstrap()`, behaviour-preserving) and log every
phase (`<pct>%: <conn>; <dir>`) + any `Blockage` (Offline/Filtering/ClockSkewed/
CantReachTor) verbatim. (3) A panic hook routes swallowed spawn-panics to the log.
(4) The timeout now reports arti's real last status — the false "network blocking
Tor?" is gone.

**What the instrumentation revealed (reproduced locally).** arti progresses
`0% → 8% handshaking with Tor relays → 15% connecting successfully; directory is
fetching a consensus`, then the DIR phase never advances; debug shows completed
`tor_proto::channel::handshake`, `tor_circmgr::build: Spawning reactor`, then
repeated `tor_proto: Received DESTROY cell. Reason: Circuit expired for being too
dirty or old`. So: the runtime DRIVES arti correctly (progress observed), the TLS
handshake WORKS (reaches 15%), circuits build — and the stall is specifically the
**consensus/directory fetch** completing, after which circuits age out and churn.
This refutes ALL of the briefing's suspects (runtime handle, enable_all, TLS, lazy
client) by direct evidence. Sascha's `CYBERDESK_TOR_TRACE=1` 90 s log will name his
exact cause (his clock is fine since Tor Browser works, so likely the consensus
download stalling — the local repro is confounded by the sandbox environment).

**Runtime/TLS hardening (Stage B) — the one actionable fix + a regression guard.**
`type Client` and the client are now built on an EXPLICIT
`tor_rtcompat::tokio::TokioRustlsRuntime` (via `current()` from the same block_on
runtime, then `TorClient::with_runtime(rt)`), not the opaque `PreferredRuntime`.
Reason: **`PreferredRuntime` silently prefers native-tls if any crate in the tree
enables tor-rtcompat's `native-tls` feature** (Cargo global feature unification) —
a real latent risk that would swap our TLS backend with no compile error. Naming
the runtime removes that risk AND makes it `Debug`-loggable (the log now prints
`runtime=TokioRustlsRuntime { .. }`). ARTI RUNTIME/TLS REQUIREMENTS (do not regress
on a bump): keep `arti-client`/`tor-rtcompat` at `default-features=false, features=
["tokio","rustls"]`; keep `type Client = TorClient<TokioRustlsRuntime>`; keep the
runtime `new_multi_thread().enable_all()` (io+time) — arti needs both drivers;
never let `native-tls` into the tree without re-pinning.

Fail-closed intact throughout (no direct fallback). This precedes CD-15 leak
acceptance: after Sascha's confirming run reaches Ready, the leak checklist runs.

## D-0030 - 2026-07-10 - CD-18: MF-zone tabbed viewer (first real content), log ring buffer, per-window Tor+close icons, complete Tor settings

"Make Tor fully visible and controllable." Five parts, all in the token design,
mouse-first, floating-consistent. Independent of the CD-15 leak acceptance — it only
needs the CD-15-HOTFIX logging and the CD-11 MF zone, and it actively HELPS CD-15
(the Tor tab makes bootstrap status visible live instead of hunting date-suffixed
log files).

**1 — In-memory log ring buffer (the viewer's data source, not file tailing).** A
bounded `tracing` Layer (`RingLayer`) keeps the last ~2000 records
{seq, ts_ms, level, sev, target, msg} in one `VecDeque` under a Mutex; `logging::
init()` moved from the `fmt()` facade to `registry().with(filter).with(fmt).with
(ring)` so the ring and the rolling file share ONE `EnvFilter` — the ring captures
exactly what the file does. Records store a severity RANK (0=TRACE..4=ERROR) so
`level_min` is a plain `sev >= min`, dodging tracing's INVERTED `Level: Ord`. The
visitor copies ONLY the `message` field (never other structured key/values), so the
no-secrets rule holds doubly now that a UI surfaces the log. IPC `get_log_lines
{filter?, since_seq?}` reads the ring in-process — no date-suffix race. (Do NOT tail
the file from the UI.)

**2 — The MF zone's first real content: a tabbed live viewer.** A new opaque OSR
view `Role::MfZone` (a third view kind; the per-view array grew to `MAX_SLOTS+2`) is
composited into the permanent right MF rect, reusing the slot page pipeline (its own
`PageTarget`, an unconditional page-uniform write from `sides[1]`), and replaces the
rows-glyph placeholder once it has a texture. It is `cyberdesk://mfzone/` — the same
scheme lock + web isolation + message-router IPC as settings/info/start (the CD-14
broadened `cefQuery` forwarding covers it for free). It is created eagerly like the
Internal view but NOT focused (`on_after_created` guard) so it never steals Slot(0)'s
keyboard focus; it is mouse-driven, routed via `disp_right.contains` in `mouse_target`,
and because slot-activation is gated to `Role::Slot` an MF click never changes the
active slot. Geometry is set on resize only (constant texture; the X animates via the
render NDC rect, like slots). **Tabs:** Tor (default) — a Connecting/Ready/Failed+reason
status header over the streaming tor/arti log (the CD-15 diagnostic surface); Log — a
live tail of the full log with an info/debug filter, Copy, and auto-scroll that pauses
on scroll-up; Terminal — a **reserved** placeholder (a real PTY terminal is a Season-8
tools item; the tab exists so the switching UX is complete).

**3 — Per-window icons consolidating scattered controls.** Two always-present icons
sit to the right of each ensemble's address capsule: the **anonymity/Tor icon** (the
CD-15 shield, relocated here, gaining a distinct "ready" state; toggles Tor for that
window) and a new **close icon** (a `close_slot` IPC → main-thread `close_slot_at`,
which enforces last-slot-refuses — the single choke point shared with Ctrl+W). The
CD-12 shell-drawn corner-hover close orb is RETIRED (its helpers, the `close_hover`
field, the overlay-pass close branch, the `CYBERDESK_CAPTURE_CLOSE` knob, and the
`close_size` token all removed; the now-producerless `drag.wgsl kind:1` ring+cross
branch is left in place, harmless). No duplicate controls; ESC chain + Ctrl+W intact.

**4 — Complete Tor settings + information.** The section now carries every option as
a clean control plus all info: the engine master switch and "Tor for new windows"
(CD-15), the live status incl. the Failed reason (hotfix), the embedded engine
version ("arti <ver>", honestly the arti-client crate, not the Tor CLI), a
circuit-isolation note, a working **New circuit / new identity** button, and a
reserved **Bridges — coming** entry (the real fix for a Tor-blocking network is a
separate follow-up ticket; only the place is held). "New identity": arti-client 0.44
exposes no single global new-identity call, so `tor_new_circuit` bumps a
`NEW_IDENTITY_EPOCH` that makes each per-slot SOCKS relay rebuild its isolated client
on its next connection — fresh circuits under a fresh isolation group, reload to
apply. A lock-free bump that never touches the proxy or fail-closed.

**5 — Log-path nicety.** The rolling appender writes `cyberdesk.log.<date>`, so the
bare `cyberdesk.log` never exists — the "file not found" confusion from Sascha's
test. `logging::log_location()` now resolves the NEWEST `cyberdesk.log*` (or reports
the dated pattern), and README states the pattern. The MF viewer makes the file
largely unnecessary anyway.

**Honest scope + verification.** Tor and Log tabs are fully functional (real ring
data); Terminal and Bridges are visible placeholders. Verified without desktop
scraping: unit tests for the ring (capacity/eviction/filter/since_seq/severity-rank);
the real page JS against DOM shims (MF tabs + tor filter + level filter; settings
version + new-circuit button; command DOM order); and the LIVE app — the MF page
polled `get_log_lines` 8× in 8 s (proving the new view loads, its JS runs, and the
IPC round-trips), a runtime close test (add→2, close→1, last-slot refused), and clean
boots throughout. NOTE: `docs/cyberdesk-vision-law.md` is referenced by the ticket
but does not yet exist in the repo — the ticket's vision-law checks were applied from
its inline principles (token design, mouse-first, per-window autonomy, honesty).

## D-0029 - 2026-07-10 - CD-15-HOTFIX addendum: track the embedded Tor (arti) engine as an update component; derive its version from Cargo.lock

Sascha's call-out: the CD-13 update info area tracked CyberDesk and the CEF core but
NOT the embedded Tor engine — and an outdated Tor client is security-critical (arti
can even declare itself obsolete). So the arti-client package is now a tracked
update component, surfaced exactly like CEF.

**Manifest — no schema change.** The CD-13 `components` object is already a generic
`{<id>: {recommended, reason?, notes_url?}}` map (D-0023), so a `"tor"` key needs no
struct/schema change; `schema` stays `1`. `build_items` gained a `m.components.get
("tor")` branch mirroring `cef` (id `tor-update`, title "Tor engine update
recommended", the same `security`/`recommended` severity, the same versioned-dismiss
watermark and glyph-count contribution). Absent `tor` (older manifest / offline
cached manifest) → skipped, no item, no error — the same fail-safe as cef.

**The one genuinely new decision — the RUNNING arti version source.** CEF exposes
compile-time constants (`cef::sys::CEF_VERSION_*`), so `current_cef_version()` reads
the truth for free. arti-client 0.44.0 exposes **no** equivalent public version
constant (verified against the pinned crate source — its only `pub const`s are
unrelated, and it does not re-export its `CARGO_PKG_VERSION`). And `env!("CARGO_PKG_
VERSION")` in our crate yields CyberDesk's own version, not the dependency's; Cargo
provides no `env!`-accessible dependency-version var for a normal (non-`links`) dep.

Rather than hand-restate the version in a `const` (which silently drifts on `cargo
update`), a **`build.rs`** — the repo's first build script — reads the RESOLVED
`arti-client` version from the committed `Cargo.lock` (a small dependency-free line
scan of the `[[package]]` blocks) and injects it via `cargo:rustc-env=
ARTI_CLIENT_VERSION`; `current_tor_version()` reads it with `env!`. This derives the
authoritative running version and auto-syncs to the lockfile — the same "derive,
don't restate" spirit as CD-13's CEF constants (the harder, correct path, D-0006).
If the lockfile can't be read the script emits `"unknown"` and never fails the build.
`rerun-if-changed=Cargo.lock` keeps it cheap. Currently resolves `0.44.0`.

**Honesty.** The reported version is the arti-client **crate** version (the engine
CyberDesk links) — not the standalone `arti` CLI nor the Tor network protocol
version. The info panel labels the row "Tor engine (arti)".

**Verified (no network):** unit tests for the tor parse/compare + absent-component
safety; an offline run against a local manifest with a higher `tor.recommended`
showed the glyph count include the item (`count=1 items=["tor-update"]`, `cur_tor=
0.44.0` from `build.rs`); and a headless render of the real `info.js` showed the
"Tor engine (arti)" status row ("0.45.0 available" when behind, "up to date" when
matched, omitted when the `tor` object is absent). Amends the D-0023 schema note:
`components` now carries `cef` AND `tor`.

## D-0028 - 2026-07-10 - CD-15-HOTFIX: file logging, never-block-UI, async proxy on context init, bootstrap timeout + Failed state (amends D-0027)

CD-15 was built but FAILED on Sascha's live machine: the admin Tor status sat on
"connecting" forever, the whole front-end froze (no browser could be opened), and
there was no log to diagnose with. This hotfix makes CD-15 actually work. It
precedes CD-15 acceptance (Sascha re-runs the leak checklist afterwards).

**File logging first (measure, don't guess).** A windowed release build has no
visible stderr, so every failure was silent. Added a rolling-daily file log via
`tracing` at `%LOCALAPPDATA%\CyberDesk\logs\cyberdesk.log` (dated, non-blocking, no
ANSI; `RUST_LOG` overrides the default `info,cyberdesk=debug,arti…=info` filter).
`tracing` is the deliberate pick: arti and tokio already emit `tracing`, so ONE
subscriber captures our lifecycle logs AND arti's internal bootstrap/dirmgr
progress. The whole Tor lifecycle is logged verbosely (runtime build, SOCKS bind,
state/cache dirs, bootstrap begin/ready/failed+reason, context create, proxy
applied, browser create). This log is what found the root cause below. Never logs
secrets. General debugging infrastructure for all future work.

**ROOT CAUSE of the freeze — a NULL error out-param.** CEF's
`RequestContext::SetPreference` returns false when handed a NULL `error` pointer,
and cef-rs's `CefString::default()` is `Borrowed(None)` which marshals to null. So
EVERY per-context pref set silently failed (returned 0 with an empty error string).
The `proxy` pref never applied; the fail-closed guard (D-0027) then destroyed the
Tor browser — and with Tor as the new-window default that killed every column,
i.e. "frozen, nothing opens." Fix: pass a REAL error buffer (a `BorrowedMut` over a
stack `cef_string_t`) to every `set_preference`, through one `apply_pref` helper;
the set now succeeds and any genuine error is captured and logged instead of lost.
Confirmed offline across repeated runs: `proxy_ok=true`.

**Proxy applied ASYNC, in the context-init callback.** A freshly created
`CefRequestContext` initializes asynchronously; `SetPreference` no-ops until it is
ready. The prefs moved out of the synchronous create path into a
`RequestContextHandler::on_request_context_initialized` (`TorContextHandler`). This
is also exactly the right fail-closed moment: the browser's network requests wait
for context initialization, so the proxy is on the context BEFORE any traffic. If
the proxy set ever fails there (must not, on an initialized context), the handler
closes the slot rather than let it reach the network directly.

**Never block the UI thread; create the browser immediately.** Reaffirmed and
verified: `tor::init` spawns the background `tor-engine` thread; `run()` does
`block_on` on THAT thread; `toggle_tor` returns immediately and posts browser
creation to the CEF UI thread. The Tor browser is created IMMEDIATELY under its
proxied context, NOT gated on arti bootstrap — its first URL is the local
`cyberdesk://start` page (no network), and a real fetch simply cannot complete
until arti is READY (safe — fail-closed, never a direct fetch). "Fail-closed" here
means *the proxy is applied to the context*, not *wait for bootstrap*.

**Never "connecting" forever — timeout + terminal Failed{reason}.** Arti bootstrap
now runs under a `tokio::time::timeout` (default 90 s, overridable via
`CYBERDESK_TOR_BOOTSTRAP_SECS` for very slow Tor networks / tests); a timeout or
error sets a terminal `Failed{reason}` state, never infinite `Bootstrapping`. Arti
is given an explicit, known-writable state + cache dir under the app data dir via
`CfgPath::new_literal` — the default config's `${ARTI_*}` path variables must
resolve at runtime and were a likely Windows stall. Verified offline: a 4 s cap on
a Tor-blocked network yields `Failed{"bootstrap timed out after 4s…"}`.

**WebRTC leak checklist — corrected (amends D-0027).** D-0027 listed three WebRTC
prefs and left "do `webrtc.*` apply per request-context in CEF 149?" as an open
question for the live run. Now answered from the readable error strings:
`webrtc.ip_handling_policy = "disable_non_proxied_udp"` **does** apply per-context
(confirmed `webrtc_ok=true`) — that single policy blocks any non-proxied UDP path,
which is the WebRTC leak guard. The other two (`webrtc.multiple_routes_enabled`,
`webrtc.nonproxied_udp_enabled`) are **unregistered preferences in CEF 149** — they
only logged "Trying to modify an unregistered preference" and did nothing, so they
were removed. The proxy pref remains the fail-closed guarantee.

**Status display reflects reality.** The `tor_status` IPC gained a `reason` field
(empty unless failed). Settings shows the concrete failure reason under the status
pill (so "failed" is never a dead end) and re-polls every 2 s. The per-window Tor
glyph shows a distinct warn state when the engine FAILED — a lit shield must never
imply protection that isn't there (it is fail-closed and cannot fetch). Wire-format
change: `tor_status` response is now `{status, reason}`.

**Honesty (unchanged, restated).** This is still not Tor-Browser-grade: it hides the
IP but does not add anti-fingerprinting or change the TLS-layer fingerprint. The
routing/WebRTC/DNS/two-circuit checklist is still verified live on Sascha's
networked machine — the offline work here proves the UI never freezes and status
reaches Ready or Failed, not that traffic actually exits via Tor.

## D-0027 - 2026-07-10 - CD-15: per-window Tor via per-CefRequestContext proxy + the leak checklist + honest scope

The per-window switching, the leak checklist, and the honest-scope UI on top of
the D-0026 engine (arti + per-slot SOCKS).

**Per-window switching = per-`CefRequestContext` proxy (NOT the global context).**
Verified in the pinned cef 149.3.0 crate: `browser_host_create_browser`'s last arg
takes `Option<&mut RequestContext>`; `request_context_create_context` + `RequestContext::
set_preference` set the `proxy` pref. A Tor slot's browser is created under its OWN
context whose `proxy` = `{mode:"fixed_servers", server:"socks5://127.0.0.1:<per-slot
-port>"}` (its own arti circuit, D-0026); a clearnet slot keeps the direct global
context. Setting the proxy on the GLOBAL context is the classic "proxy changes for
all windows" bug — avoided by per-slot contexts. `set_preference` is UI-thread-only
under `multi_threaded_message_loop`, so the whole Tor-slot creation is posted to the
CEF UI thread via `post_task(ThreadId::UI, …)`. Toggling tears the slot's browser
down and respawns it under the other context at the start page (a fresh identity, no
state bleed) — never mutating a live browser's proxy.

**The leak checklist.** On each Tor context: `proxy` (above); the WebRTC prefs
`webrtc.ip_handling_policy = "disable_non_proxied_udp"` + `webrtc.multiple_routes_
enabled=false` + `webrtc.nonproxied_udp_enabled=false`; QUIC disabled **globally**
via the `--disable-quic` command-line switch (no per-context QUIC pref exists; QUIC
rides UDP and can bypass a SOCKS proxy). Remote DNS is enforced host-side in the
SOCKS relay (D-0026). **Whether `webrtc.*` apply per request-context in CEF 149 is
not verifiable here (no network) — it is on Sascha's empirical WebRTC test**; the
documented fallback if they don't hold per-context is the global
`--force-webrtc-ip-handling-policy` switch (which also constrains clearnet — a
breakage tradeoff to weigh). The proxy pref is the fail-closed guarantee.

**FAIL-CLOSED, hardened by an adversarial security review.** A 5-lens review
(fail-open / thread-MTML / pref-correctness / DNS-remote / toggle-isolation),
verified in two rounds, found and closed three real fail-open IP leaks before they
shipped: (1) if the proxy pref does not apply, **no browser is created** — a "Tor"
slot never falls back to a direct connection; (2) a link/popup opened from a Tor
slot (`open_in_new_slot`) **inherits** the source's Tor mode, so it can't silently
open clearnet; (3) each browser is tagged with its creation mode and
`on_after_created` **closes** a mode-mismatched browser, so a rapid double-toggle
(TOCTOU) can't leave a clearnet browser bound to a Tor slot; and fresh slot ids set
the mode explicitly so a reused id can't inherit a closed Tor slot's stale flag. The
review's second round came back dry.

**Settings + honest scope (mandatory).** A Tor settings section: an engine master
switch (`tor_enabled`, default on) that gates turning Tor on, a "Tor for new windows"
default (`tor_default`, default off) read at every slot creation, and a live status
readout (off / connecting / ready / failed) via the `tor_status` IPC. The **honesty
label** states plainly: Tor mode routes the window through Tor and hides the IP, but
does NOT provide Tor Browser's anti-fingerprinting and cannot change the
network-layer (TLS/JA3) fingerprint — so it is **not equivalent to Tor Browser** for
anonymity. No overclaiming (Vision Law + marketing rule). CD-16 (browser hardening)
narrows the fingerprinting gap and its strong tier will auto-engage in Tor windows;
it does not close the gap.

**Verification honesty.** Machine-checkable here: compile / clippy / 42 tests, the
adversarial review (fail-open logic), the toggle-glyph + settings render (headless),
clearnet boots. NOT checkable here (no network) and gated on Sascha's live run:
check.torproject.org shows a Tor exit, DNS-leak shows no local resolver, WebRTC
doesn't leak, two Tor windows differ (different circuits). These are hard acceptance
gates before the feature ships.

## D-0026 - 2026-07-09 - CD-15: the embedded Tor engine (arti) + per-slot local SOCKS relay

Sascha's ruling: build Tor in, per window, securely. Stage A is the engine; the
per-window CEF proxy wiring, the full leak checklist, and the honest-scope UI are
D-0027 (Stages B/C).

**Engine = embedded `arti-client` (v0.44), not a subprocess.** arti is the Tor
Project's pure-Rust Tor. Two integrations were weighed: embed `arti-client` +
a local SOCKS bridge, or spawn the `arti` binary as a SOCKS subprocess. **Embedded
was chosen** — it matches the single-binary doctrine and Sascha's proven SimpleGoX
approach (his explicit direction on this ticket). The residual cost of embedding is
noted below.

**Dependency justification (a large, reasoned exception to the lean-dependency
doctrine).** `arti-client` pulls tokio + a large tor-* tree (~200 crates). This is
heavy, but the per-window Tor feature justifies it, and it stays pure-Rust +
**rustls** (`default-features = false, features = ["tokio", "rustls"]`, no OpenSSL).
Runtime: arti runs on a dedicated background tokio multi-thread runtime on its own
thread; `tokio-util` (compat) bridges arti's futures-based `DataStream` to tokio for
the relay; `tor-rtcompat` names arti's `PreferredRuntime`. The shell's winit/wgpu/CEF
main thread never touches async. It builds clean on Windows MSVC (verified).

**Bootstrap is off the shell thread; status is a lock-free atomic.** `tor::init()`
spawns the engine thread once (idempotent via a CAS on the status), which binds the
SOCKS listeners, then `TorClient::create_bootstrapped`. Status
(`IDLE`/`BOOTSTRAPPING`/`READY`/`FAILED`) is an `AtomicU8` the UI reads. Startup and
the UI **never block or freeze** on Tor — verified by a boot with the engine started
offline (bootstrap runs/retries on its thread; the shell renders and stays
responsive; no freeze, no crash in the window observed).

**Per-slot SOCKS + circuit isolation.** Rather than one SOCKS port with per-cred
isolation (CEF can't easily carry SOCKS creds in its proxy pref), each slot id gets
its **own loopback port** (`127.0.0.1:9250+id`) served by its **own
`isolated_client()`** — arti puts each on its own circuit family, so two Tor windows
are unlinkable (acceptance #4). A slot's Tor request context (D-0027) proxies through
its port.

**Remote DNS (host side of the leak checklist).** The hand-rolled SOCKS5 CONNECT
relay hands a hostname (SOCKS `ATYP=domain`) to arti **unresolved** — DNS resolves
remotely through Tor, never a local resolver. An explicit IP (`ATYP=1/4`) came
straight from the client (not a local resolution), so it is connected via
`TorAddr::dangerously_from` — the one intentional, audited place IPs enter. The
WebRTC / QUIC / proxy-bypass half of the checklist lives on the CEF context (D-0027).
**Empirical verification (check.torproject.org, DNS-leak, WebRTC) is Sascha's live
run** — this environment has no network, so the routing/leak behaviour cannot be
tested here; that is stated plainly and is a hard gate before the feature ships.

**NetGuard (D-0004) gains a second sanctioned outbound path.** Alongside the single
pinned update-manifest URL (D-0023), Tor exit traffic is now permitted — but unlike
that one URL, this is *user-driven browsing through the Tor context*, which is the
whole point. Documented as the second exception; when NetGuard is built it governs
both.

**Residual risk (accepted, embedded-specific).** Embedded arti may
`process::exit(1)` on an obsolete consensus, which would take the shell down; the
subprocess path would isolate that. Embedded was chosen anyway (doctrine + SimpleGoX);
the risk is documented and accepted. SBOM/CRA: `arti-client`, `tokio`, `tokio-util`,
`tor-rtcompat` (and their tree) are new dependencies to list.

## D-0025 - 2026-07-09 - CD-14: own start page (Google banished), no saved websites, big-monitor focus

Three Sascha rulings.

**1 — Google is banished; slots open to an OWN start page.** The hardcoded
`https://www.google.com/` slot default is gone. Every empty/new slot now loads an
internal page served from the binary at **`cyberdesk://start/`** — the same scheme
and isolation as settings/command/info, **zero network** (no fonts, images, or
remote resources; self-contained like the other internal pages). It carries the
reserved **Energy Core** motif (CD-06) at last: a bright hollow core inside
concentric rotating brand-cyan arcs (SVG + CSS, GPU-cheap, `prefers-reduced-motion`
aware) on a black canvas with a faint static micro-lattice, a search/address
capsule, and a row of round favorite tiles (the CD-12 launcher language). The
search box and tiles reuse the existing `navigate` + `query_suggestions` IPC (same
host-side URL-vs-search classifier + `search_engine` setting) — they act on **this**
slot, because interacting with a slot makes it the active slot host-side.

*Reasoned wiring notes:* (a) The browser-side message router now forwards
`on_process_message_received` for **all** views, not just the internal one, so a
slot's start page can use `cefQuery`. This is safe: `window.cefQuery` is exposed
ONLY on `cyberdesk://` frames (the render-side `on_context_created` gate), and the
start page is the sole `cyberdesk://` content a slot ever shows — a web page in a
slot has no query bridge. (b) `Ctrl+T` now spawns the new slot at the start page
(the lazy placeholder covers the brief spawn until it paints); the CD-12 Ctrl+T
floating-capsule auto-reveal is **retired** — the start page's own search box is
the landing surface (Ctrl+L still reveals the floating capsule). The
`search_engine` setting keeps Google as one *search* choice (CD-07) — that is the
user's engine, not a hardcoded start page.

**2 — Websites are not saved (the privacy reversal of CD-10 / D-0018-D-0019).**
The `session_slots` URL persistence is removed. `restore_session` is now an
unconditional **default-workspace boot**: one slot at the start page + the internal
view, on every launch — never restored websites. There is no save path at all
(the debounced session-save wiring is gone), so open URLs never touch disk. Store
**schema v5 DROPs the `session_slots` table**, which also **purges** any URLs a
prior build had persisted (verified against a simulated v4 install carrying a URL:
after boot the table is gone, schema is 5, no panic). This **supersedes the
session-URL parts of D-0018/D-0019**; the slot engine, width units, rearrange, and
open-in-new-slot (the non-persistence parts of those decisions) stand. The
now-vestigial lazy-slot machinery retires with it (pre-armed `armed` URLs,
spawn-on-first-touch, the restored-pending placeholder dot) — every slot spawns its
start page immediately. **History and favorites (CD-07) are untouched** — only the
restore-open-websites behavior goes; a future "history off" would be its own setting.

**3 — Big-monitor focus for now.** The three main areas (Spine, slots, MF zone)
target the large monitor this season; sub-1920 / small-resolution polish is
explicitly deferred (no investment this ticket). The start page scales with
viewport units, so it is not a regression there, but small-panel layout is not a
goal until later.

Verified: the start-page layout via a headless Edge one-shot (black backdrop,
Energy Core, capsule, favorite tiles); zero external resource loads by
construction; the privacy purge + fresh boot via SQLite inspection; live boots on
the big-window path with no panic. (Note: the referenced
`docs/cyberdesk-vision-law.md` does not yet exist in the repo; this ticket was
self-checked against the stated principles — no bars, floating elements,
mouse-first, honest security — which the start page and the no-restore default all
satisfy.)

## D-0024 - 2026-07-09 - CD-13: the info area is the generic notification-rail seed; V1 informs, never installs

The info area (top-right glyph + floating panel, CD-13) is built on a **generic
info-item model** — `{id, severity, title, body, action?}` — not a bespoke
"updates" widget. This is deliberate: it is the **seed of the future notification
rail** (Season 7, where the Priority Engine's events — doorbell, call, alarm,
security alert — ride the same model and the same surface). V1 produces only the
two update items (CyberDesk + CEF), but nothing about the shape, the panel, the
dismissal model, or the glyph is update-specific. When events arrive they slot in
as more info items with their own severities; the rail does not need re-architecting.

**Rendered per the floating law (D-0021).** No strip: the info glyph is its own
shell-drawn element beside the gear (idle = a faint ring; updates available = a
filled brand disc with a modest pulse and a count badge — a status light, not an
alarm), and the panel is a floating top-right card on the shared internal OSR
view. It joins the mutually-exclusive overlay set (info / settings / command band);
the existing ESC chain and `take_overlay_close` extend to it naturally. Its home
is the permanent Multifunctional zone's conceptual neighborhood (top-right), the
same corner D-0022 made permanent.

**V1 informs; it does not install.** The panel shows availability and links to
release notes (which open in a slot); it never downloads, never installs, never
auto-bumps CEF. Self-update is a deliberate future capability that arrives with
the **signed ML-DSA pipeline** (Season 6+ / commercial): this surface is exactly
where its **Install** action will live (a third item action alongside "Release
notes" / "Dismiss"), but until the signature-verified pipeline exists, offering to
install would be a security foot-gun. A CEF bump likewise stays a deliberate
build / test / release act — the item informs, the human decides.

Dismissal is versioned (D-0023): dismissing calms the glyph and persists across a
restart, and an item re-appears only when the manifest advances past the dismissed
version — so "I'll deal with 0.9.0 later" is honored, but 0.10.0 re-raises it.

## D-0023 - 2026-07-09 - CD-13: update-awareness — the one pinned outbound endpoint + its HTTPS client

CyberDesk gains an info area (top-right, CD-13) that shows product + component
update availability. This is a milestone: **the host's FIRST intentional outbound
network connection.** Two coupled decisions are recorded here; the info-area UX
and its notification-rail seeding are D-0024.

**The NetGuard exception (D-0004 survives via exactly ONE documented hole).** The
deny-by-default doctrine (no module opens its own connections outside the future
central NetGuard) stands, with a single carve-out: **one allowlisted URL, HTTPS,
the pinned CARVILON update manifest** (`updates.feed_url` token, default
`https://carvilon.com/updates/cyberdesk.json`). The client queries nobody else —
not Google, not CEF's servers, not a CDN. This is spot-checkable in code:
`updates::fetch` has exactly one call site (`run_check` with `feed_url()`), and
`feed_url()` returns the config token (or the `CYBERDESK_UPDATE_FEED` test
override). A 404 / unreachable / malformed feed is **silent** — the glyph stays
idle, the last-known cached manifest (if any) is kept, startup is never blocked,
never an error in the user's face. When NetGuard is built (Season 5), this URL
becomes its first allowlist entry rather than a code-level exception.

**The HTTPS client dependency (reasoned exception to the lean-dependency
doctrine).** The host needs a minimal TLS client for that one fetch. Chosen:
**`ureq` 2.12 with rustls** (`default-features = false, features = ["tls"]`) —
blocking (fits a background worker thread), rustls-based (no OpenSSL / system TLS),
small stable API. Note: `download-cef` already pulls `ureq 3` as a **build**
dependency (CEF fetch at build time); our runtime `ureq 2` ships in the binary and
does not duplicate anything shipped. The fetch runs on a named background thread
with hard `timeout_connect` (5 s) + `timeout_read` (8 s) caps, so the shell UI
never waits on it. A local `file:` / path in `CYBERDESK_UPDATE_FEED` is read from
disk (end-to-end testing before the manifest is live); the production path is
HTTPS-only.

**Version self-awareness (crate-source-first).** CyberDesk's version is
`CARGO_PKG_VERSION`. The running CEF/Chromium version comes from the pinned
crate's **compile-time constants** — `cef::sys::CEF_VERSION_MAJOR/MINOR/PATCH`
(→ `149.0.6`) and `CHROME_VERSION_MAJOR/MINOR/BUILD/PATCH` (→ `149.0.7827.201`).
The old `cef_version_info(entry)` runtime call does not exist in this binding
(verified in the crate); `cef_api_version()` returns the configured API version,
not the product version, so the constants are the correct source.

**Schema + storage.** The manifest schema (v1) is `{schema, cyberdesk:{latest,
notes_url}, components:{<id>:{recommended, reason, notes_url}}}` — documented with
a sample in `docs/updates/cyberdesk.sample.json` and the wire-format. Parsing is
tolerant: a higher `schema` with extra fields is read best-effort (serde ignores
unknowns); a truly malformed feed fails to parse and is treated as no-data. Version
comparison is tolerant of both our semver and CEF's `major.minor.patch+chromium-…`
(only the head before `+` matters), unit-tested against good / malformed /
future-schema fixtures with NO network. SQLite **schema v4** adds `update_meta`
(cached manifest JSON + last-check unix time, so the glyph reflects last-known
offline) and `update_dismissed` (per item id → the version dismissed at; an item
re-appears only when the manifest advances past it). The worker checks on startup
and every `check_interval_hours` (6), nudged immediately by "Check now".

## D-0022 - 2026-07-09 - Revised frame law: three slots, permanent Multifunctional zone

Sascha's ruling after living with the CD-11 frame (D-0020): **four columns are too
many, and the zones matter more than a fourth browser.** This revises D-0020's
symmetric-zones + four-slot parts (D-0021's floating layer is untouched — it reads
the frame rects generically and adapts automatically: ensembles, drop zones, close
orbs all follow the new geometry).

**1 — Slot maximum is THREE.** A new `slots.slot_max` token (= 3) caps the frame
everywhere: `frame_capacity` (its unit ceiling becomes `slot_max·2`), `max_slots`,
Ctrl+T, drag-into-gutter targets, and restore plans all clamp against it. The
compile-time `MAX_SLOTS` array ceiling stays **4** (the per-view arrays / id space),
so `slot_max` is a tunable product policy `≤ MAX_SLOTS` — raise the token, no array
resize. A fourth column simply never opens while `slot_max = 3`.

**2 — The RIGHT zone is the Multifunctional (MF) zone, and it is PERMANENT.** It is
always `mf_zone_width` (= 320) at every resolution; it never rails, never
disappears. It is the future tab area (status, files, FTP, music — later seasons),
a placeholder now in the slot-placeholder family with a **distinct core glyph**: a
three-bar **rows / tab-rail** glyph (the left zone keeps the diamond), so the two
zones read apart with no font (the shell has none).

**3 — The LEFT zone (future Spine) is the flexible one.** The CD-11 reflow law and
its animation-safety construction now apply to the LEFT zone alone: **Full**
(`side_zone_width` = 320) when the slots leave room for it alongside the permanent
MF zone, else it retreats to a thin **Rail** (`side_rail_width` = 48).

**4 — Wider gutters.** The `gutter` token rose **40 → 56** ("more space between the
screens"), applied between slots AND between the group and each zone. The budget
holds — three 1200 slots + both zones full + all four gutters at 5120: `320 + 56 +
(3·1200 + 2·56) + 56 + 320 = 4464 ≤ 5120` (656 px margin, proven in a test). No
other margin token needed a nudge.

**5 — The floor law.** At 1920 the minimum working set is exactly **one slot + the
MF zone + the left rail** (the full left zone doesn't fit: `320 + 56 + 1200 + 56 +
320 = 1952 > 1920`, so it rails; `48 + 56 + 1200 + 56 + 320 = 1680 ≤ 1920` fits,
balanced 120 px margins). Scaling: more width adds slots up to three, and the left
zone goes Full as soon as it fits alongside them.

**Geometry — the asymmetric shift.** The frame `left | gutter | slots | gutter | MF`
is centered in the window as a block. Because it is now asymmetric, the slot group
is **no longer window-centered**: it is offset by `(left_width − mf_width)/2` toward
the smaller zone (0 when the left is Full, so both zones = 320 → centered; −136 at
1920 with the left railed). Implementation reuses `slot_rects_units` (window-
centered) and **translates** the rects by that `dx` — one function still decides
state + all rects, so the reflow animation stays desync-safe by construction: the
shell eases the LEFT width (`disp_left_width`) and the group's `dx` glides with it,
while the MF zone width is constant and only follows the group's right edge. Both
rendering and input read the same per-frame geometry.

**Revised capacities** (unit budget = `width − mf_zone_width − side_rail_width −
2·gutter`, gutter 56): **1920 → 1, 2560 → 1, ~3000 → 2, 3440 → 2, 5120 → 3** (was
1/1/2/4 at CD-11). Two slots now need roughly a **3000**-wide window; three need
the ultrawide. Sessions / width-units logic is **unchanged in structure** — only
the capacity value shrinks; restore now fits against `frame_capacity` (the same
budget the live shell caps against) instead of the zone-blind `max_slots`, so a
restored workspace never momentarily over-fills the frame (`max_slots` becomes a
tested building block only).

Verified: 42 unit tests (revised capacities, floor law, asymmetric shift, MF
permanence, boundary cases), `--capture` at 1920 (rail + slot + MF rows glyph) and
5120 (full Spine diamond + 3 slots + full MF rows glyph), a 3000×900 boot. See
D-0020 (the symmetric frame this revises) and D-0021 (the floating layer it feeds).

## D-0021 - 2026-07-09 - CD-12: floating command elements — the bar dies, per-window command sets

Sascha's ruling: **the bar dies, and nothing global replaces it.** A single top
bar that drove "the active column" was the last centralized surface — wrong for a
workspace of autonomous windows. It is retired. In its place, **every column owns
its own floating command set**, and favorites become a shared launcher of round
tiles. The top region is no longer a bar; it is a transparent band on which
per-window ensembles and one launcher row float over the Pulse Grid.

**One transparent internal view, N floating ensembles (the CD-11 IPC precedent).**
The band is a single OSR CEF view spanning the top, composited with a fully
transparent background (`BrowserSettings.background_color = 0x00000000`, premultiplied
BGRA in on_paint) so only its pills paint and the Pulse Grid breathes between them.
The page builds one `.ensemble` per slot (each = back/forward/reload orbs + an
address capsule with a lock icon, url input and star). The host supplies **frame
state** — the engaged slot and each slot's band-DIP x/width — and pushes it **on
change only** (`window.cdFrame`), exactly the CD-11 cadence: no per-frame IPC. The
page glides its ensembles to the new positions via CSS (~220 ms), so an ensemble
visually trailing the column by a frame during a reflow **is correct**, not a bug.
Because the ensembles are DIP-positioned HTML and the band is one view, the
transparent view casts **no zone shadow** (dimming the whole top would darken the
grid where nothing shows) — only the opaque settings card still dims its rect.

**Generalized reveal state machine.** CD-08's single-bar reveal/hysteresis/typing
machinery generalizes to N: hovering the top gap above a column engages *that*
column's ensemble (`band_hot_slot`), Ctrl+L engages the keyboard-active column's
capsule with autofocus (`reveal_active_capsule`), and the band disengages on
mouse-out (+hysteresis, typing exception), a committed navigation, or ESC. A
compositing linger (`band_off_at`, ~300 ms) keeps the band painted until the CSS
fade-out finishes, then finalizes to `Closed`. Autofocus is a **transient**: the
push change-signature excludes it and the page pulls `get_frame` on load, so a
per-frame push(false) can never clobber a pending Ctrl+L focus.

**Every command carries its slot.** The nav/palette commands (`get_nav_state`,
`navigate`, `go_back/forward`, `reload`) gained an optional `slot` field: each
ensemble drives its own column. The host's `target_slot` reads it (clamped), else
falls back to the active slot. This revises CD-09's "the bar targets the active
slot" — targeting is now per-ensemble. See docs/cyberdesk-wire-format.md.

**Drag a favorite into a gutter → a new column there.** Favorites live once, as
round launcher tiles. The page owns only the gesture *start*: a tile drag past a
6 px threshold fires `drag_start {url,title}`; the **host owns the whole drag**
(the page has no cross-column coordinates and the OSR view can't draw outside
itself). The shell draws, topmost, a ghost circle on the cursor and the control
gutters as drop zones (the nearest to the cursor hot), captures the mouse (slot
views get no events, ESC cancels — chained before the band), and on release either
inserts + spawns the favorite as a new column at the nearest gutter, or — at full
capacity, where no gutter can accept an insert — navigates the slot under the
ghost. The CD-11 gutter widening (24 → 40) reserved exactly this control surface.

**Floating per-slot close orb.** Each column reveals a shell-drawn close orb (a
brand ring + inset cross on a dark backing disc) at its **top-outer corner** when
the cursor enters that corner's hot-zone; a click closes that column (the last
refuses; a non-active close leaves the active column as is), and the frame reflows.
Reasoned realization of "top-outer-corner": the orb sits at the **top-right** of
every slot — the universal close convention — rather than mirrored per side, which
would make the middle columns' "outer" ambiguous and break muscle memory.

**Rendering — one shared overlay pass.** The drag visuals and the close orbs are
one instanced soft-rounded-rect pass (`drag.wgsl` + `DragOverlay`, in the
placeholder/lines family, premultiplied OVER, drawn topmost). A `kind` field in
the instance selects the fragment: `0` a filled soft rounded rect (a circle when
`corner_radius` = half — the ghost, drop-zone bars, slot highlight, orb backing),
`1` a ring + cross (the close orb). Drag and close orbs never coexist (the drag
captures the mouse, suppressing hover), so the app feeds the pass whichever is
live. `CYBERDESK_CAPTURE_DRAG` / `CYBERDESK_CAPTURE_CLOSE` render samples over the
`--capture` frame for headless verification, alongside the CD-09/CD-10 CAPTURE_* knobs.

**Tokens.** A `[command]` block carries the band geometry (`band_height`,
`launcher_top`, `ensemble_top`, `capsule_height`, `orb_size`, `tile_size`,
`tile_gap`) as both host sizing and page CSS vars (`Theme::to_css_vars`), plus
`close_size` (host-only — the orb is shell-drawn, so no CSS var). One token source,
as always.

**Verification honesty.** The live transparent compositing of ensembles over the
Pulse Grid is only fully verifiable on Sascha's monitor. Machine-checkable here:
the page layout (headless Edge one-shot of `command.html`), the overlay geometry
(`--capture` with the CLOSE/DRAG sample knobs), the host logic (compile / clippy /
unit tests / a throwaway-profile boot). The 2560 dev width holds one column; the
drag-into-gutter path needs ≥ ~2800 (`CYBERDESK_WINDOW_SIZE`) or the 5120 ultrawide.

**Forward note.** CD-13's info area / status glyphs follow this floating law — a
shell-drawn or transparent-view element positioned from host frame state, not a new
global surface. NetGuard (D-0004) is untouched: the band opens no network of its own.

## D-0020 - 2026-07-09 - CD-11: the main frame — side zones, reflow-to-rails, control gutters

Sascha's ruling: this IS the main system, and it was missing. The slot group did
not own the full width — but the width does not belong to the browsers alone.
**Left and right of the slots live side zones, first-class citizens** (placeholders
now; their future contents are the Spine and the status / files / music rails).
When the browsers demand the width, the side zones **retreat, animated, into thin
rails**, and expand back when slots close. The gutters widen into reserved control
territory (CD-12 puts drop zones there).

**The frame law (pure, deterministic math — `slots::frame_layout`).** The frame is
`side | gutter | slots | gutter | side`, centered in the window. Because it is
symmetric, the slot group stays centered in the window — so `slot_rects_units`
(D-0017/D-0019) is **reused unchanged** and the side zones simply flank the group,
one gutter away, at the slot height. One function decides everything (side state,
side width, all rects); no incremental fudging.

- **Side state.** **Full** if the slot group plus full side zones (`side_zone_width`
  = 320) and their flanking gutters fits the window; else **Rail** (`side_rail_width`
  = 48). One decision from the total slot units.
- **Capacity.** The shell caps slots against `frame_capacity` — the unit budget of
  the **rail** center budget (the roomiest side state), so slots never exceed what
  the frame will ever hold. Side zones therefore reduce mid-size capacity vs the
  pre-CD-11 `max_slots`: measured 1920→1, 2560→1, 3840→2 slots; the 5120 ultrawide
  still reaches **four** (1–3 slots show full side zones, the fourth forces rails).
  A window narrower than one slot + full sides (e.g. the 1600 dev window) shows the
  rail state at one slot. `max_slots` stays as the tested no-side-zones building
  block.
- **Placeholder until content.** The side zones render in the slot-placeholder
  family with a differing glyph — a subtle fill above base, a thin inset outline,
  and a small centered **diamond** (rotated-square outline) core glyph. No text: the
  shell has no font (the slot index glyphs are hand-drawn 7-segment SDF, D-0019).
  Geometry now; content in later seasons.

**Control-gutter reservation.** The `gutter` token widened **24 → 40** (both between
slots and between the slot group and the side zones). This is deliberately generous
control surface — CD-12 builds drop zones here, and the Pulse Grid glowing in it is
intended. **Token calibration (reasoned):** the briefing's target was ~48, but 48
does not fit four 1200px slots at rail on the 5120 ultrawide once the rails and
gutters are subtracted; 40 fits with a ~24 px margin (2·48 + 2·40 + 4·1200 + 3·40 =
5096 ≤ 5120) and keeps `slot_width` at its established 1200. Side-zone / rail widths
are `~` in the briefing and taken as 320 / 48.

**Animation-safety construction (pre-empting the D-0019 hard-swap precedent).** In
CD-10 the slot **swap** was left a hard jump (D-0019) because an animated version
risked the rendered rects and the input hit-tests disagreeing mid-tween. The reflow
here is animated **and** safe by construction, exactly as the briefing required: the
shell keeps one **animated frame** — per-slot rects ease toward the `frame_layout`
target (a newly added slot grows from a collapsed sliver at its target centre) and a
single interpolated `side_width` drives the side rects, all computed **once per
frame**. Rendering (the composited slots + side zones + zone shadows) and input
(mouse hit-tests, and later the CD-12 drop targets) read that **same** per-frame
geometry (`disp_slots` / `disp_rect`), so desync is impossible — Ctrl+T/W, unit
toggles and window resizes reflow as one fluid ~220 ms ease-out (the top-bar slide's
interpolation pattern), never a jump. The reflow is explicitly **not** downgraded to
a hard jump. (The top bar reads the settled target rect, not the animated one, so
its CEF view size stays stable while the columns glide.)

## D-0019 - 2026-07-09 - CD-10: session restore, width units, and slot rearrange

CD-10 makes the CD-09 slot system feel permanent and fluid: the workspace
survives restarts, columns can be reordered and widened.

**Session workspace (persisted).** A new schema-v3 `session_slots(position, url,
width_units, active)` table (store.rs) holds one implicit session — the full
ordered slot list. It is written **wholesale** (delete + insert in a transaction)
on every meaningful change (open / close / navigate-commit / rearrange / width
toggle / active change), **debounced ~500 ms** off the render hot path: the shell
computes a compact session signature each frame and arms a save only when it
differs from the last-saved one (so link-driven navigations are captured too,
not just host-side actions). `session.rs` is the domain layer over the shared
store (mirroring memory.rs), including the pure, unit-tested `plan_restore` (fit
saved slots from the left by width units, the rest to overflow, active fallback)
and the `persist_url` filter (internal `cyberdesk://` / blank / empty slots
persist as an empty URL — same rule as history/favorites, D-0014).

- **Restore on startup:** the saved order / widths / active are rebuilt with fresh
  contiguous slot ids. The **active slot spawns immediately** with its URL; the
  rest stay **lazy with the URL pre-armed** and spawn on their first interaction
  (activation via click / Ctrl+1..4 / Ctrl+Tab — routed through `set_active`,
  which spawns an armed slot). This keeps startup light while the workspace
  reappears. A fresh install / no session falls back to the CD-09 default: one
  slot on the home page.
- **Placeholder for a pending slot:** the shell has no text rendering (the index
  glyphs are hand-drawn 7-segment SDF, not a font), so a restored-but-unspawned
  column keeps its index glyph and adds a small **scheme-colored pending dot**
  (https → accent, http → warn, else → text-dim) — the briefing's honest middle
  ground, distinguishing "a page is waiting here" from a genuinely empty column.
  The pending URL is also visible via the top-bar prefill when that slot is active.
- **Windowed shrink:** restoring more slots than the current width allows keeps
  what fits from the left; the rest are held in an in-memory overflow that is
  re-saved (so a wider **restart** brings them back). A live window shrink closes
  columns from the right into the same overflow.

**Width units (double slots).** Each slot carries `width_units` (1 or 2). The
layout math extends to `slots::slot_rects_units`: a `u`-unit slot spans
`u·slot_width + (u-1)·gutter` (it absorbs its internal gutter), and — the tidy
invariant — a group of total units `U` occupies **exactly the same centered
extent as `U` single columns**, so `max_slots(width)` (the column-fit) is also the
unit budget. `slot_rects` becomes a single-unit convenience over it. **Ctrl+Shift+D**
toggles the active slot 1↔2 units; doubling is a **no-op if it would overflow** the
unit budget, halving always works. Only the toggled slot's OSR view resizes (its
page reflows); the others merely recenter. Active indication, mouse hit-testing,
the top bar, zone shadows and loading lines are all **unit-agnostic** — they read
each slot's actual rect from `slot_rects_units`, no duplicated logic. Unit-tested:
mixed 1/2-unit sequences, double span, same-extent-as-columns, centering.

**Rearrange (hard swap).** **Ctrl+Shift+Left/Right** swaps the active slot with its
neighbor. Per the stable-id order model (D-0017), this is a **pure order operation**
— the slot keeps its id (and its CEF handlers / texture), only its display position
changes; nothing resizes (widths unchanged), so the compositor picks up the new
positions on the next frame with no browser move and no CEF call. **Decision: a
hard swap, no slide animation.** The briefing invited an optional eased slide
(reusing the CD-08 bar-slide interpolation), but an animated position swap would
have to interpolate each slot's rect while the mouse hit-test, the bar geometry and
the CEF view origins all read those rects — a real desync risk during the ~180 ms
tween, for a polish nicety that cannot be verified headlessly (no desktop scrape).
The hard swap is instant, correct, and matches the "swap is an order operation, not
a browser move" guidance. The animated slide is deferred to the Season-2 Edit-Mode
drag language (which specifies the motion vocabulary first, a CD-10 non-goal).

## D-0018 - 2026-07-09 - CD-10: open links in new slots (supersedes D-0011 when capacity exists)

The gesture-aware popup policy (D-0011) navigated a user-gesture popup into the
source view in place. CD-10 extends it: a **user-gesture popup opens in a NEW slot
to the right of the source** when there is room, becoming active — the "open link
in new column" of the zone vision. This **supersedes D-0011's navigate-in-place
rule whenever capacity exists**; with a full grid it falls back to D-0011 (navigate
the source slot's main frame). Non-gesture popups (ad / script `window.open`) stay
fully suppressed. No separate OS window ever opens — `on_before_popup` always
returns 1.

- **Mechanism:** `on_before_popup` (CEF UI thread, per source slot) queues
  `(source_slot_id, target_url)` for any `user_gesture != 0` popup and suppresses
  the window. The shell's main thread drains the queue and opens each in a fresh
  lazy slot inserted right of the source (reusing the tested `slots::insert_position`
  and the `create_browser_url` lazy-spawn), which spawns immediately with the URL
  and becomes active; if `order.len() == MAX_SLOTS` or the unit budget is full, it
  navigates the source slot in place instead.
- **Ctrl+click / middle-click:** these ride the **same** path. Chromium routes a
  modified click on a link through `on_before_popup` as a tab disposition
  (`NEW_FOREGROUND_TAB` / `NEW_BACKGROUND_TAB`, confirmed against the pinned crate's
  `WindowOpenDisposition`) with `user_gesture = 1`, so no separate mouse handling is
  needed — the gesture gate covers `target=_blank` clicks and modified clicks alike.
- **Honesty note:** the `target=_blank` gesture path has driven `on_before_popup`
  since CD-04 (D-0011), so it is proven in this app. The Ctrl-/middle-click routing
  is standard Chromium behavior (the disposition enum is present in the crate) but
  was **not** click-injected end-to-end here — the shell cannot synthesize a real
  in-page click headlessly. The app-side new-slot machinery *was* verified
  end-to-end (a temporary self-test drove `open_in_new_slot`; the session then held
  the source plus the new active column, since removed). If a specific site's JS
  intercepts the modified click, no popup fires and nothing opens — the standard
  gesture-popup contract.

## D-0017 - 2026-07-09 - CD-09: the multi-slot engine (columns, lazy spawn, focus routing) and the D-0009 verdict

CD-09 turns the single surf zone into the zone system the whole season was built
for: up to four **fixed-width content columns** side by side, aligned by the
layout math, never crooked. This is the heart ticket.

**Slot model (the law).** A **slot** is a fixed-width content column: `slot_width`
(1200 logical px) wide, as tall as the surf zone (`height_frac` = 70 % of the
window, vertically centered), with `slot_gutter` (24 px) between slots; the group
is horizontally centered and never comes within `min_margin` (48 px) of the edge.
All in the new `[slots]` theme section (one token source, as always). `slot_width`
is tuned so **four columns fit the 5120 ultrawide** (4·1200 + 3·24 = 4872 < 5120);
`max_slots(width)` returns what fits (4 on 5120, 3 on 3840, 2 on 2560, 1 on 1920),
clamped to `MAX_SLOTS` = 4, never below 1. The Pulse Grid glows in the gutters and
margins — intended and beautiful.

- **Lazy slots.** A new slot has NO browser until its first navigation; until then
  the shell draws a placeholder (a rounded fill lifted above the base color, with
  the slot's index as a faint 7-segment glyph — purely shell-side, no CEF, so a new
  column appears instantly with no white about:blank flash). Slot 0 loads the home
  page eagerly (parity); the rest spawn on the first `navigate` targeted at them
  (queued to the main thread, which owns the HWND).
- **One active slot.** Keyboard input, the top bar and the scheme hint act on it.
  Active indication: a thin 2 px brand accent along the slot's BOTTOM edge (the top
  edge belongs to the loading line). Only the active slot's browser holds CEF focus;
  switching moves focus (set_focus 0 on the old, 1 on the new).

**Stable-id order model (the key architecture decision).** Slots are tracked as an
ordered list of stable **ids** (`order: Vec<usize>`), each id a fixed index into the
per-slot browser/texture arrays. A slot keeps its id — and therefore its CEF client/
handlers (which bake in `Role::Slot(id)`) and its wgpu texture — for its whole life;
only its *position* in `order` changes when columns are added, closed or recentered.
This avoids ever migrating a live browser between indices (which would desync the
handlers' baked role from where their `on_paint` writes). Ctrl+T inserts a free id
right of the active one; Ctrl+W removes the active id and promotes the nearest
neighbor; Ctrl+1..4 focus by position; Ctrl+Tab / Ctrl+Shift+Tab cycle. The
positional index logic is pure and unit-tested (`slots.rs`).

**Rendering.** The single page pass became a shared `PagePipeline` + per-target
`PageTarget` (one per slot + the overlay); the render loop draws each painted slot's
texture (feathered, at its rect), one instanced placeholder pass for empty slots, and
one instanced slot-lines pass (per-slot loading line at the top edge + the active
accent at the bottom). The zone-shadow uniform grew from 4 to **6 rects** (up to
MAX_SLOTS slots + the one open overlay; std140 array of vec4, both pulse shaders
updated). `--capture` gained `CYBERDESK_CAPTURE_SLOTS=N` to render N placeholder
columns, so the four-column money shot is verifiable headlessly (no desktop scrape).

**Input routing.** Mouse events route to the view under the cursor (the slot whose
rect *contains* it, or the overlay) at coordinates relative to that view; crossing
views sends a mouse-leave so hover states clear; a click inside a slot makes it
active; cursor-icon feedback comes from the hovered view. Keyboard routes to the
active slot; the slot-management shortcuts are intercepted host-side first. The top
bar acts on the active slot — and this needed **no new IPC**: the existing
`get_nav_state` / `navigate` / `go_back` / `go_forward` / `reload` now read and drive
`browser::active_slot()` internally, so the wire format is unchanged (verified). All
slots' visits record into the one shared history. The gesture-aware popup policy
(D-0011) stays per-slot (a user-gesture popup navigates its own slot's main frame).

**Reasoned deviations (documented per the standing rule).** The single-slot state is
no longer pixel-identical to CD-08: a lone column is now a fixed 1200 px wide (the
slot model's natural single form) rather than the old 60 %-of-width zone — wider on
the 1600 dev window, narrower on the 5120 ultrawide (where a single centered column
with generous glowing margins is the intended aesthetic) — and it carries the active
accent line (the slot law mandates exactly one active slot at all times). Behavior
(feathering, loading line, bar, favorites, history, popups, nav keys) is identical;
"parity" is behavioral, not pixel-exact. This is the deliberate consequence of
adopting the slot model, not a regression.

**Performance gate — the D-0009 measurement moment. VERDICT: the trigger did NOT
fire.** Measured on an NVIDIA RTX 3090 at 5120×1440 with **4 slots** (1200×1008 each,
**18.5 MB/frame** of page uploads), 300 frames after warmup, via a temporary headless
harness (main-thread `write_texture` staging + the full shell composite + submit +
GPU wait; since removed):

- Per-slot upload (4 slots, staging): **median 3.0 ms, p99 4.0 ms, max 4.9 ms**.
- Full frame (upload + composite + submit + wait): **median 4.45 ms, p99 6.2 ms,
  max 6.8 ms**.
- 60 fps frame budget: 16.667 ms.

The worst frame (6.8 ms) sits well under the budget with ~10 ms of headroom, so
4-slot browsing does **not** stutter and the CPU OSR path stays viable — D-0009's
stutter trigger has not fired for this ticket. **But** the uploads are already the
single dominant frame cost (3.0 of 4.45 ms, ~68 %) and scale linearly with slot
count and resolution, exactly as D-0009 predicted once per-pixel throughput started
to matter. So the **recommendation** stands as D-0009 framed it: the accelerated,
zero-copy shared-texture path (D-0009 option a — replicate cef-rs's D3D11 importer
against wgpu-30's DX12 hal) is the well-scoped next optimization to reclaim that
headroom, and becomes necessary sooner on weaker GPUs, higher DPI, or if slot counts
grow — but it is **not required now** and stays out of scope for CD-09 (measure,
record, recommend). Caveat: the harness measures the main-thread upload + composite
(what governs the render loop's 60 fps), not the CEF-side `on_paint` memcpy (a
separate thread), and forces a full GPU sync per frame (more pessimistic than
vsync-pipelined real frames), so the real on-screen margin is at least as good.

## D-0016 - 2026-07-09 - CD-08: the command surface is a hover-reveal top bar

Sascha's CD-07 acceptance changed the command surface: from the centered command
palette (D-0014) to a **hover-reveal top bar** living in the free gap above the
surf zone. This explicitly **revises D-0014's "no favorites bar"** — a favorites
surface with its own clickable controls now exists, as Sascha's call. It is a
functional v1 in the token world; the design-law polish stays Season 2.

**Surface.** The bar spans the surf-zone width, anchored to the top edge. It
holds the address input (scheme hint + star + back/forward/reload glyphs) and,
below it, one of two bodies: the favorites as clickable **chips** (title + star,
click navigates) while the input is untouched, or the CD-07 live suggestion
**list** while typing. The palette logic is reused wholesale — only the surface
moved. Chips reuse the empty-`input` `query_suggestions` (favorites, capped at
`command.max_results`); there is no separate favorites command (implementer's
call from the briefing). Favorites beyond the cap are not chipped in v1
(management UI is a later ticket).

**Reveal** (slide down, ~180 ms ease-out, host-side): the cursor enters the top
hot zone (the gap band above the surf zone, full surf width), OR Ctrl+L (which
also focuses + selects the input — unchanged). **Hide** (slide up, same ease):
the cursor leaves the union of hot zone + bar rect with a **~250 ms hysteresis**
(no flicker on grazing touches), OR a navigation commits, OR ESC. **Typing
exception:** while the input is focused and holds text (the prefilled URL counts,
so a keyboard user is never cut off), a mouse-out does NOT hide the bar — only
ESC, Enter (navigate), or a chip/suggestion click end it then. A **Ctrl+L reveal**
additionally only becomes subject to the mouse-out hysteresis once the cursor has
*engaged* the bar (entered it at least once), so a keyboard reveal is never
hidden before the user can type. ESC chain is now **bar -> settings -> quit**.

**Mechanics.** A tiny host-side state machine (a 0..1 slide `progress`, eased,
plus a hysteresis deadline and an "engaged" flag) drives the one shared internal
OSR view (mutually exclusive with the settings card, as before). The page renders
at the full bar size; the compositor reveals it from the top edge by
**scissor-clipping** the panel draw to `progress * height` — the bar is drawn with
square corners flush to the top, and the zone shadow dims only the visible slice.
The bar height is computed host-side from the shared theme tokens (`input_height`
+ the new `chip_row`, or `input_height + N*row_height + 2*list_pad`), so the page
CSS and the composite stay in lockstep as the body changes — no per-frame
allocation, no page-reported geometry. Two IPC additions carry the little the
host cannot derive: `autofocus` in `get_nav_state` (Ctrl+L vs hover) and a
`bar_typing` signal for the mouse-out exception (see wire-format). The loading
line stays at the surf zone's top edge; the bar lives above it in the gap and
composites over it only where a tall body overlaps the surf zone's top margin.

**Favorites bug (Stage A), measured first.** The CD-07 "only one favorite ever
shows" was diagnosed on the real DB before any code change (measure-before-
guessing): storage and the empty-input query are correct — two distinct URLs
produce two rows and `query_suggestions("")` returns both (verified on a scratch
DB and by a regression test). The fault was the display: the palette prefilled
its input with the current surf URL and filtered the suggestion list by it, so
only the favorite matching the current page ever showed; the D-0014 empty-input
favorites surface was never reached on open. The fix (committed separately as
`fix(command)`, not the briefing's `fix(memory)` — the memory layer was never at
fault) shows the full favorites list while the input holds the untouched
prefilled URL; the top bar's chips then make the favorites surface explicit.

## D-0015 - 2026-07-08 - CD-07: the settings "select" is a custom in-page dropdown

The search-engine setting needs a select control, but the internal views are
**off-screen (OSR)** and `RenderHandler::on_paint` only composites the main VIEW
element — native popup widgets (`PaintElementType::POPUP`) are deliberately
ignored (consistent with "no context menu" through Season 5). A native
`<select>` would open its option list as exactly such a popup, which would never
paint — the dropdown would be invisible.

So the "select control" is a **custom in-page dropdown**: a button plus an
absolutely-positioned `<ul>` menu, all ordinary markup that composites in the one
VIEW texture like everything else on the page. It also themes perfectly to the
token world (native option lists can't be fully styled anyway) and matches the
slider's design language. The menu opens downward within the settings card (the
search-engine row sits at the top, with room below). Reasoned deviation from the
literal "select"; the behaviour and look are a select, the mechanism is ours.

## D-0014 - 2026-07-08 - CD-07: the command palette IS the favorites/history surface

CyberDesk gains local memory — a `history` table (url, title, last_visit,
visit_count) and a `favorites` table (url, title, added_at, position) in the same
`state.db` (schema v2). Recorded on the surf view only; `cyberdesk://` and blank
navigations never enter either table. No sync, no export.

**No favorites bar — the command bar becomes a command palette.** The deliberate
design law: CyberDesk shows NO favorites bar and NO browser-chrome imitation. The
one command surface (`Ctrl+L`) is where favorites and history live — as live
suggestions below the input. A visual favorites surface with its own buttons is
**Season-2 design-law material**, not this ticket. `Ctrl+D` (or the command-bar
star) favorites the current page; the star reflects and toggles the current
page's state live.

**History cap + pruning.** History is capped at **~10,000 rows**; each insert
prunes the least-recently-visited rows past the cap (`DELETE … ORDER BY
last_visit DESC LIMIT -1 OFFSET 10000`). A visit is one upsert per real address
change (bump `visit_count`, refresh `last_visit`); the title is filled in when it
arrives (it lands after the address commit).

**Frecency (kept honest and simple).** History suggestions rank by
`visit_count * recency_weight`, where the weight is bucketed by the age of the
last visit: `<1 h → 100`, `<1 day → 80`, `<1 week → 60`, `<30 days → 40`, else
`20`. Favorites always outrank history and are shown first (in their saved
order); a favorite is excluded from the history half so it appears once. Matching
is a case-insensitive substring on url + title, with LIKE wildcards in the input
escaped. The whole ranking and matching runs **host-side** in the IPC handler;
the page only renders what it is given (query per keystroke, debounced ~90 ms).

**Palette sizing.** The palette view is resized to fit exactly `input bar + N
rows` (grows and shrinks with the live suggestion count, primed on open). The row
and input dimensions are theme tokens (`[command]` in `theme.toml`), emitted as
`--cmd-*` CSS vars, so the page CSS and the host-side rect share one source of
truth — no hardcoded sizes, and no favorites-area scrim over empty space.

## D-0013 - 2026-07-08 - CD-06: depth overhaul, ring removed, feather corrected, autonomous push

Sascha's verdict on the CD-05 visuals: the background looked like "800x600 Amiga
times" — far too little content for a 5120x1440 canvas (~1.2k primitives), the
effect too predictable, and the rotating ring in the middle still ugly. CD-06
fixes all three, plus the parked feather correction, and changes the push policy.

**Ring removed from the shell.** The rotating CARVILON ring no longer renders in
the shell pass or the `--capture` path (the capture composites the background
faithfully since CD-05, so it needs no ring backdrop). The shell background is
the Pulse Grid alone. The ring shader/module (`ring.wgsl`, `RingUniforms`,
`ring_pipeline`) stays in the tree **dormant** (`#[allow(dead_code)]`) — its
future is the start animation and the Energy Core interaction motif (Season 2),
so it is demoted, not deleted.

**Depth-layer architecture (the 10x).** The Pulse Grid is now **three depth
layers** — far → mid → near — each its own generated board, all baked additively
into the one HDR texture (draw order is cosmetic). At ultrawide this lifts the
content from ~1.2k to **12,424 baked primitives** (~10x), baked in **3.6 ms**
(still imperceptible; single-digit ms as required). Rationale: one flat layer at
any density still reads flat and repetitive; real depth (a crisp bright front, a
dimmer middle, a faint fine recede) is what makes a 1.2 m canvas read as *deep*
rather than merely busy.
- **Far**: finest lattice (~half the near cell), ~4x the near trace count,
  thinnest lines, ~0.36 brightness. **Mid**: between the two (~0.68 cell, ~2x
  count, ~0.6 brightness). **Near**: the CD-05 scale/brightness — the crisp
  bright front; the two bus lines and the flare-anchor pads live here.
- **Per-layer seeds** derive deterministically from `background.seed` (three
  sub-seeds pulled from a master splitmix64), so the determinism contract holds
  across launches (verified: byte-identical captures). The micro-lattice now
  sums three weaves (far/mid/near cells) in a single fullscreen pass.
- **Component vocabulary** kills the uniform random-walk predictability: **chip
  footprints** (outline rectangle + pin-pad rows on 2 or 4 edges, near/mid),
  **via clusters** (3–8 filled dots, all layers), **junction hubs** (pads with
  several traces routed toward them), and **varied segment distribution** (short
  zigzags mixed with occasional long straight runs, especially on far).
- **Life across depth**: pulse count, speed, brightness and head size scale per
  layer (near bright/fast, mid fewer/dimmer, far sparse/slow/faint — depth in
  motion); node flares stay near-layer. The HDR bake target and the zone shadow
  are unchanged and keep working across all three layers. All counts, sizes and
  scales are theme tokens; the generator kept the CD-05 instance/sprite pipeline
  (more primitives and vocabulary, not a new renderer).

**Feather corrected (parked CD-05 verdict).** The 34 px smoothstep feather read
as a 3D/vignette curve — the page was already >50% transparent 16 px inside its
edge and faded over a wide creamy band, so it seemed to curve away. The band is
narrowed to **12 px** (`feather_width`) and a **falloff curve exponent** token
(`feather_exp = 0.45`) applies `pow()` to the edge coverage in `page.wgsl`: the
page now stays fully opaque until ~10 px from the edge, then fades over the last
few pixels (0.55 at 4 px, 0.17 at 1 px, 0 at the edge) — a light, casual soften,
steep not creamy, with AA preserved at the boundary. The OFF state (hard 16 px
rounded corner) is untouched and the toggle still switches live in the one page
pipeline.

**Push policy (permanent, from CD-06 on).** Push **per stage, autonomously,
never ask.** The pre-push secret/IP grep stays mandatory before every push; only
the asking stops. If a push is denied by the tool permission system, note it and
continue — do not stall a stage on it. CLAUDE.md carries this rule.

## D-0012 - 2026-07-08 - CD-05: background v2 "Pulse Grid"

The Deep Field (CD-03) is too dark for the Cyber look. Its replacement is the
**Pulse Grid**: a fine circuit-board weave (micro lattice, routed traces with
pads and solder dots, two full-width bus lines) with light pulses travelling the
traces and occasional node flares. It becomes the Cyber default; the Deep Field
is **demoted, not deleted** — it survives intact as the future "Calm" template
variant, selectable via the `background.kind` token (`"pulse_grid"` |
`"deep_field"`).

**Amplitude-spec supersession.** The Deep Field's brightness discipline (the
6-8 % amplitude cap) does NOT apply to the Pulse Grid. This background is allowed
to glow. Content readability is protected by the **zone shadow** (the background
multiplies down toward `zone_shadow` under the surf zone and the open overlay,
with a soft feathered edge) instead of global darkness — glow in the margins,
calm under the page.

**Seed determinism.** The board is generated by a dependency-free splitmix64 PRNG
seeded from `background.seed`. Given the same seed, frame size and DPI scale, the
layout is identical across launches — it feels like YOUR board, not random noise
per boot. The life layer (pulses/flares) runs off the same PRNG but is
deliberately outside the determinism contract (only the static board must match).

**Bake-once architecture.** The static layer (lattice + traces + pads + dots +
bus) is rendered ONCE, at startup and on resize/seed change, into a full-
resolution offscreen texture; each frame composites it as the backmost layer,
scaled by the glow-intensity uniform. The bake is imperceptible (~0.5 ms at
1600x900, ~0.8 ms at 5120x1440). Thin lines stay crisp because the bake is full
res, not half res (the Deep Field's half-res+blit economy was tied to its
per-frame procedural cost; a baked static layer costs zero per frame). Reasoned
deviation: the bake target is **Rgba16Float** (HDR) so glow above 1.0 survives
the up-to-2.2x intensity scaling without banding.

**Settings.** The background toggle is renamed `deep_field` -> `animated_background`
(it now governs whichever background the template selects); the store migrates
the old key's value across. A new **glow-intensity** slider (50-220 %, default
from the `background.glow_default` token) is applied live and persisted; the
`set_setting` IPC now accepts a numeric value for that key (see wire-format).

**Self-test.** `--capture` now renders the full shell (Pulse Grid + ring) with
the ring on its transparent path (`is_srgb = 0`), so the PNG matches the on-
screen framebuffer and the circuit can be eyeballed without screen-scraping the
desktop. `CYBERDESK_CAPTURE_SIZE=WxH` and `CYBERDESK_CAPTURE_GLOW=<mult>` size and
brighten it for headless verification (e.g. the ultrawide target, or the 220 %
readability check).

## D-0011 - 2026-07-08 - CD-04: gesture-aware popup policy

The surf view's `LifeSpanHandler::on_before_popup` always returns `1` — no
separate browser window is ever created. When the popup carries a genuine user
gesture (`user_gesture != 0`) and the source is the surf zone, the target URL is
loaded into the surf view's **own main frame**; popups without a user gesture are
suppressed outright.

**Why the user gesture is the discriminator.** Two earlier extremes both failed:

- CD-01 navigated the surf view on *any* popup. A foreign session's ad/script
  `window.open` then hijacked the view (the foreign-session ad hijack).
- CD-02/03 suppressed *all* popups. That killed legitimate `target=_blank` links
  and click-to-open flows — clicking such a link did nothing.

CEF's `user_gesture` flag cleanly separates the two: a real click that opens a
link is a gesture; an ad/script `window.open` fired from a timer or load handler
is not. So gesture -> navigate in place; no gesture -> drop. Either way no new
window opens, which also preserves the single-surface shell model.

**Scope.** The navigate-on-gesture branch is gated on `Role::Surf`. The internal
views are already navigation-isolated (D-0010) and never spawn popups; the return
value still suppresses any window unconditionally.

## D-0010 - 2026-07-08 - CD-03: internal view uses a `cyberdesk://` custom scheme

The settings view is a second OSR browser locked to a registered custom scheme,
`cyberdesk://settings/`, rather than a reserved web host or a `data:` URL.

**Why a custom scheme.** It gives the internal UI a real, standard, secure
origin (registered via `on_register_custom_schemes` with STANDARD | SECURE |
CORS | FETCH), which is a clean security context for the message-router IPC and
lets isolation be expressed as a simple scheme check. A `data:` URL has an opaque
origin and awkward sub-resource semantics; a reserved web host would blur the
web/internal boundary we are trying to make absolute.

**Served entirely in-process.** A `SchemeHandlerFactory` + `ResourceHandler`
serve the settings document straight from embedded bytes (HTML with the theme
tokens, CSS, and JS inlined — a single document, zero sub-resource requests).
Nothing touches the network.

**Hard web isolation (D-0004).** The internal view's `RequestHandler::
on_before_browse` cancels any navigation whose URL is not `cyberdesk://`. Verified
with an opt-in self-test (`CYBERDESK_ISOLATION_SELFTEST=1`) that steers the view
at `https://example.com/` and confirms the block fires and the view stays put.

**IPC only on the internal view.** `window.cefQuery` is registered by the
renderer-side message router only for `cyberdesk://` V8 contexts, and only the
internal client forwards router messages browser-side. The surf zone never sees
the bridge. Wire format: docs/cyberdesk-wire-format.md (Settings IPC).

## D-0009 - 2026-07-08 - CD-02: accelerated OSR researched, CPU path kept for now

CD-02 ships CPU off-screen rendering: `RenderHandler::on_paint` delivers BGRA, we
upload it into a wgpu texture and composite it. This records the research into the
accelerated (zero-copy GPU) path and why we stay on CPU for now.

**GPU-process finding (good news).** The CD-01 release-only GPU sub-process crash
(D-0008c) does NOT occur under OSR. Release OSR runs with a healthy GPU process -
no STATUS_BREAKPOINT, no SwiftShader fallback. Reworking the presentation path
(OSR) resolved it. So both the CPU path (verified) and a future accelerated path
are viable on a working GPU process.

**The accelerated path exists in cef-rs.** Set `shared_texture_enabled` (and
`external_begin_frame_enabled`) in WindowInfo, handle
`RenderHandler::on_accelerated_paint` (whose `AcceleratedPaintInfo.shared_texture_handle`
is a D3D11 shared HANDLE), and `cef::osr_texture_import::SharedTextureHandle::import_texture(&wgpu::Device)`
imports it via the wgpu-hal DX12 escape hatch (`as_hal::<Dx12>`, open the D3D11
handle as a D3D12 resource, `texture_from_raw`, `create_texture_from_hal`) - exactly
the dx12 external-resource path.

**Concrete blocker.** cef-rs's importer (behind the `accelerated_osr` feature) is
built against **wgpu 29**; CyberDesk uses **wgpu 30**. wgpu `Device`/`Texture`
types are version-specific, so cef's `import_texture(&wgpu29::Device)` cannot
consume our wgpu-30 `Device` nor yield a texture usable in our wgpu-30 pipeline,
and enabling the feature would pull in a second, conflicting wgpu.

**Options.** (a) Replicate cef's ~100-line D3D11 importer against wgpu-30's hal
(open the shared HANDLE as a D3D12 resource via the `windows` crate, wrap with
`wgpu::hal::api::Dx12` `texture_from_raw` + `create_texture_from_hal`; enable wgpu's
dx12 hal feature). Keeps wgpu 30. (b) Pin the whole app to wgpu 29 to use cef's
helper directly - rejected, it regresses our stack.

**Decision.** Stay on the CPU path for CD-02. It is verified working (release,
healthy GPU process, sharp DPI text, full mouse/keyboard input, scrolling), and the
readback cost is acceptable at this stage. The accelerated path is well-scoped for a
focused follow-up (option a) once feathering (CD-03) makes per-pixel throughput
matter - a working GPU process under OSR means it will pay off. Documented and
stopped within the CD-02 time-box.

## D-0008 - 2026-07-08 - CD-01 reality notes: sandbox deferred, isolated cache, GPU fallback

Three findings from the CD-01 build, recorded honestly. (a) OS sandbox deferred: the app runs with no_sandbox because the cef-rs Windows sandbox requires the bootstrap.exe launcher model, which breaks the plain cargo-run acceptance path. This is a time-boxed deviation from the iron law and D-0006, with a hard re-enablement gate before Season 5/6 (before the browser becomes daily-use and before crypto lands). (b) Isolated browser profile: CEF gets its own root_cache_path - the surf zone never shares state with any user-installed browser. This fixed a real incident where the embed picked up the user's Chrome session, and resolved profile-singleton GPU-process collisions. (c) Release-only GPU subprocess crash (STATUS_BREAKPOINT) with automatic SwiftShader software fallback - page renders correctly. Root-cause work deferred to CD-02, which reworks the entire presentation path (OSR).

## D-0007 - 2026-07-08 - Edit Mode instead of free windows

The fixed layout is the normal state. Position changes only in an explicit Edit Mode and only within the grid rules (allowed slots, snapping). Afterwards the layout locks again. Reasoning: predictability is the core of the product; controlled adjustment yes, layout anarchy never.

## D-0006 - 2026-07-08 - High-performance doctrine

In trade-offs, the better, harder path beats the compromise and the shortcut (Sascha's directive, binding). Applies to architecture and quality decisions. Guardrail: the doctrine governs the quality of the path, not the scope - we still build in stages.

## D-0005 - 2026-07-08 - No GPL linking in the proprietary core

libsigrok and similar (GPLv3) are never linked. Logic analyzer: own FX2 driver via rusb, own decoders (UART, I2C, SPI first). Unmodified GPL firmware (fx2lafw) may ship as a separate file with a source notice - it runs on the device, not inside our process.

## D-0004 - 2026-07-08 - NetGuard principle

No network access except through the central NetGuard layer. Deny-by-default per zone, certificate pinning per destination, own DNS resolver, kill switch, logging (hash chain later). Binding in every briefing from CD-02 on.

## D-0003 - 2026-07-08 - Debian 13 "Trixie" as OS foundation (long-term goal)

For the later CARVILON OS: Debian stable instead of Ubuntu - trademark-neutral, lean start without foreign branding, live-build is Debian's own tool, updates keep flowing from Debian sources (no fork). CyberDesk becomes its shell.

## D-0002 - 2026-07-08 - CEF binding: cef-rs (pinned in CD-01)

The cef crate (tauri-apps/cef-rs), pinned: cef = "=149.3.0" -> CEF 149.0.6+chromium-149.0.7827.201, windows64 minimal distribution. Chosen for pre-generated bindings (no libclang required), the CMake+Ninja wrapper build against already-installed tooling, RuntimeStyle::ALLOY (chromeless page surface) and set_as_child windowed embedding for CD-01. See D-0008 for CD-01 reality notes.

## D-0001 - 2026-07-08 - Rust host + CEF instead of Electron/Tauri

Crypto and start authorization belong in the memory-safe Rust process: Argon2id and Zeroize are practically impossible to implement cleanly in a JS runtime (V8, garbage collector). The surf zone requires offscreen rendering (page as GPU texture with soft edges) - Tauri's system WebView cannot do that. CEF is Chromium without Node and without the npm chain; the Chromium sandbox stays active. Fairness note: modern Electron with correct defaults is better than its reputation, but satisfies neither the Zeroize nor the OSR requirement.
