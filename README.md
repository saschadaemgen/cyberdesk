# CARVILON CyberDesk

CyberDesk is the desktop frontend of the CARVILON platform — a single
fullscreen application in the style of a serious "cyber operating system". A
memory-safe Rust host renders the shell (fixed zone layout, one color world,
heavily animated) and embeds web content through the Chromium Embedded
Framework (CEF). On that foundation it is a privacy-first browsing
environment: per-window Tor with onion support, always-on coherent
fingerprinting hardening graded by an Ampel (traffic-light) control, an
engine proven silent by net-log measurement, and browsing content that lives
in RAM only — no disk trace.

> Proprietary — Copyright (c) 2026 Sascha Daemgen IT and More Systems.
> All rights reserved. See `LICENSE`.

---

## State after CD-35

* **Shell:** Borderless fullscreen on the primary monitor, dark background
  (`#04070A`), vsync. The shell background is the Pulse Grid alone — the CARVILON
  ring was removed from the shell in CD-06 (its motif migrates to the Season-2
  start animation / Energy Core, D-0013). `ESC` walks a small chain — cancel a
  favorite drag, else hide the command set, else close settings, else quit (CD-12)
  — and otherwise quits cleanly from anywhere. Dev mode via `--windowed` (1600×900).
* **Pulse Grid background:** a seeded circuit board that lives behind the shell,
  built as **three depth layers** (far → mid → near) baked into one
  full-resolution HDR texture — a crisp bright front, a dimmer middle, and a
  faint fine recede (~10× the earlier content: ~12k primitives at ultrawide,
  baked in a few milliseconds). Each layer weaves a micro lattice, routed traces
  with pads and solder dots, **chip footprints** (outline + pin-pad rows), **via
  clusters** and **junction hubs**; the near layer carries the two full-width
  bus lines. Bright pulses with fading trails travel the traces on every depth
  (near bright/fast, far sparse/slow/faint — depth in motion), with occasional
  expanding node flares on the near layer. The board is composited each frame
  scaled by a live **glow-intensity** control and is identical across launches
  (per-layer seeds derive from one master seed — full determinism). It is
  allowed to glow — a **zone shadow** dims it under the surf zone and any open
  overlay (calm under content, glow in the margins) so the page stays readable.
  The earlier **Deep Field** (a breathing glow with drifting nebulae, dust, and
  a scan sweep) is preserved as a token-selectable "Calm" variant
  (`background.kind = "deep_field"`). See `docs/cyberdesk-decisions.md` (D-0012,
  D-0013).
* **Per-window Tor (CD-15, D-0026/D-0027):** clearnet is the default; each column has
  its own **Tor toggle** — a shield glyph in its floating command set. Flipping it
  routes **that window** through the Tor network (embedded **arti**, pure-Rust Tor, on
  a background runtime) via its own local SOCKS circuit — the glyph lights and pulses
  while the engine bootstraps, and other windows are unaffected (per-`CefRequestContext`
  proxy, so no "proxy changes all windows" bug). The browser opens **immediately**
  under its proxied context (the proxy is applied to the context before any request,
  so it never falls back to a direct connection); it just can't fetch a real page
  until bootstrap is **ready**. Bootstrap has a **timeout** (default 90 s, override
  with `CYBERDESK_TOR_BOOTSTRAP_SECS`): if the network blocks Tor the status becomes
  **failed** with a reason — never an endless "connecting", and the glyph turns to a
  warn state so a lit shield never implies protection that isn't there. Each Tor
  window is on its **own circuit** (two Tor windows are unlinkable), a leak checklist
  is enforced per context (SOCKS proxy, WebRTC `ip_handling_policy` constrained, QUIC
  off, remote DNS), and the wiring is **fail-closed** (a slot never silently falls
  back to a direct connection — verified by an adversarial security review that caught
  three real leaks). Settings expose the engine switch, a "route new windows through
  Tor by default" toggle, and a live status readout (with the failure reason).
  **Scope:** Tor mode is CyberDesk's IP-anonymity layer — sites see a Tor exit
  address, never the user's IP, on an isolated circuit per window; fingerprinting
  hardening is a separate, always-on layer (see below) and the two compose. *This is the
  host's second sanctioned outbound path (D-0004 → D-0027); the live routing/leak checks
  run on the maintainer's networked machine.*
* **Open `.onion` addresses (CD-35, D-0052):** Tor windows open **onion services** —
  `.onion` addresses are resolved **inside Tor, never through clearnet DNS** (the
  hidden-service rendezvous has no DNS query and no exit node, so onion browsing has
  no exit-node exposure), on the same per-window isolated circuits. A `.onion` in a
  **clearnet** window is **refused before any lookup** — an honest full-window notice
  offers to open it in a new Tor window or switch that window to Tor — and a
  fail-closed request guard keeps `.onion` subresources and redirects away from
  clearnet DNS too. While a Tor window is on an onion service, the HUD route reads
  **Tor · Onion**. Onion browsing is in-memory like all browsing content — no disk
  trace. (Onion-Location auto-switch, client auth, and `.onion` TLS specifics are
  later phases.)
