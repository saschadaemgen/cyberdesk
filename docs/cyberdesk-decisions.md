# CyberDesk - Decisions

Newest decision on top. Format: D number - date - decision - reasoning.

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
