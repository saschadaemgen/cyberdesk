# CyberDesk - Security

Project CARVILON CyberDesk - living document - Status: 2026-07-21 (through CD-40 Stage 1 / D-0061)
Maintained by Claude Code (CC), updated in the same commit-set as the code it describes (D-0053).

## Iron law

The surf zone (CEF) has no path to CARVILON functions (doors, cameras, time clock) by design. No IPC route exists from the web renderer to control commands. Separation by architecture, not by filter.

## Process boundaries and IPC

- Rust host and CEF renderers are separated by a hard process boundary.
- The Chromium **OS sandbox is currently deferred** (D-0008: the cef-rs Windows
  sandbox requires the bootstrap.exe launcher model, which breaks the plain
  cargo-run path). This is a **tracked, time-boxed deviation** from the
  "sandbox stays active" doctrine with a hard re-enablement gate before Season
  5/6 (before daily-use browsing and before crypto lands) — recorded here so
  the security doc never overstates the live posture.
- IPC exclusively through an explicit allowlist of named commands (the full
  schema is cyberdesk-wire-format.md). The bridge (`window.cefQuery`) exists
  ONLY on `cyberdesk://` frames — a web page has no IPC surface at all.
- No generic eval or passthrough channels.

## Vault: keys and authorization (Season 6 crypto, CD-40 D-0058; unlock model CD-42 D-0062)

The crypto core (1a, D-0058), the start-authorization gate (1b, D-0059) and
the config surface (1c, D-0060) are live (`src/vault.rs`, `app.rs` gate,
`cyberdesk://lock/`, the settings vault section, the HUD Vault field); CD-42
(D-0062) set the authoritative unlock model on top: a **mandatory master
password** as the sole root, an optional passkey as the only additional
factor, **no recovery key**. The passkey-PRF layer is source-verified and
honestly deferred (D-0061 — see the passkey caveat below). The standing laws
hold **by construction**: no key material in memory before authentication
(the app boots into a lock view — no slots, no MF zone, no HUD — and the
workspace is created only after the VMK exists; on first launch that view IS
the mandatory master-password setup), and no key material in the WebView,
ever — while a secret is being entered, the HOST consumes the keyboard
directly into locked memory and the page renders dots from a pushed
character count; a renderer process never holds a keystroke of a vault
secret.

The model, precisely:

- **Envelope key management.** One random 256-bit Vault Master Key (VMK)
  protects the vault's sensitive data; it is never derived from any single
  factor. The master password wraps the VMK in an XChaCha20-Poly1305
  envelope via Argon2id (RFC 9106 second recommendation — 64 MiB, t=3, p=4 —
  stored per method, re-tunable); the optional passkey's WebAuthn PRF secret
  joins only as the 2FA pair member. Enroll/remove/rotate re-wraps the VMK;
  the vault data is never re-encrypted.
- **The unlock policy is structural — and exactly two shapes exist
  (D-0062).** Password-only has the single envelope `{password}`; password +
  passkey (2FA) has the single envelope `{password, passkey}`, keyed by a
  combined (BLAKE2s, domain-separated) key of both members. The master
  password is a member of EVERY envelope, so a passkey alone can never open
  anything — it is an additional factor, never a replacement. An attacker
  editing `required` in `vault.json` gains nothing — no other envelope
  exists to open. Unlock failure is one uniform error: wrong password and
  tampered blob are indistinguishable (no oracle).
- **No recovery key, no backdoor — the honest consequence (D-0062).** The
  master password is the sole 1-factor recovery. A forgotten master password
  — or a lost passkey while 2FA is required — makes the vault
  **unrecoverable, by design**: a deliberate no-backdoor stance, not a bug,
  and it is stated plainly on the setup screen so the choice is informed.
  (The CD-40 "never-brick" rule — a mandatory non-hardware fallback — was
  retired with the recovery key; under 2FA the user has explicitly accepted
  hardware loss as vault loss.) The structural invariants — exactly one
  master password, at most one passkey, the envelope shape matching the
  policy — are enforced on every re-wrap AND on every load; a violating
  (hand-edited, corrupted) file is refused. The offline brute-force surface
  of `vault.json` is the password envelope at the stored Argon2id cost.
