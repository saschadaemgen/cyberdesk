# CyberDesk - Feature Backlog

Project CARVILON CyberDesk - living document - Status: 2026-07-20
Maintained by Claude Code (CC), updated in the same commit-set as the code it describes (D-0053).

Soft season mapping - chat fill level and reality decide the actual cuts. Completed items move into the respective season protocol.

## Principles & season orientation

(Folded in from the retired `cyberdesk-roadmap.txt`, CD-36 / D-0053 — one
source of truth for the season→feature mapping.)

Principles:

- Risk first: the unproven moves to the front.
- Security before integration: no door control before authorization stands.
- D-0006: The better, harder path beats the compromise. Governs the quality of
  the path, not all-at-once scope.
- Season end: season protocol (including failures and dead ends) + chat
  handover + update of the living docs. Stored in `seasons/` AND in the chat
  project. (Doc updates themselves are continuous per D-0053 — every change in
  the same commit-set; the season-end pass is the review, not the catch-up.)

Season orientation (terse; numbers are orientation, not a contract):

| Season | Theme | Anchor |
| --- | --- | --- |
| S1 | Foundation + risk proof | CD-01 shell+CEF, CD-02 OSR texture, CD-03 feathering — **shipped** |
| S2 | Design law | color world, motion language, shaders, drag-and-drop language (CD reference binding) — running; privacy/Tor/hardening arc CD-14..CD-35 shipped within it |
| S3 | Zone engine | grid, S/M/L reflow, tab rail, Edit Mode |
| S4 | Modes + Event Engine | Standard/Admin/DND, priority x mode, simulated events |
| S5 | Browser becomes CARVILON | favorites/history (SQLite — shipped CD-07), downloads, context menu, request filter |
| S6 | Crypto + authorization | Argon2id, Zeroize, encrypted state, start authorization |
| S7 | CARVILON integration | cameras, doors, status, time clock (NetGuard rules Edge/VPS) |
| S8+ | Tools | terminal, editor, explorer, FTP, logic analyzer, NetGuard monitor |
| S9+ | Everyday | music, SimpleX, email, calls, calendar/day planner, office unit |

Long-term goal: CARVILON OS (Debian 13 Trixie) - CyberDesk boots as the shell.
Nothing on the app path is throwaway work.

## Foundation (Season 1 — shipped)

- CD-01 shell skeleton (winit/wgpu, fullscreen, rotating ring, ESC, --windowed) + CEF windowed with google.com [with CC]
- CD-02 OSR PoC: web page as GPU texture inside our own frame
- CD-03 feathering/compositing: soft edges, animated background behind the content

## Design (Season 2)

- Design law: color world, motion language, background shaders - reference files by CD (Claude Design) are binding, CC implements them 1:1
- Pulse Grid activity coupling (deferred from CD-05): drive the background pulses/flares from real system activity - NetGuard traffic, MQTT ticks, event-engine signals - instead of the current autonomous animation. Belongs after NetGuard/events exist (Season 4+). Also earmarked: a template picker UI to switch the Pulse Grid ("Cyber") and Deep Field ("Calm") backgrounds, which are currently token-selectable only (D-0012).
- Drag-and-drop as a global design language: ghost element, zone highlight on hover, magnetic docking, spring animation (specification CD, implementation CC)
- App start animation (logo rotation); Plymouth theme stays earmarked for the OS path

## Zone engine (Season 3)

- Fixed grid: Spine, main area S/M/L with reflow, video zone, terminal zone, right tab rail (Status, Files, FTP, Music)
- Ultrawide layout as primary target, 16:9 as fallback
- Edit Mode (D-0007): allowed slots, snapping, locking afterwards

## Modes + events (Season 4)

- Modes: Standard, Admin (larger main area), Do-Not-Disturb
- Priority Engine: event x zone rank x mode -> override, overlay, or suppress; simulated events first

## Browser becomes CARVILON (Season 5)

