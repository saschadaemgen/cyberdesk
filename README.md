<div align="center">

# Cyb3rD3sk

**Anonymous-research & reverse-engineering workbench.**
*Anonymous by design, verifiable by measurement.*

![License](https://img.shields.io/badge/license-AGPL--3.0-blue)
![Platform](https://img.shields.io/badge/platform-Windows-lightgrey)
![Built with](https://img.shields.io/badge/built%20with-Rust-orange)
![Engine](https://img.shields.io/badge/engine-Chromium%20149-brightgreen)
![Edition](https://img.shields.io/badge/edition-open%20core%20%2B%20Pro-purple)
![Status](https://img.shields.io/badge/status-in%20development-yellow)

</div>

---

Cyb3rD3sk is the anonymous-research and reverse-engineering workbench of the
CARVILON platform: a single fullscreen "cyber operating system" built as a
memory-safe Rust host that embeds Chromium for web content and renders
everything else itself - fixed zones, one color world, a living procedural
interface. It is made for people whose work depends on not being profiled:
researchers, analysts, and engineers who need multiple independent browsing
identities side by side, an IP-anonymity layer they control per window, and a
machine that keeps no record of where they have been.

## Features

### Shipped

- **Per-window Tor.** Every window is clearnet or Tor - your choice, per
  window, one click. Each Tor window runs on its own isolated circuit through
  the embedded pure-Rust Tor engine (arti); two Tor windows are unlinkable
  from each other, and the wiring is fail-closed: a window never silently
  falls back to a direct connection.
- **`.onion` support.** Tor windows open onion services - resolved inside the
  Tor network, never through clearnet DNS, with no exit-node exposure. A
  `.onion` in a clearnet window is refused before any lookup and offered the
  Tor path instead; a fail-closed guard keeps onion subresources and
  redirects away from clearnet DNS too.
- **Coherent fingerprint hardening - the Ampel.** One traffic-light control
  grades the protection: **Off / Green / Yellow / Red**. Green is the coherent
  everyday level - canvas, WebGL, GPU identity, audio, text metrics, fonts,
  device profile, screen and more, at zero layout cost; Yellow adds the
  aggressive clamps (clock precision, media/codecs, math);
  Red is the bunker - every vector tight plus a locked, standard-sized
  window so reported and real agree exactly. Settable globally and
  per-window; stable signals are clamped to common values, measured signals
  carry fresh per-session noise, and every exposed value stays mutually
  consistent. Stepping protection down passes a two-confirmation gate;
  stepping up is instant. A **new identity** is one click away - per window,
  on a timer you can watch count down, and fresh on every launch.
- **Zero unsolicited telemetry - proven by measurement.** The Chromium
  engine is fully de-Googled: every phone-home vector is disabled at the
  switch and preference level, in clearnet and Tor windows alike. The claim
  is measured, not asserted: a network-log audit on idle shows zero
  unsolicited connections to Google or telemetry (the recipe ships in `docs/`).
- **In-memory browsing.** Browsing content - history, cookies, cache - lives
  in RAM only and ends with the session; no browsing content is written to
  disk. An on-launch residue purge additionally sweeps the engine's scaffolding
  directory every start. Restoring a session is an explicit choice
  ("Quit & Save"), and even then only layout and clearnet URLs are kept -
  never content, never a Tor window's URL.
- **De-Googled search.** The address bar routes to the engine you choose -
  DuckDuckGo by default, never silently anything else.
- **The workbench shell.** Up to three fixed-width browser columns with
  per-column floating controls, a permanent multifunctional zone with a live
  viewer (Tor status and the application log, plus a reserved terminal tab -
  planned), an always-truthful HUD (clock, protection level, route - including
  **Tor · Onion** while on an onion service), and a seeded procedural
  circuit-board background that reacts to the shell.

### In development

- **Digital logic analyzer** - 8 channels at 24 MHz, with protocol decoders,
  rendered in the shell's own GPU pipeline.
- **Per-window VPN** - a second IP-anonymity route beside per-window Tor.
- **Credential & file vault** - quantum-resistant encrypted storage with
  streaming decryption; plaintext never touches disk.
- **Onion-service hosting** - serve a v3 onion service, not only visit them.
- **Professional edition** - the commercial tier layered on the open-core
  base; advanced tooling and support under commercial terms (see
  *Editions & license* below).

*Windows is the current platform; a Linux appliance is planned - every layer
(winit, wgpu, Chromium) is already portable.*

## Security & privacy

Two independent protections compose, and Cyb3rD3sk is precise about what each
one does.

- **Tracking-resistance - fingerprint hardening.** Stable browser signals are
  clamped to common values; measured signals carry fresh per-session noise. A
  site cannot link you across sessions or across sites, yet every exposed value
  stays mutually consistent - there is no half-spoofed contradiction that would
  make a window *more* identifiable, and no operating system, browser, or
  platform is faked. Coherent tracking-resistance, always on, in every window.
- **IP anonymity - per-window Tor.** A Tor window exits through the Tor network
  on its own isolated circuit; the site sees a Tor address, never yours, and the
  two protections stack. `.onion` traffic never leaves Tor at all - it is
  resolved by the hidden-service rendezvous inside the network, with no exit
  node and no clearnet DNS.
- **No disk trace.** Browsing content lives in memory and is gone when the
  session ends; a standing on-launch purge sweeps the engine's scaffolding
  directory as a backstop.
- **Proven by measurement, not asserted.** The de-Googled claim is backed by a
  network-log audit - on idle, zero unsolicited connections to Google or
  telemetry. The audit recipe ships in `docs/` so the result can be reproduced.

The full engineering detail of each protection layer lives in the internal
`docs/`.

## Editions & license

Cyb3rD3sk is **open core**:

- **The base is free software** under the **GNU Affero General Public License,
  version 3.0** - full text in [`LICENSE`](LICENSE). AGPL's network-use clause
  applies: distribute a modified version, or run one as a network service, and
  you make its source available in turn.
- **The Professional edition** is a separate commercial product that layers
  advanced capabilities on the open-core base under commercial terms. It is
  licensed apart from the AGPL core; the open-core license does not extend to it.

Third-party components - the Chromium Embedded Framework and the Rust crate
dependencies - keep their own licenses; see [`NOTICE`](NOTICE).

## Lineage & interop

Cyb3rD3sk is built by the makers of **SimpleGo**, and is compatible with
**SimpleGo** and **SimpleGoX** - SimpleGo's native-Rust encrypted-messaging
client that speaks SimpleX, Matrix, and Telegram over Tor and I2P. That
messaging capability is SimpleGoX's own; Cyb3rD3sk is designed to run alongside
it, not to reimplement it.

## Build & run

Windows 11 (x64, MSVC). You need Rust (`x86_64-pc-windows-msvc`), Visual Studio
2022 with "Desktop development with C++", and CMake, Ninja, and Python 3 on
`PATH` (they build the CEF wrapper).

```pwsh
# 1. fetch the pinned Chromium/CEF binaries (once; hundreds of MB, never committed)
./scripts/fetch-cef.ps1

# 2. build
cargo build --release

# 3. run - windowed dev mode (1600×900) …
./target/release/cyberdesk.exe --windowed
# … or fullscreen
cargo run --release
```

`CEF_PATH` is preset to `vendor/cef/` in `.cargo/config.toml`, so no manual
configuration is needed. The first build is slower because CMake + Ninja compile
the CEF wrapper.

## Project layout

```
cyberdesk/
├─ src/
│  ├─ main.rs        # entry point, CLI, process model
│  ├─ app.rs         # event loop, window, slot layout, input routing, HUD/frame push
│  ├─ renderer.rs    # wgpu renderer: shell + per-slot compositing, capture
│  ├─ browser.rs     # CEF OSR views, custom scheme, isolation, IPC allowlist, onion guard
│  ├─ slots.rs       # slot/zone layout engine (pure, unit-tested)
│  ├─ tor.rs         # embedded Tor engine (arti): per-slot SOCKS relay, onion-capable
│  ├─ harden.rs      # fingerprint-hardening config: Ampel levels, per-vector flags, gate
│  ├─ hardening.js   # document-start farbling + clamps, config-gated per vector
│  ├─ degoogle.rs    # engine phone-home kill switches / prefs, per-context
│  ├─ forensic.rs    # anti-forensic on-launch residue purge + footprint readout
│  ├─ store.rs       # schema-versioned SQLite (settings, favorites; RAM-only history)
│  ├─ settings.rs · memory.rs · updates.rs · logging.rs · fsprobe.rs · theme.rs
│  ├─ pulsegrid.rs   # procedural circuit-board background: generator + life sim
│  ├─ *.html/.css/.js   # embedded internal pages (settings, command, start, mfzone, hud, onion, info)
│  └─ *.wgsl         # shaders (Pulse Grid, page/feather, slot lines, overlays, gear)
├─ scripts/
│  ├─ fetch-cef.ps1        # downloads the pinned CEF version into vendor/cef/
│  └─ harden-selftest.mjs  # headless checks of the hardening JS
├─ docs/                   # living technical docs (architecture, security, wire-format,
│                          #   feature-backlog, decisions, degoogle-audit)
├─ .cargo/config.toml
└─ vendor/cef/             # (git-ignored) CEF binaries - fetched, never committed
```

## Status

Pre-1.0 and in active development. The workbench and its full privacy stack are
**shipped and usable**: per-window Tor with `.onion` support, the Ampel
fingerprint hardening, proven-zero telemetry, in-memory browsing, and the
multi-column shell with its live viewer and HUD. **In development:** the digital
logic analyzer, per-window VPN, the credential/file vault, onion-service hosting,
and the Professional edition. **Platform:** Windows today, a Linux appliance
planned. The development history lives in `docs/` (the decision log and season
protocols), not here - this page tracks what Cyb3rD3sk *is*, not how it got here.
