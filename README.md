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

## State after CD-03 (Season 1 complete)

* **Shell:** Borderless fullscreen on the primary monitor, dark background
  (`#04070A`), a slowly rotating CARVILON ring (open arc + hollow inner ring,
  `#009FE3`) that frames the surf zone, vsync. `ESC` quits cleanly (from
  anywhere, even with the page focused). Dev mode via `--windowed` (1600×900).
* **Deep Field background:** a procedural, texture-free living background behind
  everything — a breathing glow, two drifting nebula gradients, sparse twinkling
  dust, and a rare scan sweep — rendered at half resolution and upscaled, within
  a tight amplitude budget (no flicker, ~6–8% brightness delta).
* **Surf zone (CEF, off-screen rendering):** CEF renders the page off-screen
  (`on_paint`); CyberDesk uploads each frame into a wgpu texture and composites
  it inside its own frame — the page sits centered (~60% × 70%) with the shell
  visible around it. Its edges are **feathered** (a soft SDF-based alpha falloff
  that dissolves the page into the Deep Field; toggleable back to the hard
  rounded corner). Mouse and keyboard are forwarded into the page (a Google
  search, clicking, and scrolling all work) and the cursor follows the page.
* **Settings:** a gear button (top-right) opens an in-shell settings card — a
  **second, web-isolated OSR view** locked to an internal `cyberdesk://` custom
  scheme (D-0010), served entirely in-process from embedded assets. It can never
  reach the web (its navigation is confined to `cyberdesk://`). Two toggles
  (feathered edges, Deep Field) are wired over a CEF message-router IPC bridge
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

* **`ESC`** quits the application cleanly (or closes the settings card if open).
* The **gear** button (top-right) opens the settings card; the two toggles apply
  live and persist across restarts.
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

## Project layout

```
cyberdesk/
├─ src/
│  ├─ main.rs        # entry point, CLI, process model
│  ├─ app.rs         # winit event loop, window, input routing, gear, ESC
│  ├─ renderer.rs    # wgpu renderer: shell + page/panel compositing, capture
│  ├─ browser.rs     # CEF OSR (two views), custom scheme, isolation, IPC
│  ├─ theme.rs       # theme tokens -> shader uniforms + settings CSS vars
│  ├─ theme.toml     # the embedded "cyber" token set (single style source)
│  ├─ store.rs       # schema-versioned SQLite app-state store
│  ├─ settings.rs    # live settings state (owns the store) shared with the IPC
│  ├─ settings.html/.css/.js   # embedded internal settings page assets
│  ├─ ring.wgsl      # background + CARVILON ring
│  ├─ deepfield.wgsl # procedural Deep Field background   ·  blit.wgsl (upscale)
│  ├─ page.wgsl      # surf-zone page / settings panel compositing (feathering)
│  └─ gear.wgsl      # settings gear button
├─ scripts/
│  └─ fetch-cef.ps1  # downloads the pinned CEF version into vendor/cef/
├─ docs/                          # living project documents (English)
│  ├─ cyberdesk-architecture.md
│  ├─ cyberdesk-decisions.md      # D-0001 … D-0010
│  ├─ cyberdesk-security.md
│  ├─ cyberdesk-wire-format.md    # settings IPC schema
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