* **Fingerprinting hardening — coherent tracking-resistance (CD-16, D-0039):**
  always on, in **every** window (clearnet and Tor alike). A fresh random seed per
  launch keys **deterministic per-session farbling** of the readback surfaces sites
  fingerprint — canvas, WebGL readback, audio, client rects, text metrics — injected at
  document-start so it runs before any page script. The noise is invisible and **stable
  within a session** (nothing breaks or flickers) but **changes on the next launch**, so
  a site cannot **link** one session to the next. High-entropy stable attributes are
  bucketed (CPU cores, device memory), the local-font enumeration API is neutralized,
  the WebGL vendor/renderer strings are standardized to a common Windows-coherent GPU,
  and the timezone is normalized to UTC. Crucially it does **no OS/UA/platform
  spoofing** — those stay real and mutually consistent, so nothing contradicts itself
  (the mismatch trap that makes half-spoofed browsers *more* unique). **Scope:**
  coherent **tracking-resistance** — it breaks cross-site and cross-session
  linkability while keeping every exposed value mutually consistent; IP anonymity
  is the built-in per-window Tor layer above, and the two compose.
* **Hardening controls — global + per-window, safety-gated (CD-25, D-0040; CD-30,
  D-0047):** the hardening is visible and configurable. Settings has a **global preset**
  (the **Ampel** levels below — **Off** / **Green** / **Yellow** / **Red**, default
  **Green**) that applies to every window, plus a **Custom** detail view for per-vector
  control. Each window can **override** its own level from a floating control beside its
  Tor/close icons — inherit the global, or pick its own — with the level,
  inherited-vs-override, and any **reduced** state shown honestly (a reduced window reads
  as a warning). The **preset path is the default** (a unique per-vector toggle
  combination is itself a fingerprint, so the safe coherent presets stay primary). Any
  action that **weakens** protection — Off, a step down the Ampel, dropping a vector,
  entering custom — shows an honest trackability warning and needs **two confirmations**
  before it applies; **strengthening is instant and ungated**. The gate informs and
  confirms but never forbids: it is also the developer escape hatch (fully disable any one
  protection to debug). Every label stays **tracking-resistance, never anonymity**, and no
  control claims to hide the OS/UA/platform.
* **The Ampel — one graded protection control (CD-30, D-0047):** the traffic light is the
  primary way protection is set and read. **Green** — the everyday default — runs the
  coherent core: every vector except the clock, media/codec and math clamps, so it costs
  you no layout and no timing. **Yellow** adds those three. **Red** is the maximum: every
  vector at its tight buckets, plus the window lock below. **Off** stays behind the gate.
  The Ampel lives in the permanent HUD and in settings, and each window's own icon is a
  **mini traffic light** for that window's effective level. Green < Yellow < Red is a
  strict order: stepping **up** applies instantly, every step **down** goes through the
  two-confirmation gate. The earlier preset names survive as aliases of identical content
  (a persisted *Standard* reads as Yellow, *Strict* as Red), so an upgrade never silently
  changes a choice you made.
* **Red locks the window (CD-30, D-0047; CD-31, D-0048):** at Red a window's viewport
  snaps to a standard size — 1920×1080, laddered down (1600×900, 1280×720) to the largest
  the frame holds — vertically centered and **locked** while Red is live; unlocked
  neighbors absorb the space. Reported screen and viewport then agree exactly, so the
  window reads like a fullscreen browser on an ordinary machine. Everyday sizing outside
  Red stays completely free, and your column layout returns by construction the moment you
  step down. Red is always a **deliberate in-session choice**: a saved Red boots into
  Yellow — full protection, freely resizable — never into a locked window you didn't ask
  for. If not even the smallest standard size fits the display, the level and its vectors
  are unaffected and the window honestly stays zone-sized.
* **The HUD — status that is always true (CD-30, D-0047):** a permanent transparent
  readout floats top-right: a digital clock in your local time, the global Ampel, the
  active window's route (**Clearnet** / **Tor**), the honest count of **vectors active**,
  and the identity countdown / age. Every field is painted from the host's resolved
  config — the same one the render processes receive — so the HUD reports the protection
  that is actually running, never a state that isn't.
