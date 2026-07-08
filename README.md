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

## State after CD-05 (Season 1 extended)

* **Shell:** Borderless fullscreen on the primary monitor, dark background
  (`#04070A`), a slowly rotating CARVILON ring (open arc + hollow inner ring,
  `#009FE3`) that frames the surf zone, vsync. `ESC` quits cleanly (from
  anywhere, even with the page focused). Dev mode via `--windowed` (1600×900).
* **Pulse Grid background:** a seeded circuit board that lives behind the shell —
  a fine micro lattice, routed traces with pads and solder dots, two full-width
  bus lines, bright pulses with fading trails travelling the traces, and
  occasional expanding node flares. The static layer is baked once into a
  full-resolution texture (imperceptible startup cost) and composited each frame,
  scaled by a live **glow-intensity** control; the board is identical across
  launches (seed determinism). It is allowed to glow — a **zone shadow** dims it
  under the surf zone and any open overlay (calm under content, glow in the
  margins) so the page stays readable. The earlier **Deep Field** (a breathing
  glow with drifting nebulae, dust, and a scan sweep) is preserved as a
  token-selectable "Calm" variant (`background.kind = "deep_field"`). See
  `docs/cyberdesk-decisions.md` (D-0012).
* **Surf zone (CEF, off-screen rendering):** CEF renders the page off-screen
  (`on_paint`); CyberDesk uploads each frame into a wgpu texture and composites
  it inside its own frame — the page sits centered (~60% × 70%) with the shell
  visible around it. Its edges are **feathered** (a soft SDF-based alpha falloff
  that dissolves the page into the Deep Field; toggleable back to the hard
  rounded corner). Mouse and keyboard are forwarded into the page (a Google
  search, clicking, and scrolling all work) and the cursor follows the page.
* **Free surfing (command bar + history):** `Ctrl+L` summons a command bar over
  the surf zone; its text is classified host-side as a URL or a Google search.
  Back / forward / reload and the mouse's forward/back buttons drive the page
  history, an amber glyph flags a plain-`http://` page, and a loading line traces
  the top of the zone. Popups follow a gesture-aware policy (D-0011): a real
  click on a `target=_blank` link navigates the surf view in place, script
  `window.open` is dropped — no second window ever opens.
* **Settings:** a gear button (top-right) opens an in-shell settings card — a
  **second, web-isolated OSR view** locked to an internal `cyberdesk://` custom
  scheme (D-0010), served entirely in-process from embedded assets. It can never
  reach the web (its navigation is confined to `cyberdesk://`). A **glow-intensity**
  slider (50–220 %) plus three toggles (animated background, feathered edges, and
  stay-in-foreground) are wired over a CEF message-router IPC bridge
  (`get_settings` / `set_setting`), applied live and persisted to SQLite.
* **One token source:** every style value — colors, radii, periods, amplitudes —
  comes from an embedded theme (`src/theme.toml`), resolved both into wgpu shader
  uniforms and into the settings page's CSS custom properties. App state lives in
  a schema-versioned SQLite store under `%LOCALAPPDATA%\CyberDesk\`.

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

* **`Ctrl+L`** opens the command bar; **`ESC`** closes an open overlay (command
  bar or settings), otherwise quits. See **Controls** below for the full map.
* The **gear** button (top-right) opens the settings card; the three toggles
  apply live and persist across restarts.
* The first build is slow because CMake+Ninja compile `libcef_dll_wrapper`. The
  CEF runtime files (`libcef.dll`, resources, `locales/`) are copied next to the
  `.exe` in `target/<profile>/` automatically.

### Optional: headless render self-test

Renders a single ring frame off-screen to a PNG file (useful for CI / visual
regression; does not touch any desktop):

```pwsh
cargo run --release -- --capture ring.png
```

---

## Controls

The surf zone behaves like a stripped-down browser with no visible chrome until
you summon it. All navigation shortcuts act on the surf view.

| Input | Action |
| --- | --- |
| `Ctrl+L` | Open the command bar over the surf zone (from any state) |
| type + `Enter` (in the bar) | Navigate — a scheme, a dotted host, or `localhost` loads as a URL (default `https://`); anything else becomes a Google search |
| `Alt+←` / `Alt+→` | History back / forward |
| Mouse button 4 / 5 | History back / forward |
| `F5` / `Ctrl+R` | Reload |
| `Ctrl+Shift+R` | Hard reload (ignore cache) |
| `ESC` | Close the command bar or settings card if open, otherwise quit |

An amber glyph in the command bar marks a page served over plain `http://`
(e.g. `neverssl.com`, which stays http by design); `https` and internal pages
show no warning. The **gear** (top-right) opens the settings card with a live,
persisted **glow-intensity** slider (50–220 %, brightness of the animated
background) and three toggles: **animated background** (the Pulse Grid, or
whichever background the template selects), **feathered edges**, and **stay in
foreground** (keep the fullscreen shell above other windows; always off in
`--windowed` dev mode).

---

## Project layout

```
cyberdesk/
├─ src/
│  ├─ main.rs        # entry point, CLI, process model
│  ├─ app.rs         # winit event loop, window, input routing, nav keys, foreground guard
│  ├─ renderer.rs    # wgpu renderer: shell + page/panel compositing, capture
│  ├─ browser.rs     # CEF OSR (two views), custom scheme, isolation, settings + nav IPC
│  ├─ theme.rs       # theme tokens -> shader uniforms + settings CSS vars
│  ├─ theme.toml     # the embedded "cyber" token set (single style source)
│  ├─ store.rs       # schema-versioned SQLite app-state store
│  ├─ settings.rs    # live settings state (owns the store) shared with the IPC
│  ├─ pulsegrid.rs   # Pulse Grid background: seeded generator + life simulation
│  ├─ settings.html/.css/.js   # embedded internal settings page assets
│  ├─ command.html/.css/.js    # embedded command-bar page assets
│  ├─ ring.wgsl      # background + CARVILON ring
│  ├─ pulsegrid_*.wgsl  # Pulse Grid: lattice · sprite (SDF prims/pulses) · composite
│  ├─ deepfield.wgsl # Deep Field ("Calm" variant) background  ·  blit.wgsl (upscale)
│  ├─ page.wgsl      # surf-zone page / settings panel compositing (feathering)
│  ├─ loading.wgsl   # surf-zone loading line
│  └─ gear.wgsl      # settings gear button
├─ scripts/
│  └─ fetch-cef.ps1  # downloads the pinned CEF version into vendor/cef/
├─ docs/                          # living project documents (English)
│  ├─ cyberdesk-architecture.md
│  ├─ cyberdesk-decisions.md      # D-0001 … D-0012
│  ├─ cyberdesk-security.md
│  ├─ cyberdesk-wire-format.md    # settings + navigation IPC schema
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
* **Black instead of dark background / no ring:** check the graphics driver;
  wgpu needs a working D3D12 or Vulkan backend adapter.
* **`GPU process exited unexpectedly` on stderr:** this was a CD-01 child-window
  issue; under CD-02's off-screen rendering the GPU process is healthy and the
  message no longer appears (see `docs/cyberdesk-decisions.md`, D-0009). If it
  does show, CEF falls back to SwiftShader and the page still renders.
* **CEF profile/cache:** kept isolated under `target/<profile>/cyberdesk-cache/`
  (git-ignored) — the surf zone deliberately shares no state with a separately
  installed Chrome.
