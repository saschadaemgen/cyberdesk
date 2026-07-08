# CyberDesk - Architecture

Project CARVILON CyberDesk - living document - Status: 2026-07-08 (Season 1 extended - CD-05 complete)
Proprietary - Copyright (c) 2026 Sascha Daemgen IT and More Systems. All rights reserved.

## What CyberDesk is

A single fullscreen application in the style of a serious cyber operating system: fixed zone layout, one color world (base #04070A, brand blue #009FE3), heavily animated, optimized for ultrawide displays (target roughly 1.20 m of screen width, 16:9 as fallback). Only CARVILON-built applications run inside it, plus one surf zone. No freely movable windows - predictability is the core of the product.

## Layer model

1. **Zones (fixed):** Spine on the left, main area (three sizes S/M/L with reflow of neighboring zones), video zone, terminal zone, right tab rail (Status, Files, FTP, Music, ...). Positions are law. Changes only in Edit Mode within the grid rules (D-0007).
2. **Modes (layout presets):** Standard, Admin (larger main area), Do-Not-Disturb, and others. A mode loads a preset of the same fixed layout - it never invents a new one.
3. **Event Priority Engine:** Events (doorbell, call, alarm, security alert) carry priorities, zones carry ranks, the mode acts as the gate. Decision per event: override, overlay, or suppress. Even interruptions are regulated.

## Process and technology model

- **Rust host:** window management (winit), rendering (wgpu), zones/modes/event engine, later crypto (Argon2id, Zeroize) and start authorization.
- **CEF (Chromium Embedded Framework):** delivers pixels of the surf zone, nothing else. CD-01: windowed embed as feasibility proof; from CD-02 on, offscreen rendering into a GPU texture, then feathering/compositing inside our own frame (soft edges bleeding into the design). CEF runs with an isolated browser profile (own root_cache_path) - the surf zone never shares state with any user-installed browser.
- **Hard process boundary host<->CEF**, IPC only through an explicit allowlist. No Electron, no Node, no npm chain in the core. Chromium sandbox stays active.
- **NetGuard:** no module opens connections on its own; everything goes through the central network layer (deny-by-default per zone, certificate pinning, own DNS resolver, kill switch, counters). Browser traffic attaches to the same monitor via CefRequestHandler.

## Platform path

Development: Windows 11 (MSVC). Later: Linux appliance. Long-term goal: CARVILON OS (Debian 13 "Trixie") booting directly into CyberDesk as its shell - the app is the first deliverable and the later heart of the OS. Nothing on the app path is throwaway work.

## Status

Season 1 extended (CD-05) complete. CD-01: shell (winit/wgpu, borderless fullscreen, rotating ring) plus a chromeless CEF windowed embed of google.com. CD-02: off-screen rendering - the page becomes a wgpu texture composited inside our own frame (CPU path; accelerated path researched, D-0009). CD-03: feathered surf-zone edges, the procedural Deep Field background, and a web-isolated `cyberdesk://` settings view over a message-router IPC bridge (D-0010). CD-04: free surfing - a `Ctrl+L` command bar (host-side URL-vs-search), history navigation (Alt+arrows, mouse buttons 4/5, F5 / Ctrl+R / Ctrl+Shift+R), a gesture-aware popup policy (D-0011), a loading line, and a tier-1 foreground guard. CD-05: background v2 - the seeded, bake-once "Pulse Grid" circuit board (traces, pads, bus lines, travelling pulses, node flares) replaces the Deep Field as the Cyber default and is allowed to glow, with a zone shadow dimming it under content and a live glow-intensity slider; the Deep Field is demoted to a token-selectable "Calm" variant (D-0012). Next: Season 2 (zone layout and the Edit Mode grid). This document is updated after every season, and mid-season when necessary.