* **The complete fingerprint surface — every vector solved and settable (CD-29,
  D-0045; CD-32, D-0049):** the surface is now exhaustive. Eleven independent vectors —
  canvas, WebGL readback, **GPU identity**, audio, layout & text metrics, **device
  profile** (CPU, memory, touch, battery, network), **fonts**, **clock precision**,
  **media & codecs**, **math rounding**, **window size** — each its **own visible
  toggle**, settable **globally and per-window**. Stable signals are **clamped to common
  values** (your installed fonts are hidden behind a standard set so every window returns
  the same font answer; the GPU reads as a generic card; codecs, math and screen size
  read as an ordinary machine) and measured signals carry **fresh per-session noise**
  (canvas, WebGL, audio, text metrics, high-resolution timers). **Screen size** reports a
  **common real resolution** (default 1920×1080; 1600×900 / 1280×720 presets) that always
  stays consistent with the real window — never a decoy. **Window size** reports the
  nearest common step to your real column, as one coherent cluster —
  `innerWidth`/`innerHeight`, the root client box, `visualViewport` and `matchMedia` all
  agree, so no internal contradiction gives it away. Your layout is never touched for it:
  the window itself only ever snaps at **Red** (D-0049), where the reported size becomes
  the real one. Presets stay the coherent primary path; the
  safety gate still fires on any weakening.
