# CyberDesk - Feature Backlog

Soft season mapping - chat fill level and reality decide the actual cuts. Completed items move into the respective season protocol.

## Foundation (Season 1, running)

- CD-01 shell skeleton (winit/wgpu, fullscreen, rotating ring, ESC, --windowed) + CEF windowed with google.com [with CC]
- CD-02 OSR PoC: web page as GPU texture inside our own frame
- CD-03 feathering/compositing: soft edges, animated background behind the content

## Design (Season 2)

- Design law: color world, motion language, background shaders - reference files by CD (Claude Design) are binding, CC implements them 1:1
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

- Favorites and history in SQLite, own buttons away from the content
- Downloads land in the Files zone; context menu, JS dialogs, and popups in our own design
- Request filter as adblock foundation (filter lists on the network level)
- Later: Widevine for streaming DRM (plan for Google's license process), autofill via the Rust password core

## Crypto + authorization (Season 6)

- Argon2id, Zeroize, encrypted app state, start authorization, key management
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