- Favorites and history in SQLite, own buttons away from the content — **shipped
  early** (CD-07 command palette + favorites; history RAM-only since CD-33)
- Downloads land in the Files zone; context menu, JS dialogs, and popups in our own design
- Request filter as adblock foundation (filter lists on the network level)
- Later: Widevine for streaming DRM (plan for Google's license process), autofill via the Rust password core

## Crypto + authorization (Season 6)

- **Vault — foundation SHIPPED (CD-40, D-0058/D-0059/D-0060/D-0061);
  authoritative unlock model SHIPPED (CD-42, D-0062); passkey wiring
  VERIFIED+DEFERRED**: crypto core (`src/vault.rs`) — envelope key management
  (one random VMK; Argon2id master-password envelope; passkey-PRF as the only
  optional additional factor), escrow-based re-wrap from an unlocked session,
  sealed app state under the VMK, `VirtualLock`ed + zeroized key memory
  (closes the CD-33-deferred Tasks C/D for vault keys). The D-0062 model:
  **mandatory master password at first launch** (the gate boots into setup —
  no vault, no workspace), unlock policy exactly password-only or password +
  passkey (2FA, both required), **no recovery key, no backdoor** — a
  forgotten master password means an unrecoverable vault, by design, stated
  plainly at setup. Start-authorization gate — closed-gate boot shows only
  `cyberdesk://lock/` (unlock or first-launch setup), HOST-captured secret
  entry (no renderer ever holds a keystroke), "Lock now" via cold relaunch,
  identity seed sealed as the first tenant, `debug_assertions`-only dev
  bypass. Config + tile surface — enrolled methods, password-only /
  password+passkey policy (host-gated weakening), Argon2id cost re-tune
  (verified before re-derive), change-master-password, HUD Vault field; no
  recovery-key controls exist. Passkey via WebAuthn PRF — source-verified and
  HONESTLY DEFERRED (D-0061): security-key CTAP2 hmac-secret is buildable via
  the in-tree Win32 API (windows-sys 0.61.2, v7); Windows Hello PRF needs the
  Feb-2026 KB5077181 + API v8 (not pinned) and the native call needs a live
  run — so the determination + the proven envelope seam (enroll_passkey,
  unit-tested) + an honest "coming" UI ship; the bounded native wiring (and
  the 2FA passkey step at the gate) lands once PRF is confirmed on the
  device. Until then no passkey can be enrolled and 2FA cannot be enabled
  (host-refused). If a 2FA-loss safety net is ever wanted, an optional
  recovery key is a small additive change — deliberately out of scope
  (D-0062). Auto-lock via the event engine is a later stage (event-engine
  dependency).
- File vault (quantum-resistant): encrypted file store integrated into the Files zone, format-agnostic (any file type and size, including large media). Per-file keys (DEK) wrapped by an Argon2id-derived master key (KEK), content in chunked AEAD (XChaCha20-Poly1305 or AES-256-GCM), filenames and metadata encrypted, auto-lock via the Event Engine (DND, away, timeout, security alert), Zeroize plus memory locking. Chunked AEAD enables streaming decryption with random access: media zones (music, video, photos) and the office unit read and write directly through the vault API - plaintext never touches disk, seeking decrypts only the needed chunks. Thumbnail/cover cache is encrypted inside the vault as well (a plaintext preview cache would leak content). Post-quantum stance: the symmetric core is already PQ-safe; hybrid X25519+ML-KEM-768 (FIPS 203) for any future cross-device key exchange, ML-DSA (FIPS 204) for signatures (also covers signed updates). Drag-and-drop onto the vault zone IS the encrypt action (sealing animation). No hidden-volume deniability features (complexity not justified). Note: sensitive files should be created inside the vault from the start - secure deletion of plaintext originals on SSDs is unreliable by design (wear leveling).

## CARVILON integration (Season 7)

- Cameras (streams), door control, Edge status, time clock
- NetGuard rules for Edge and VPS; security alerts into the Event Engine

## Tools (afterwards, cut openly)

- Terminal, code editor, file explorer, FTP client
- Logic analyzer: FX2 driver (rusb), firmware upload, decoders UART/I2C/SPI (later 1-Wire, PWM), wgpu waveform rendering, hotplug -> the tab wakes up at its fixed place
- NetGuard monitor: flow map, live counters, kill switch, anomaly alerts

## Everyday (afterwards)

- Mail client (personal priority - 20 years of Thunderbird frustration): IMAP/SMTP multi-account (lettre, async-imap, mail-parser - license check before linking as always), conversation threading, instant local search via SQLite FTS5, offline cache. Safe HTML rendering in a locked-down CEF view: no JS, no remote content without click, tracking pixels blocked on the network level via CefRequestHandler/NetGuard (visible counter badge). Compose: plaintext and simple rich text first, no HTML template designer. Attachments drag-and-drop straight into the vault. New mail runs as an event through the Priority Engine (DND finally applies to email natively). Calendar invites (iCalendar) feed the calendar/day planner. Account tokens in the Rust core with Zeroize. OAuth2 for Gmail/M365: technically straightforward, but commercial distribution requires Google's verification process - plan lead time; plain IMAP providers work immediately. Scope discipline is the feature: no Exchange MAPI, no PST import, no plugin system. PGP (rPGP/Sequoia, license check) as a later stage.
- Music, messages (SimpleX client), calls/video calls, calendar + day planner, news/status
- Media engine for local entertainment: VLC-class playback (MKV/MP4, HD/4K, fullscreen) via GStreamer (gstreamer-rs; alternatives FFmpeg bindings and libVLC to be compared at season start). Hardware decode (D3D11/DXVA - patented codecs via GPU/OS decoders, we ship no software codec implementations for H.264/HEVC), subtitles incl. ASS/SSA via libass, audio track and chapter selection. Vault integration: appsrc fed by streaming decryption, seeking = chunk-level random access - media never leaves the vault as a file. Decoded frames become wgpu textures in the zone compositor, so design effects (feathering) apply to video like everything else; cinema fullscreen is a mode preset; events overlay per Priority Engine. Music path optionally pure Rust via Symphonia. Licensing: LGPL components dynamically linked only (DLLs beside the exe). Note: CEF cannot play MKV/AC3/DTS - web video stays in the surf zone, local media goes through this native stack.
- Office unit: ONE tool for text and tables instead of two programs - text documents with real table and formula blocks inline ("what a normal person needs"). PDF export first, docx/xlsx import and export later. Office feature parity is explicitly not the goal.

## Platform + foreground guard

- Foreground guard in three tiers: Tier 1 app-level (borderless fullscreen + always-on-top + watchdog re-asserting topmost on focus loss) for dev and desktop use. Tier 2 Windows kiosk grade: Shell Launcher / Assigned Access (CyberDesk replaces explorer.exe as the shell - no taskbar, no desktop underneath), autologon, keyboard filter (Win key and Alt+Tab swallowed; Ctrl+Alt+Del blockable only on IoT/Enterprise editions). Tier 3 CARVILON OS: single-client compositor (cage as ready building block, own compositor later) - nothing else exists that could cover the app. Hard limit by OS design (accepted, and good): Secure Desktop surfaces (UAC prompts, Ctrl+Alt+Del, lock screen) cannot be covered by any application. Principle: the only thing allowed to cover CyberDesk is CyberDesk (event overlays via the Priority Engine).
- Platform strategy: Windows first (running since CD-01). Linux appliance second - every component (winit, wgpu, CEF, GStreamer) is Linux-native; CD-02 OSR removes the last Windows-specific embed path (child HWND), after which we composite the texture ourselves, platform-neutral. macOS technically feasible (CEF + wgpu/Metal) but strategically deferred - bundle structure, signing, notarization, and developer-account overhead only on concrete customer demand.

## Long-term goal

- CARVILON OS: Debian 13, branded Calamares installer, CyberDesk boots as the shell
- NetGuard extended into a device firewall on the appliance (nftables rules generated from the same policy, eBPF monitoring via aya)

## External critiques (EC register)

Outside technical critiques that constrain or direct the engineering. Each entry
records the verified facts and the engineering consequence we draw from them.
Register rule (D-0044): entries never conclude "use another product" — a critique's
consequence is always something we build.

### EC-01 — Fingerprinting strategy: uniformity vs. randomization (Sam Bent / Tor Browser 14.5 OS-spoofing removal)

**Verified facts (kept, they shape CD-16/CD-25/CD-29):**

- There are two workable anti-fingerprinting strategies: **uniformity** (everyone
  reports the same value — needs a crowd all reporting it) and **randomization**
  (per-session noise — needs no crowd, breaks linkage).
- Tor Browser **14.5 removed OS spoofing**: it now reports the real OS family,
  because maintaining a coherent OS lie across every surface proved untenable —
  even the uniformity flagship gave up that vector rather than ship an
  incoherent one.
- The **anonymity-set effect is real but bounded**: a large identical crowd
  structurally helps only against a global passive adversary correlating
  fingerprints network-wide (bounded scope note: `cyberdesk-security.md`,
  internal-only).
- **Naive/incoherent spoofing backfires**: a spoofed value that contradicts
  another surface (the Brave UA/`navigator.platform` mismatch) makes the user
  *more* unique, not less. Coherence is a hard constraint — no OS/UA/platform
  spoofing, no cross-surface contradiction (binding since CD-16, D-0039).

**Engineering consequence (the corrected conclusion):** solve **every**
fingerprint vector — clamp stable signals to a common standard value, farble
measured signals with fresh coherent per-session noise (the CD-29 sweep
executes this vector-by-vector) — and treat **each solved vector as a product
and marketing win**. The earlier conclusion drawn from this critique ("for
serious anti-fingerprinting anonymity, use Tor Browser") was wrong and is
deleted (D-0044): a bounded limit against one adversary model is not a reason
to point users at anything else.

**Stance:** CyberDesk targets **parity-or-better with Tor Browser on every axis
buildable in software**. The only non-buildable axes are crowd size (mass) and
audit reputation (time) — internal scope notes, never product limitations to
advertise.

## Marketing lines — the solved fingerprint surface (CD-29, D-0045/D-0046)

Confident and accurate, one per solved vector (D-0044: never name a competitor,
never self-deprecate). For the feature list / site.

- **Canvas fingerprint re-randomized every session.** Your canvas reads clean and
  unlinkable — the same within a session, different the next.
- **GPU render fingerprint neutralized.** Sites see a generic, ordinary graphics
  identity, not the make and model of your card.
- **WebGL readback carries fresh per-session noise** — the pixels a tracker hashes
  never match from one session to the next.
- **Audio fingerprint scrambled, inaudibly.** Your sound stack reports a fresh,
  unlinkable signature every session; playback is untouched.
- **Your installed fonts are invisible to websites.** Every window returns the same
  standard font set, whatever you have installed.
- **Layout and text measurements jittered per session** — the pixel-perfect metrics
  used to profile you don't line up across sessions.
- **High-resolution timers blunted against hardware fingerprinting** — CPU-speed and
  micro-benchmark tricks can't measure your machine finely.
- **Codec and media support normalized** to a common profile — your exact device
  media fingerprint stays private; your installed voices are hidden.
- **Math fingerprint erased.** The tiny per-CPU rounding differences trackers exploit
  are rounded away to one common answer.
- **Your screen reads as an ordinary display** — a common resolution, consistent with
  your real window, never a contradiction that flags you.
- **CPU cores, memory, touch, battery and network reported in common buckets** — the
  exact spec of your machine stays yours.
- **New identity, on demand.** One click re-rolls a window's entire fingerprint and
  reloads it fresh — plus a fresh identity every launch, and an automatic rotation you
  can watch count down on the grid.