* **New identity — on demand, on a timer, or every launch (CD-29, D-0046):** a rotation
  re-seeds the whole farble basis, producing a fresh, unlinkable fingerprint. **New
  identity now** (in each window's tracking-resistance menu) re-rolls that window — plus
  its Tor circuit if you like — and reloads it fresh; **automatic rotation** re-rolls on
  a timer you set, with the **Pulse Grid visibly counting down and re-rolling**; and a
  **fresh identity each launch** is the default. Honest by design: the manual button and
  on-restart are the immediate cross-session-linkage breakers, and the automatic
  countdown re-seeds what you open next while giving the shell its showpiece.
* **De-Googled — proven silent by measurement (CD-17, D-0041; CD-26, D-0042):** the
  Chromium engine's every phone-home vector — Safe Browsing, component updater,
  variations/Finch, connectivity probes, prediction, search suggest, domain
  reliability/NEL, translate, spell check, autofill/leak-check, link-doctor,
  optimization hints, GCM/push, plus the deep idle vectors CD-26 closed (GAIA
  ListAccounts pinned to a dead loopback origin, AI-mode eligibility off, the
  Reporting/NEL store unloaded) — is disabled at the switch/preference level,
  applied to clearnet and Tor windows alike. The claim is bounded and **measured,
  not asserted**: a net-log capture on idle shows **zero** unsolicited
  Google/telemetry connections (the recipe is `docs/cyberdesk-degoogle-audit.md`).
  Secure DNS (DoH) is pinned off — clearnet resolves via the OS deterministically,
  Tor windows resolve remotely through the tunnel.
* **Browsing lives in RAM — and the disk is swept every launch (CD-33, D-0050;
  CD-34, D-0051):** browsing content — history rows, cookies, cache — is never
  written to disk: every window, clearnet and Tor, runs on in-memory request
  contexts, and the in-app history/suggestion store is a RAM-only table that ends
  with the session. A **standing on-launch purge** (default on, settable, with a
  live on-disk footprint + last-purge readout in settings) additionally wipes the
  CEF scaffolding directory before the engine starts — clearing legacy residue and
  acting as a regression backstop, allowlisted to exactly that one directory (Tor
  state, favorites, and settings live in a disjoint tree and are never touched).
* **Own start page, no Google (CD-14, D-0025):** every empty/new
  slot opens to an **own start page** served from the binary at `cyberdesk://start/`
  (same isolation as settings, **zero network**) — Google is gone. It is a black
  canvas with a faint micro-lattice, the glowing **Energy Core** (the reserved CD-06
  motif — a bright hollow core inside concentric rotating brand-cyan arcs, motion
  respecting `prefers-reduced-motion`), a search/address capsule (same host-side
  URL-vs-search classifier + chosen engine), and round favorite tiles that open in
  the slot. History and favorites are local SQLite; history is **RAM-only** since
  CD-33 (suggestions work all session, nothing lands on disk).
* **Session lifecycle — fresh by default, restore by choice (CD-21, D-0035):** a
  normal quit saves nothing; the next launch is the default layout — a clearnet
  window plus a Tor window (when the engine is enabled and the display fits two).
  **Quit & Save** (in the MF zone, beside plain Quit) persists the layout: slot
  order, widths, each window's clearnet/Tor mode, and — for clearnet windows
  only — the URL. A Tor window's URL is **never** written to disk; it returns as
  a real Tor window on the start page. No cookies, cache, or content are saved
  either way, so a restored session comes back logged out — restore brings back
  your tabs, not your sessions.
* **Update awareness — the info area (CD-13, D-0023/D-0024; client-side since
  CD-22, D-0036):** a small status light top-right, beside the gear. Click it for a
  **floating panel** listing the shipped components — CyberDesk itself, its CEF
  core, and the embedded Tor (arti) engine — each with its exact pinned version and
  an honest status (current / **held back** with the reason, e.g. the arti 0.44
  bootstrap regression, D-0034). Since CD-22 this surface is **entirely
  client-side**: the earlier live manifest fetch was retired, so the host opens
  **no HTTP client of its own** — its only sanctioned outbound path is per-window
  Tor browsing (D-0027/D-0052). The self-update feed returns with the signed
  (ML-DSA) update pipeline, and this panel is where its Install action will live.
  It is the seed of the later notification rail.
* **Floating command elements — the bar dies (CD-12, D-0021):** the single top bar
  is retired. **Every column carries its own floating command set** — back/forward/
  reload orbs and an address capsule — that reveals above *that* column and drives
  it; move the mouse into the gap above a column, or press `Ctrl+L` for the active
  one. They float on a **transparent band** over the Pulse Grid (only the pills
  paint; the background breathes between them) and glide as columns reflow. Favorites
  become **round tiles** in one shared launcher row. **Drag a favorite tile into a
  control gutter and it opens there as a new column** — the shell draws a ghost on
  the cursor and lights the gutters as drop zones, dropping into the nearest (at full
  capacity it navigates the column under the ghost instead; `ESC` cancels). The
  CD-12 shell-drawn corner close orb was consolidated in CD-18 into the explicit
  per-window **close icon** beside each address capsule (see per-window icons
  below).
* **The main frame — asymmetric zones + reflow (CD-11 D-0020, revised D-0022):** the
  slot group does not own the full width; a zone flanks it on each side. The **right**
  is the permanent **Multifunctional (MF) zone** (status / files / FTP / music tabs
  in later seasons) — always 320 px, at every resolution, marked now by a three-bar
  rows glyph. The **left** (future Spine, a diamond glyph) is the flexible one: full
  width when the slots leave room for it alongside the MF zone, else it **retreats,
  animated, into a thin rail** — one fluid ~220 ms motion, driven by a single
  per-frame layout that both rendering and input read (so it never desyncs or jumps).
  The whole frame is centered as a block, so with the left railed the group shifts
  toward it. Gutters are a generous 56 px of **control territory** (CD-12 drop zones),
  with the Pulse Grid glowing in it; the frame is entirely automatic
  (`slots::frame_layout` decides). The slot maximum is **three**: the floor is one
  slot + the MF zone + a left rail at 1920, and the 5120 ultrawide shows three slots
  with both zones full.
* **Slot engine — fixed-width content columns (CD-09, D-0017; cap revised D-0022):**
  the surf zone is up to **three fixed-width columns** ("slots", 1200 logical px each)
  side by side, gutter-spaced and centered between the zones. `Ctrl+T` adds a column
  that opens to the start page (a placeholder with its index glyph covers the brief
  spawn until it paints), `Ctrl+W` closes the active one, `Ctrl+1..3` / `Ctrl+Tab` switch. One
  column is **active** at a time (a thin brand accent underlines it): the keyboard and
  its floating command set drive it; the mouse drives whichever column it is over (a
  click makes that column active). The Pulse Grid glows in the gutters and margins,
  dimmed under each column by the zone shadow. On the ultrawide, three different sites
  sit pixel-aligned side by side.
* **A fluid workspace (CD-10, D-0018/D-0019; websites no longer restored, D-0025):**
  columns can be **reordered** (`Ctrl+Shift+←/→` swap with a neighbor) and **widened**
  to double width (`Ctrl+Shift+D` — a two-column-wide slot for a web-app, the group
  staying centered and pixel-aligned; no-op if it won't fit). A **real click on a
  `target=_blank` link, or a `Ctrl`/middle-click on any link, opens the target in a
  new column beside the source** (which becomes active), or in place when the grid is
  already full; ad/script popups stay suppressed. Websites are not auto-persisted
  across restarts: a plain quit starts the next launch fresh, and only the explicit
  **Quit & Save** restores the layout (clearnet URLs only — CD-21, D-0035; the
  earlier always-on session restore was reversed for privacy in CD-14, D-0025).
* **Surf columns (CEF, off-screen rendering):** each column's CEF browser renders
  off-screen (`on_paint`); CyberDesk uploads each frame into that slot's wgpu
  texture and composites it at the slot's rectangle (70 % tall, centered). Column
  edges are **feathered** (a light, steep SDF-based alpha soften over a narrow band
  — the outermost pixels only, corrected in CD-06 from the wider CD-05 band that
  read as a vignette; toggleable back to the hard rounded corner). Mouse and
  keyboard are forwarded into the column under the cursor / the active column (a
  Google search, clicking, and scrolling all work) and the cursor follows it. At
  the loaded columns the ultrawide holds (the D-0009 gate was measured at four in
  CD-09; the cap is now three, D-0022) the render loop stays well inside the 60 fps
  budget; the accelerated zero-copy OSR path (D-0009) is recommended but not yet
  needed (see `docs/cyberdesk-decisions.md`, D-0017).
* **Free surfing (floating command sets + memory):** the command surface is a set of
  **floating ensembles** — one per column (CD-12, D-0021), evolved from the CD-08
  hover-reveal bar. Each reveals above its column on hover-into-the-gap or `Ctrl+L`
  and drives it (prefill, star and scheme hint reflect that column). An ensemble
  holds the address input (classified host-side as a URL or a search on the chosen
  engine); the shared launcher shows your **favorites as round tiles**. Start typing
  and up to six live **suggestions** appear from favorites + history — favorites
  first, then history by a simple frecency. `Arrow` keys move the selection, `Enter`
  navigates the selected entry (or the raw text), a click navigates; a favorite tile
  clicks to navigate the engaged column or **drags into a gutter to open a new one**.
  **`Ctrl+D`** favorites the current page and the star reflects and toggles it live.
  An ensemble retreats when the mouse leaves it (after a short grace period, never
  while you are typing), when a navigation commits, or on `ESC`. Back / forward /
  reload and the mouse's forward/back buttons drive the page history, an amber glyph
  flags a plain-`http://` page, and a loading line traces the top of each column.
  Popups follow a gesture-aware
  policy (D-0011): a real click on a `target=_blank` link navigates its own column
  in place, script `window.open` is dropped — no second window ever opens.
* **Settings:** a gear button (top-right) opens an in-shell settings card — a
  **second, web-isolated OSR view** locked to an internal `cyberdesk://` custom
  scheme (D-0010), served entirely in-process from embedded assets. It can never
  reach the web (its navigation is confined to `cyberdesk://`). It now carries the
  full control surface, wired over the message-router IPC bridge, applied live and
  persisted to SQLite: a **search-engine** select (DuckDuckGo — the factory
  default, D-0043 — / Brave / Startpage / Bing / Google; the host routes every
  address-bar query to the SELECTED engine), the **glow-intensity** slider and
  appearance toggles (animated background, feathered edges, stay-in-foreground),
  the **Tor section** (engine switch, Tor-for-new-windows default, live status
  with the failure reason, new circuit / new identity, the pinned arti version),
  the **protection section** (the global Ampel preset, the per-vector Custom
  detail, the reported-screen preset), **identity rotation** (fresh identity each
  launch, the automatic rotation timer), and **on-disk privacy** (the on-launch
  residue purge with its live footprint + last-purge readout, CD-34).
* **One token source:** every style value — colors, radii, periods, amplitudes —
  comes from an embedded theme (`src/theme.toml`), resolved both into wgpu shader
  uniforms and into the settings/command pages' CSS custom properties. App state
  lives in a schema-versioned SQLite store under `%LOCALAPPDATA%\CyberDesk\` —
  settings, local history and favorites (D-0014); open websites are **not** saved
  (CD-14, D-0025). All local, no sync.
* **Diagnostics log:** a windowed release build has no console, so a rolling-daily
  log is written to `%LOCALAPPDATA%\CyberDesk\logs\` as **`cyberdesk.log.<date>`**
  (e.g. `cyberdesk.log.2026-07-10` — the bare `cyberdesk.log` never exists on disk;
  the app resolves and reports the newest dated file). It captures the app lifecycle
  and the full Tor bootstrap (including arti's internal progress). Set `RUST_LOG` for
  more detail. Never contains secrets. **You rarely need the file:** the MF-zone
  viewer's **Tor** and **Log** tabs (below) stream the same records live in the UI.
  For a stalled Tor bootstrap, set **`CYBERDESK_TOR_TRACE=1`** to raise arti's own
  crates to **trace** (incl. the directory-fetch layer `tor_dirclient`) — the log
  then shows the exact phase-by-phase progress (`<pct>%: connecting; fetching a
  consensus …`), the guard pick, the TLS handshake, the circuit build, the consensus
  request lifecycle, and any blockage reason, so a stall is never silent. (`=1` is
  trace because the dir-fetch detail lives there; pass `=debug` for less verbosity.)
* **The Multifunctional (MF) zone viewer (CD-18):** the permanent right zone holds a
  tabbed live viewer — **Tor** (a Connecting/Ready/Failed+reason status header over
  the streaming Tor/arti log, so bootstrap progress is visible live and a blocked
  network shows *Failed with a reason*, no file hunting), **Log** (a live tail of the
  full app log with an info/debug filter, a Copy button, and auto-scroll that pauses
  when you scroll up), and **Terminal** (reserved — a real terminal is a later
  release). Tabs switch on click. It is an internal `cyberdesk://mfzone/` page, the
  same scheme-locked, isolated, no-network surface as settings/info/start.
* **Per-window icons (CD-18):** each window's floating command set carries two
  explicit, always-present icons beside its address bar — an **anonymity/Tor icon**
  that shows the window's status (clearnet / connecting / ready / failed) and toggles
  Tor for that window, and a **close icon** that closes that window (the last window
  refuses). These consolidate the scattered CD-15 Tor glyph and the CD-12
  corner-hover close into two clear controls. **CD-25** adds a third: a **tracking-
  resistance icon** that opens this window's protection chooser and shows its effective
  level — **CD-30** makes it a **mini Ampel** (the lamp for the live level lights green,
  yellow or red; Off leaves them dark; a reduced or off window is warn-tinted, with an
  override marker when it differs from the global).
  **CD-29** grows that menu into the window's full identity control: **New identity
  now**, a per-vector **Custom** detail, and a **screen-size** cycler — all per-window.

The accelerated (zero-copy GPU) OSR path was researched; CyberDesk stays on the
CPU path for now — see `docs/cyberdesk-decisions.md` (D-0009).

Target platform: **Windows 11 (x64, MSVC)**. Other platforms are deliberately
out of scope for this ticket.

---

## Prerequisites

| Tool | Purpose | Note |
| --- | --- | --- |
| Rust (stable, `x86_64-pc-windows-msvc`) | build | via `rustup` |
| Visual Studio 2022 — "Desktop development with C++" | MSVC linker + Windows SDK | for Rust-MSVC and the CEF wrapper |
| CMake ≥ 3.29 | builds `libcef_dll_wrapper` | must be on `PATH` |
| Ninja ≥ 1.12 | CMake generator for the wrapper | must be on `PATH` |
| Python 3 | CEF/Chromium build helper | must be on `PATH` |
| PowerShell 5.1+ | `scripts/fetch-cef.ps1` | ships with Windows |

Quick check that everything is present:

```pwsh
rustc --version; cargo --version; cmake --version; ninja --version; python --version
```

---

## 1. Fetch the CEF binaries (once / on version change)

The CEF binaries are several hundred MB and are **never** committed. The
following script downloads the exact pinned CEF version (see
`docs/cyberdesk-decisions.md`, D-0002) from the official CDN into `vendor/cef/`
and lays it out so the build uses it directly:

```pwsh
# from the repository root
./scripts/fetch-cef.ps1
```

The script verifies the download's SHA-1 and is idempotent (a second run
without `-Force` detects an existing installation). `vendor/cef/` is listed in
`.gitignore`.

---

## 2. Build & run

CyberDesk locates the CEF installation via the `CEF_PATH` environment variable,
which is already set to `vendor/cef/` in `.cargo/config.toml` — no manual
configuration needed.

```pwsh
# fullscreen (acceptance mode)
cargo run --release

# windowed 1600x900 (dev mode)
cargo run --release -- --windowed
```

`CYBERDESK_WINDOW_SIZE=WxH` overrides the dev-window size to exercise multi-column
layouts on a non-ultrawide — with the D-0022 zones, a second column needs roughly
`3000x900`, and three need the 5120 ultrawide.

* Move the mouse into the gap above a column (or press **`Ctrl+L`**) to reveal that
  column's floating command set; **`ESC`** walks the chain — cancel a favorite drag,
  else hide the command set, else close settings, else quit. See **Controls** below
  for the full map.
* The **gear** button (top-right) opens the settings card; the search-engine
  select, the slider, and the toggles apply live and persist across restarts.
* The first build is slow because CMake+Ninja compile `libcef_dll_wrapper`. The
  CEF runtime files (`libcef.dll`, resources, `locales/`) are copied next to the
  `.exe` in `target/<profile>/` automatically.

### Optional: headless render self-test

Renders a single shell-background frame (the Pulse Grid) off-screen to a PNG
file (useful for CI / visual regression; does not touch any desktop):

```pwsh
cargo run --release -- --capture background.png
```

`CYBERDESK_CAPTURE_SIZE=WxH` sizes it (e.g. `5120x1440` for the ultrawide
judgment), `CYBERDESK_CAPTURE_GLOW=<mult>` brightens it (e.g. to inspect the faint
far layer), and `CYBERDESK_CAPTURE_SLOTS=N` renders N placeholder slot columns
(CD-09) so the multi-column layout — columns, gutters, glowing margins, zone
shadow, index glyphs — can be eyeballed headlessly (e.g. `=3` on the ultrawide).
`CYBERDESK_CAPTURE_UNITS=2,1,…` overrides it with an explicit per-column
width-unit sequence (CD-10 double slots), and `CYBERDESK_CAPTURE_PENDING=N` marks
the first N columns as restored-pending (the scheme-colored placeholder dot).
`CYBERDESK_CAPTURE_DRAG=1` overlays a **favorite-drag** sample (gutter drop zones +
ghost) so the CD-12 overlay can be eyeballed headlessly. `CYBERDESK_CAPTURE_INFO=idle|active`
draws the CD-13 **info glyph** (idle ring / filled disc + pulse + count). (The
`CYBERDESK_CAPTURE_CLOSE` knob was retired with the shell-drawn close orb in CD-18 —
closing is now an in-page icon.)

---

## Controls

The shell shows **fixed-width content columns** ("slots", up to three — D-0022)
flanked by two zones: a **permanent Multifunctional zone** on the right and a
**flexible Spine zone** on the left that retreats to a thin rail when the slots need
the width (CD-11, revised D-0022). The frame reflows automatically; there are no
controls for it. Each column is a stripped-down browser with no visible chrome until
you summon it. Keyboard shortcuts act on the **active slot** (a thin brand accent
underlines it); mouse actions act on the slot under the cursor. Each column carries
its own floating command set (CD-12).

| Input | Action |
| --- | --- |
| `Ctrl+T` | Add a column to the right (up to what fits the width); it becomes active and opens to the start page — search or type an address there |
| `Ctrl+W` | Close the active column (the last one can't be closed); the rest recenter and a neighbor becomes active |
| Click a column's **close icon** (beside its address capsule, CD-18) | Close that column (the last one can't be closed) |
| `Ctrl+1` … `Ctrl+3` | Focus the 1st … 3rd column |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Cycle the active column forward / backward |
| `Ctrl+Shift+←` / `Ctrl+Shift+→` | Swap the active column with its left / right neighbor |
| `Ctrl+Shift+D` | Toggle the active column between single and double width (no-op if a double won't fit) |
| Click a column | Make it the active column |
| Click a `target=_blank` link, or `Ctrl+click` / middle-click a link | Open it in a new column beside the source (or in place if the grid is full) |
| Mouse into a column's top gap | Reveal that column's command set (fades in); it retreats when the mouse leaves it, after a short grace period |
| `Ctrl+L` | Reveal the active column's command set with the input focused + selected |
| type (in the capsule) | The launcher gives way to live suggestions from favorites + history; moving the mouse away no longer hides the set while you type |
| `↑` / `↓` | Move the suggestion selection |
| click a favorite tile | Navigate the engaged column to that favorite (the set retreats) |
| drag a favorite tile into a gutter | Open that favorite as a new column there (or navigate the column under the ghost when the grid is full); `ESC` cancels |
| `Enter` (in the capsule) | Navigate the ensemble's column to the selected suggestion, or the typed text — a scheme, a dotted host, or `localhost` loads as a URL (default `https://`); anything else searches the chosen engine |
| `Ctrl+D` | Favorite / unfavorite the active column's page (star reflects it live) |
| `Alt+←` / `Alt+→` | History back / forward (active column) |
| Mouse button 4 / 5 | History back / forward (column under the cursor) |
| `F5` / `Ctrl+R` | Reload (active column) |
| `Ctrl+Shift+R` | Hard reload, ignore cache (active column) |
| Click the **info glyph** (top-right, beside the gear) | Open / close the update-awareness panel; it fills + pulses when an update is available |
| Click the **anonymity/Tor icon** (beside a column's address capsule, CD-18) | Route that window through Tor (or back to clearnet); it shows the live state and pulses while the engine connects |
| Click the **mini-Ampel icon** (beside a column's address capsule, CD-25/30) | Open that window's protection chooser: its Ampel level, New identity now, per-vector Custom, screen-size cycler |
| Click the **HUD Ampel** (top-right strip, CD-30) | Change the global protection level; stepping down routes through the two-confirmation gate |
| `ESC` | Cancel a favorite drag, else hide the command set / info panel, else close the settings card, else quit |

An amber glyph in the address capsule marks a page served over plain `http://`
(e.g. `neverssl.com`, which stays http by design); `https` and internal pages
show no warning. The **gear** (top-right) opens the settings card (see the
Settings bullet above for the full surface: search engine, appearance, Tor,
protection/Ampel, identity rotation, on-disk privacy) — every control applies
live and persists across restarts. **Stay in foreground** is always off in
`--windowed` dev mode.

---

## Project layout

```
cyberdesk/
├─ src/
│  ├─ main.rs        # entry point, CLI, process model
│  ├─ app.rs         # winit event loop, window, slot layout, per-slot input routing, nav keys, foreground guard, HUD/frame push
│  ├─ renderer.rs    # wgpu renderer: shell + per-slot page/placeholder/line compositing, capture
│  ├─ browser.rs     # CEF OSR (slot/internal/MF-zone/HUD views), custom scheme, isolation, IPC allowlist, onion guard (CD-35)
│  ├─ slots.rs       # slot layout engine (frame_layout/frame_capacity, asymmetric zones, Red lock) — pure + unit-tested
│  ├─ theme.rs       # theme tokens -> shader uniforms + internal pages' CSS vars
│  ├─ theme.toml     # the embedded "cyber" token set (single style source; [slots] section)
│  ├─ store.rs       # schema-versioned SQLite store (settings, favorites; RAM-only history since CD-33)
│  ├─ settings.rs    # live settings state (search engine, Tor, hardening presets, rotation, purge) over the shared store
│  ├─ memory.rs      # history + favorites domain layer (frecency suggestions) over the store
│  ├─ harden.rs      # fingerprinting-hardening config model: Ampel levels, per-vector flags, weakening gate (CD-25/29/30)
│  ├─ hardening.js   # document-start injection: coherent farbling + clamps, config-gated per vector (CD-16/29)
│  ├─ degoogle.rs    # engine phone-home kill switches/prefs, per-context (CD-17/26)
│  ├─ forensic.rs    # anti-forensic on-launch residue purge, allowlisted + footprint readout (CD-34)
│  ├─ fsprobe.rs     # filesystem measurement probe used to verify ephemerality (CD-33)
│  ├─ logging.rs     # rolling file log + in-memory ring buffer for the MF-zone viewer (CD-15/18)
│  ├─ updates.rs     # component update status, client-side vs pinned versions (CD-13; client-only since CD-22)
│  ├─ tor.rs         # embedded Tor engine (CD-15/35): arti-client + per-slot SOCKS5 relay, isolated circuits, onion-capable
│  ├─ pulsegrid.rs   # Pulse Grid background: seeded generator + life simulation
│  ├─ settings.html/.css/.js   # embedded internal settings page assets
│  ├─ command.html/.css/.js    # embedded floating command-set page assets (CD-12)
│  ├─ info.html/.css/.js       # embedded component-status info panel assets (CD-13/22)
│  ├─ start.html/.css/.js      # embedded own start page: Energy Core + search + favorites (CD-14)
│  ├─ mfzone.html/.css/.js     # embedded MF-zone viewer: Tor/Log/Terminal tabs (CD-18)
│  ├─ hud.html/.css/.js        # embedded HUD strip: clock, Ampel, route, vectors, identity (CD-30)
│  ├─ onion.html/.css/.js      # embedded onion refusal page for clearnet windows (CD-35)
│  ├─ ring.wgsl      # CARVILON ring — dormant since CD-06 (Season-2 motif)
│  ├─ pulsegrid_*.wgsl  # Pulse Grid: lattice (3 depth weaves) · sprite (SDF prims/pulses) · composite
│  ├─ deepfield.wgsl # Deep Field ("Calm" variant) background  ·  blit.wgsl (upscale)
│  ├─ page.wgsl      # per-slot page / overlay compositing (feathering)
│  ├─ slot_placeholder.wgsl  # lazy-slot placeholder (fill + 7-segment index glyph)
│  ├─ slot_lines.wgsl        # per-slot loading line (top) + active accent (bottom)
│  ├─ drag.wgsl      # topmost command overlay: favorite-drag ghost + drop zones (CD-12)
│  ├─ info_glyph.wgsl # info glyph (idle ring / active disc + pulse + count, CD-13)
│  └─ gear.wgsl      # settings gear button
├─ scripts/
│  ├─ fetch-cef.ps1  # downloads the pinned CEF version into vendor/cef/
│  └─ harden-selftest.mjs  # headless Node-vm checks of the hardening JS (CD-29)
├─ docs/                          # living project documents (English; owned by CC, D-0053)
│  ├─ cyberdesk-architecture.md
│  ├─ cyberdesk-decisions.md      # D-0001 … (newest on top, append-only)
│  ├─ cyberdesk-security.md
│  ├─ cyberdesk-wire-format.md    # the full IPC allowlist schema
│  ├─ cyberdesk-feature-backlog.md  # incl. principles + season orientation (CD-36)
│  └─ cyberdesk-degoogle-audit.md   # net-log verification recipe (CD-17/26)
├─ .cargo/config.toml
└─ vendor/cef/        # (git-ignored) CEF binaries
```

---

## Troubleshooting

* **`CMake`/`Ninja` not found:** install the "C++ CMake tools for Windows"
  component in VS 2022, or install CMake/Ninja separately and put them on
  `PATH`.
* **Link error against `libcef`:** `vendor/cef/` is missing or incomplete — run
  `./scripts/fetch-cef.ps1 -Force` again.
* **Black instead of the circuit background:** check the graphics driver; wgpu
  needs a working D3D12 or Vulkan backend adapter.
* **`GPU process exited unexpectedly` on stderr:** this was a CD-01 child-window
  issue; under CD-02's off-screen rendering the GPU process is healthy and the
  message no longer appears (see `docs/cyberdesk-decisions.md`, D-0009). If it
  does show, CEF falls back to SwiftShader and the page still renders.
* **CEF profile/cache:** kept isolated under `cyberdesk-cache/` beside the exe
  (`target/<profile>/` in a dev build, git-ignored) — the surf zone deliberately
  shares no state with a separately installed Chrome. Since CD-33/34 it holds
  **no browsing content** (browsing runs on in-memory contexts) and the whole
  directory is purged on every launch by default; what exists mid-session is
  regenerable Chromium scaffolding, measured and reported in settings.
