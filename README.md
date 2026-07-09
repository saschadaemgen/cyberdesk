# CARVILON CyberDesk

CyberDesk is the desktop frontend of the CARVILON platform — a single
fullscreen application in the style of a serious "cyber operating system". A
memory-safe Rust host renders the shell (fixed zone layout, one color world,
heavily animated) and embeds web content through the Chromium Embedded
Framework (CEF). Season 1 delivers the runnable foundation: the shell, CEF
inside the Rust host via off-screen rendering, a living procedural background,
feathered compositing, and an isolated in-shell settings surface.

> Proprietary — Copyright (c) 2026 Sascha Daemgen IT and More Systems.
> All rights reserved. See `LICENSE`.

---

## State after CD-11 (Season 1 extended)

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
  proxy, so no "proxy changes all windows" bug). Each Tor window is on its **own
  circuit** (two Tor windows are unlinkable), a leak checklist is enforced per context
  (SOCKS proxy, WebRTC constrained, QUIC off, remote DNS), and the wiring is
  **fail-closed** (a slot never silently falls back to a direct connection — verified
  by an adversarial security review that caught three real leaks). Settings expose the
  engine switch, a "route new windows through Tor by default" toggle, and a live status
  readout. **Honest scope:** Tor mode hides your IP but is **not** Tor-Browser-grade —
  it does not provide anti-fingerprinting or change the TLS-layer fingerprint (browser
  hardening is a separate upcoming feature). *This is the host's second sanctioned
  outbound path (D-0004 → D-0027); the live routing/leak checks run on the maintainer's
  networked machine.*
* **Own start page, no Google, no saved websites (CD-14, D-0025):** every empty/new
  slot opens to an **own start page** served from the binary at `cyberdesk://start/`
  (same isolation as settings, **zero network**) — Google is gone. It is a black
  canvas with a faint micro-lattice, the glowing **Energy Core** (the reserved CD-06
  motif — a bright hollow core inside concentric rotating brand-cyan arcs, motion
  respecting `prefers-reduced-motion`), a search/address capsule (same host-side
  URL-vs-search classifier + chosen engine), and round favorite tiles that open in
  the slot. And **websites are not saved**: a restart always comes back to a single
  start-page slot — never restored open URLs (the privacy reversal of CD-10's
  session persistence; the `session_slots` table is dropped, purging any prior data).
  History and favorites are untouched.
* **Update awareness — the info area (CD-13, D-0023/D-0024):** a small status light
  top-right, beside the gear. Idle it is a faint ring; when an update is available
  it fills with the brand color, a subtle pulse, and a count. Click it for a
  **floating panel** listing what is out of date — CyberDesk itself and its CEF core
  — with release-notes links (open in a column) and a Dismiss (it re-appears only if
  a newer version ships). The panel is honest and calm when you are current. This is
  the host's **one and only** outbound connection: a single pinned CARVILON update
  manifest over HTTPS (the NetGuard exception, D-0023) — it queries nobody else, a
  missing feed is silent, and it **informs only** (no downloads, no installs; that
  is the signed-pipeline future). It is the seed of the later notification rail.
* **Floating command elements — the bar dies (CD-12, D-0021):** the single top bar
  is retired. **Every column carries its own floating command set** — back/forward/
  reload orbs and an address capsule — that reveals above *that* column and drives
  it; move the mouse into the gap above a column, or press `Ctrl+L` for the active
  one. They float on a **transparent band** over the Pulse Grid (only the pills
  paint; the background breathes between them) and glide as columns reflow. Favorites
  become **round tiles** in one shared launcher row. **Drag a favorite tile into a
  control gutter and it opens there as a new column** — the shell draws a ghost on
  the cursor and lights the gutters as drop zones, dropping into the nearest (at full
  capacity it navigates the column under the ghost instead; `ESC` cancels). Each
  column also gets a **floating close orb** (a ring + cross) at its top-outer corner,
  revealed on hover — a click closes that column.
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
  already full; ad/script popups stay suppressed. The workspace no longer **persists
  open websites** across restarts (CD-14, D-0025): every launch starts fresh at a
  single start-page slot — the earlier session-URL restore is reversed for privacy.
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
  reach the web (its navigation is confined to `cyberdesk://`). A **search-engine**
  select (Google / DuckDuckGo / Bing / Startpage, a token-styled custom dropdown,
  D-0015), a **glow-intensity** slider (50–220 %), and three toggles (animated
  background, feathered edges, and stay-in-foreground) are wired over a CEF
  message-router IPC bridge (`get_settings` / `set_setting`), applied live and
  persisted to SQLite.
