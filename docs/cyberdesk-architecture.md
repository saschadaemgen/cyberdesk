# CyberDesk - Architecture

Project CARVILON CyberDesk - living document - Status: 2026-07-08 (before CD-01 completion)
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

CD-01 complete: shell (winit/wgpu, borderless fullscreen, rotating ring) plus chromeless CEF windowed embed of google.com, verified on the 5120x1440 ultrawide target display. Next: CD-02 (OSR - the page becomes a GPU texture). This document is updated after every season, and mid-season when necessary.