- **Escrows.** Each method's wrapping key is also stored wrapped *under the
  VMK*, so enrolling a passkey / changing the policy works from an unlocked
  session without re-prompting every factor. (At password-only an enrolled
  passkey exists ONLY as an escrow — no envelope — ready for the 2FA
  switch.) Honest assessment: an attacker who obtains the VMK could also
  read the current wrapping keys — but the VMK already decrypts everything
  the vault protects, so this adds no capability; rotating a method replaces
  its escrow.
- **Memory hygiene — the CD-33-deferred Tasks C/D are CLOSED for vault
  keys.** All key material lives in dedicated `VirtualAlloc`ed,
  `VirtualLock`ed pages (never the pagefile), zeroized before unlock/free on
  drop; allocation fails closed if the pages cannot be locked. AEAD runs
  in-place (plaintext never transits cipher-crate allocations); the Argon2
  block matrix is caller-owned and zeroized after derivation. Bounded
  residuals (internal scope, never surfaced, D-0044): transient stack copies
  inside the crypto crates, the Argon2 matrix *during* a derivation, and
  hibernation (`hiberfil.sys` snapshots even locked pages). The live
  pagefile spot-check is the maintainer's acceptance step.
- **Sealed app state.** Sensitive app state is sealed under the VMK
  (`vault.seal`, AEAD with its own AAD domain) and decrypted only after
  unlock; non-sensitive layout state stays in `state.db` (do not seal what
  does not need sealing).
- **PIN honesty.** A PIN is never a standalone vault envelope (too
  low-entropy for an offline-attackable wrap). It belongs to the
  authenticator (Windows Hello / security-key gate inside WebAuthn) or, at
  most, to a later in-session quick-unlock that keeps the VMK in protected
  memory.