* **One token source:** every style value — colors, radii, periods, amplitudes —
  comes from an embedded theme (`src/theme.toml`), resolved both into wgpu shader
  uniforms and into the settings/command pages' CSS custom properties. App state
  lives in a schema-versioned SQLite store under `%LOCALAPPDATA%\CyberDesk\` —
  settings, local history and favorites (D-0014); open websites are **not** saved
  (CD-14, D-0025). All local, no sync.

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
`CYBERDESK_CAPTURE_CLOSE=1` overlays a per-column **close orb** (ring + cross) and
`CYBERDESK_CAPTURE_DRAG=1` a **favorite-drag** sample (gutter drop zones + ghost) so
the CD-12 overlays can be eyeballed headlessly. `CYBERDESK_CAPTURE_INFO=idle|active`
draws the CD-13 **info glyph** (idle ring / filled disc + pulse + count).

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
| Click a column's **close orb** (hover its top-outer corner) | Close that column (the last one can't be closed) |
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
| Click the **Tor shield** (in a column's command set) | Route that window through Tor (or back to clearnet); the shield lights on Tor and pulses while the engine connects |
| `ESC` | Cancel a favorite drag, else hide the command set / info panel, else close the settings card, else quit |

An amber glyph in the address capsule marks a page served over plain `http://`
(e.g. `neverssl.com`, which stays http by design); `https` and internal pages
show no warning. The **gear** (top-right) opens the settings card with a live,
persisted **search-engine** select (Google / DuckDuckGo / Bing / Startpage — the
top-bar search fallback), a **glow-intensity** slider (50–220 %,
brightness of the animated background), and three toggles: **animated background**
(the Pulse Grid, or whichever background the template selects), **feathered
edges**, and **stay in foreground** (keep the fullscreen shell above other
windows; always off in `--windowed` dev mode).

---

## Project layout

```
cyberdesk/
├─ src/
│  ├─ main.rs        # entry point, CLI, process model
│  ├─ app.rs         # winit event loop, window, slot layout, per-slot input routing, nav keys, foreground guard
│  ├─ renderer.rs    # wgpu renderer: shell + per-slot page/placeholder/line compositing, capture
│  ├─ browser.rs     # CEF OSR (N slot views + 1 internal), custom scheme, isolation, settings/nav/command-set IPC
│  ├─ slots.rs       # slot layout engine (frame_layout/frame_capacity, asymmetric zones) + order mgmt, pure + unit-tested
│  ├─ theme.rs       # theme tokens -> shader uniforms + settings/command CSS vars
│  ├─ theme.toml     # the embedded "cyber" token set (single style source; [slots] section)
│  ├─ store.rs       # schema-versioned SQLite store (settings, history, favorites)
│  ├─ settings.rs    # live settings state (search engine, glow, toggles) over the shared store
│  ├─ memory.rs      # history + favorites domain layer (frecency suggestions) over the store
│  ├─ updates.rs     # update awareness (CD-13): pinned-manifest fetch/parse/compare, info items, background worker
│  ├─ tor.rs         # embedded Tor engine (CD-15): arti-client + per-slot local SOCKS5 relay, isolated circuits, off-thread bootstrap
│  ├─ pulsegrid.rs   # Pulse Grid background: seeded generator + life simulation
│  ├─ settings.html/.css/.js   # embedded internal settings page assets
│  ├─ command.html/.css/.js    # embedded floating command-set page assets (CD-12)
│  ├─ info.html/.css/.js        # embedded update-awareness info panel assets (CD-13)
│  ├─ start.html/.css/.js       # embedded own start page: Energy Core + search + favorites (CD-14)
│  ├─ ring.wgsl      # CARVILON ring — dormant since CD-06 (Season-2 motif)
│  ├─ pulsegrid_*.wgsl  # Pulse Grid: lattice (3 depth weaves) · sprite (SDF prims/pulses) · composite
│  ├─ deepfield.wgsl # Deep Field ("Calm" variant) background  ·  blit.wgsl (upscale)
│  ├─ page.wgsl      # per-slot page / settings panel compositing (feathering)
│  ├─ slot_placeholder.wgsl  # lazy-slot placeholder (fill + 7-segment index glyph)
│  ├─ slot_lines.wgsl        # per-slot loading line (top) + active accent (bottom)
│  ├─ drag.wgsl      # topmost command overlay: favorite-drag ghost/zones + close orbs (CD-12)
│  ├─ info_glyph.wgsl # update-awareness info glyph (idle ring / active disc + pulse + count, CD-13)
│  └─ gear.wgsl      # settings gear button
├─ scripts/
│  └─ fetch-cef.ps1  # downloads the pinned CEF version into vendor/cef/
├─ docs/                          # living project documents (English)
│  ├─ cyberdesk-architecture.md
│  ├─ cyberdesk-decisions.md      # D-0001 … D-0016
│  ├─ cyberdesk-security.md
│  ├─ cyberdesk-wire-format.md    # settings + navigation + top-bar IPC schema
│  ├─ cyberdesk-feature-backlog.md
│  └─ cyberdesk-roadmap.txt
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
* **CEF profile/cache:** kept isolated under `target/<profile>/cyberdesk-cache/`
  (git-ignored) — the surf zone deliberately shares no state with a separately
  installed Chrome.