- **Passkey-PRF caveat (verify-first — DETERMINATION, CD-40 1d, D-0061).**
  WebAuthn PRF maps to CTAP2 `hmac-secret`; the PRF-derived secret would wrap
  the VMK as a device-bound envelope, enrolled only from an unlocked session.
  Source-verified state on the target (Windows 11, build 26200):
  - The Win32 `webauthn.dll` client API is fully present in `windows-sys`
    0.61.2 — **already in our dependency tree** (pulled by `cef` + `arti`),
    behind the `Win32_Networking_WindowsWebServices` feature, so enrolling
    needs a feature-enable (or `webauthn-authenticator-rs`), not a new crate
    download. The binding is at **API version 7**; the raw hmac-secret salt
    path (`WEBAUTHN_HMAC_SECRET_SALT` + `WEBAUTHN_AUTHENTICATOR_HMAC_SECRET_VALUES_FLAG`)
    is present at v7.
  - **Security keys (e.g. YubiKey) via CTAP2 `hmac-secret`: supported
    reliably** through that v7 path — the established, buildable route.
  - **Windows Hello platform-authenticator PRF is in flux:** it landed only
    in the **February 2026 cumulative update (KB5077181, Windows 25H2 build
    26200.7840+)**, and the convenience "PRF eval" path needs **API version
    8**, which the pinned `windows-sys` 0.61.2 (v7) does not expose. Our
    target reports build 26200 (25H2) but whether KB5077181 is installed is a
    live-machine fact — the maintainer's check.
  - The native `WebAuthNAuthenticatorGetAssertion` call needs a real HWND, a
    physical authenticator, and a user-presence gesture; it cannot run in any
    headless/unit path, so its only verification is a live run (Sascha's, per
    acceptance #7).

  **Determination:** PRF is not *dependably* available on the target as a
  blanket guarantee — Windows Hello needs a specific KB + an API-v8 crate we
  do not pin, and the security-key path, while buildable, cannot be verified
  even once without hardware + a live run. So the passkey sub-stage is an
  **honest, flagged deferral** (acceptance #3 sanctions this explicitly). What
  is already proven ready: the envelope layer treats a passkey as an opaque
  32-byte method secret — `vault::enroll_passkey` + `Factor::MethodSecret`
  are implemented and unit-tested under the D-0062 model (the passkey
  enrolls from an unlocked session as the only additional factor, never
  unlocks alone, and pairs with the password under 2FA). The remaining,
  bounded work: enable the WebAuthn feature (or add
  `webauthn-authenticator-rs`), turn a real authenticator's PRF output into
  that 32-byte secret via a persisted per-vault salt, wire it into the
  existing `enroll_passkey` seam, and add the passkey assertion step to the
  2FA unlock flow at the gate. The foundation (master password + gate +
  config surface + memory hygiene) ships now and never waits on it. Until it
  lands, no passkey can be enrolled, so the 2FA policy cannot be enabled —
  the host refuses `required = 2` without an enrolled passkey, which also
  means no unlockable-only-with-hardware state can arise before the unlock
  flow can serve it.
- **The gate, precisely (1b, D-0059; mandatory setup, CD-42).** A closed
  gate creates ONLY the lock view — unlocking an existing vault, or the
  mandatory first-launch master-password setup when none exists (no skip, no
  default: the workspace cannot boot before the vault does). Unlock/setup
  derivations run on worker threads; "Lock now" wipes key material and
  relaunches the process cold (provable teardown of every renderer — no
  in-process CEF lifecycle edge cases). A vault file that fails validation
  keeps the gate CLOSED with an honest message: corruption or tampering must
  never bypass authorization (a retired v1 recovery-key-model file gets a
  specific reset message — dev data only, sanctioned by the CD-42 briefing).
  Every deliberate exit path wipes key material explicitly. The identity
  seed (fingerprint linkage material) is the sealed store's first tenant:
  with a vault present it exists only inside `vault.seal`, migrated out of
  plaintext at setup, and is never readable — with no plaintext fallback —
  before unlock.
- **Config surface (1c, D-0060; restricted by CD-42).** Every knob is
  visible and settable in settings: enrolled methods, the password-only /
  password+passkey policy, the Argon2id cost, change-master-password, Lock
  now; the HUD tile shows the vault state (a dev bypass is loudly warned).
  No recovery-key control exists anywhere. Weakening — dropping 2FA, or an
  Argon2id cost below the RFC default or the current setting — is
  HOST-refused without a confirmation flag; a page bug cannot quietly
  cheapen the offline brute-force surface. A KDF re-tune verifies the
  captured password against the existing envelope before re-deriving, so a
  typo can never silently become the new password. Change-master-password is
  VMK-authorized (session possession = vault control — recorded reasoning in
  D-0060).
- **Dev bypass honesty.** `CYBERDESK_VAULT_BYPASS=1` exists only under
  `cfg(debug_assertions)` — a release artifact contains no bypass code path.
  It skips the gate for the dev loop; it cannot produce the VMK, so sealed
  state stays sealed even under the bypass.

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

## Onion routing (CD-35, D-0052)

`.onion` addresses open in **Tor windows** via arti's embedded onion-service
client (the stable `onion-service-client` feature of the pinned 0.43). The
property, worth stating plainly: **onion sites are resolved inside Tor, never
through clearnet DNS** — onion resolution is the hidden-service rendezvous
protocol, there is no DNS query of any kind and no exit node, so onion browsing
has no exit-node exposure. Per-window circuit isolation carries over to onion
circuits (arti keys the HS tunnel by the client's isolation).

Clearnet windows **refuse** `.onion` without leaking, in three fail-closed
layers: the address bar reroutes to the honest refusal page before any resolver
is consulted; the per-slot request handler cancels `.onion` navigations
(including top-level redirects; the FQDN trailing-dot spelling is caught too);
and a context-level guard on the ephemeral clearnet context cancels any
remaining `.onion` request on the IO thread and rewrites redirect-targets to an
inert `about:` URL — covering subresources, XHR, and worker requests that have
no per-browser handler. Chromium itself implements no RFC 7686 special-casing;
CyberDesk enforces the split. The refusal page offers the Tor path (new Tor
window / switch this window) — no dead end, no silent failure.

Ephemerality (CD-33/34) applies unchanged: Tor contexts are in-memory, the
refusal page is excluded from the RAM-only history, and `cyberdesk://` URLs are
never persisted — onion browsing leaves no disk trace.

Shipped scope is **open `.onion` addresses** (client only). Later phases, named
and deferred: Onion-Location auto-switch, client authentication, `.onion`
certificate/TLS handling. Onion-service **hosting** (serving a `.onion`, not
just visiting) was out of scope through CD-35 and is now **under evaluation** —
an isolated, env-gated feasibility probe (CD-37; the formal decision lands with
that ticket's D-number). It remains client-only in shipped builds.

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
  profile regardless of what our views use. During a session it is **empty scaffolding**:
  a `History` file with zero rows, a `Cookies` file with zero rows, and no occurrence of
  any visited host anywhere beneath it (measured). It is not browsing content, but it
  is not *nothing* — a scan mid-session will still show Chromium-shaped filenames. **CD-34
  (D-0051) wipes the whole directory on every launch**, so across launches even this
  scaffolding does not persist.
- **Pre-existing residue — CLOSED by CD-34 (D-0051).** CD-33 stopped the writing but did
  not delete what earlier builds already wrote (on the development machine, 79 MB of
  cache, 21 URLs / 254 visits, 36 cookies). `state.db`'s history was purged by the v7
  migration; the CEF profile residue is now cleared by the **standing on-launch purge** —
  allowlisted to the one `cyberdesk-cache` directory, never touching Tor state, session,
  or config. It is also a regression backstop: any future accidental disk leak survives
  at most one session. The purge runs before `init_cef` (the only moment CEF does not
  hold the profile's files open) and is a global, settable option (default ON) with a
  live footprint readout; turning it off routes through the D-0040 gate.
- **The pagefile is addressed by keeping secrets out of it, not by disk encryption.**
  Disk encryption is transparent on a running, unlocked machine and is therefore *not*
  the control against a running-system attacker; the control is that sensitive data
  never reaches the disk in the first place. Since CD-40 (D-0058) the vault's key
  material is additionally `VirtualLock`ed out of the pagefile and zeroized on drop —
  the CD-33-deferred Tasks C/D, closed against a real secret. See the vault section
  above.
- **Tor state persists by design** (`%LOCALAPPDATA%\CyberDesk\tor\state`). Entry-guard
  persistence is an anonymity *feature* — rotating guards every session raises exposure
  to a malicious guard. It is Tor's own security state, deliberately distinct from
  browsing content, and is not a forensic defect. It does, however, evidence *that* Tor
  was used (not where you went).
- **Session restore is opt-in and honest.** "Quit & Save" persists layout, per-slot
  mode, and — for **clearnet windows only** — URLs; a Tor window's URL is **never**
  written (it returns as a real Tor window on the start page, D-0035). Never cookies,
  cache, or content — a restored session brings back the tabs but **not** the login
  state; you come back logged out. Plain Quit persists nothing. Storing this metadata
  encrypted-at-rest is open (it is currently plaintext `state.db`), and it is the one
  place a visited URL can reach disk — by explicit user action.
- **`favorites` is on disk by intent**, and a favorite is a URL. It records what the
  user chose to keep, not where they have been; this is the bookmark/history split every
  ephemeral browser makes. Worth knowing it is there.

## Supply chain

Pinned dependencies, cargo-audit and cargo-deny in the workflow, no GPL linking (D-0005), CEF version pinned exactly, large binaries never in the repo (fetch script).

## CRA (Cyber Resilience Act)

Reporting obligations from September 2026, full compliance December 2027. Built in from the start instead of retrofitted: update capability (signed updates planned), SBOM generation, incident logging (hash chain later), documented vulnerability disclosure path.

## Repo hygiene

Pre-push grep against real IPs, hostnames, and secrets before every push. Test data uses placeholders only (documentation IPs such as 203.0.113.x). Repo stays private.
