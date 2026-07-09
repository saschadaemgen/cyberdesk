# CyberDesk - Decisions

Newest decision on top. Format: D number - date - decision - reasoning.

## D-0017 - 2026-07-09 - CD-09: the multi-slot engine (columns, lazy spawn, focus routing) and the D-0009 verdict

CD-09 turns the single surf zone into the zone system the whole season was built
for: up to four **fixed-width content columns** side by side, aligned by the
layout math, never crooked. This is the heart ticket.

**Slot model (the law).** A **slot** is a fixed-width content column: `slot_width`
(1200 logical px) wide, as tall as the surf zone (`height_frac` = 70 % of the
window, vertically centered), with `slot_gutter` (24 px) between slots; the group
is horizontally centered and never comes within `min_margin` (48 px) of the edge.
All in the new `[slots]` theme section (one token source, as always). `slot_width`
is tuned so **four columns fit the 5120 ultrawide** (4·1200 + 3·24 = 4872 < 5120);
`max_slots(width)` returns what fits (4 on 5120, 3 on 3840, 2 on 2560, 1 on 1920),
clamped to `MAX_SLOTS` = 4, never below 1. The Pulse Grid glows in the gutters and
margins — intended and beautiful.

- **Lazy slots.** A new slot has NO browser until its first navigation; until then
  the shell draws a placeholder (a rounded fill lifted above the base color, with
  the slot's index as a faint 7-segment glyph — purely shell-side, no CEF, so a new
  column appears instantly with no white about:blank flash). Slot 0 loads the home
  page eagerly (parity); the rest spawn on the first `navigate` targeted at them
  (queued to the main thread, which owns the HWND).
- **One active slot.** Keyboard input, the top bar and the scheme hint act on it.
  Active indication: a thin 2 px brand accent along the slot's BOTTOM edge (the top
  edge belongs to the loading line). Only the active slot's browser holds CEF focus;
  switching moves focus (set_focus 0 on the old, 1 on the new).

**Stable-id order model (the key architecture decision).** Slots are tracked as an
ordered list of stable **ids** (`order: Vec<usize>`), each id a fixed index into the
per-slot browser/texture arrays. A slot keeps its id — and therefore its CEF client/
handlers (which bake in `Role::Slot(id)`) and its wgpu texture — for its whole life;
only its *position* in `order` changes when columns are added, closed or recentered.
This avoids ever migrating a live browser between indices (which would desync the
handlers' baked role from where their `on_paint` writes). Ctrl+T inserts a free id
right of the active one; Ctrl+W removes the active id and promotes the nearest
neighbor; Ctrl+1..4 focus by position; Ctrl+Tab / Ctrl+Shift+Tab cycle. The
positional index logic is pure and unit-tested (`slots.rs`).

**Rendering.** The single page pass became a shared `PagePipeline` + per-target
`PageTarget` (one per slot + the overlay); the render loop draws each painted slot's
texture (feathered, at its rect), one instanced placeholder pass for empty slots, and
one instanced slot-lines pass (per-slot loading line at the top edge + the active
accent at the bottom). The zone-shadow uniform grew from 4 to **6 rects** (up to
MAX_SLOTS slots + the one open overlay; std140 array of vec4, both pulse shaders
updated). `--capture` gained `CYBERDESK_CAPTURE_SLOTS=N` to render N placeholder
columns, so the four-column money shot is verifiable headlessly (no desktop scrape).

**Input routing.** Mouse events route to the view under the cursor (the slot whose
rect *contains* it, or the overlay) at coordinates relative to that view; crossing
views sends a mouse-leave so hover states clear; a click inside a slot makes it
active; cursor-icon feedback comes from the hovered view. Keyboard routes to the
active slot; the slot-management shortcuts are intercepted host-side first. The top
bar acts on the active slot — and this needed **no new IPC**: the existing
`get_nav_state` / `navigate` / `go_back` / `go_forward` / `reload` now read and drive
`browser::active_slot()` internally, so the wire format is unchanged (verified). All
slots' visits record into the one shared history. The gesture-aware popup policy
(D-0011) stays per-slot (a user-gesture popup navigates its own slot's main frame).

**Reasoned deviations (documented per the standing rule).** The single-slot state is
no longer pixel-identical to CD-08: a lone column is now a fixed 1200 px wide (the
slot model's natural single form) rather than the old 60 %-of-width zone — wider on
the 1600 dev window, narrower on the 5120 ultrawide (where a single centered column
with generous glowing margins is the intended aesthetic) — and it carries the active
accent line (the slot law mandates exactly one active slot at all times). Behavior
(feathering, loading line, bar, favorites, history, popups, nav keys) is identical;
"parity" is behavioral, not pixel-exact. This is the deliberate consequence of
adopting the slot model, not a regression.

**Performance gate — the D-0009 measurement moment. VERDICT: the trigger did NOT
fire.** Measured on an NVIDIA RTX 3090 at 5120×1440 with **4 slots** (1200×1008 each,
**18.5 MB/frame** of page uploads), 300 frames after warmup, via a temporary headless
harness (main-thread `write_texture` staging + the full shell composite + submit +
GPU wait; since removed):

- Per-slot upload (4 slots, staging): **median 3.0 ms, p99 4.0 ms, max 4.9 ms**.
- Full frame (upload + composite + submit + wait): **median 4.45 ms, p99 6.2 ms,
  max 6.8 ms**.
- 60 fps frame budget: 16.667 ms.

The worst frame (6.8 ms) sits well under the budget with ~10 ms of headroom, so
4-slot browsing does **not** stutter and the CPU OSR path stays viable — D-0009's
stutter trigger has not fired for this ticket. **But** the uploads are already the
single dominant frame cost (3.0 of 4.45 ms, ~68 %) and scale linearly with slot
count and resolution, exactly as D-0009 predicted once per-pixel throughput started
to matter. So the **recommendation** stands as D-0009 framed it: the accelerated,
zero-copy shared-texture path (D-0009 option a — replicate cef-rs's D3D11 importer
against wgpu-30's DX12 hal) is the well-scoped next optimization to reclaim that
headroom, and becomes necessary sooner on weaker GPUs, higher DPI, or if slot counts
grow — but it is **not required now** and stays out of scope for CD-09 (measure,
record, recommend). Caveat: the harness measures the main-thread upload + composite
(what governs the render loop's 60 fps), not the CEF-side `on_paint` memcpy (a
separate thread), and forces a full GPU sync per frame (more pessimistic than
vsync-pipelined real frames), so the real on-screen margin is at least as good.

## D-0016 - 2026-07-09 - CD-08: the command surface is a hover-reveal top bar

Sascha's CD-07 acceptance changed the command surface: from the centered command
palette (D-0014) to a **hover-reveal top bar** living in the free gap above the
surf zone. This explicitly **revises D-0014's "no favorites bar"** — a favorites
surface with its own clickable controls now exists, as Sascha's call. It is a
functional v1 in the token world; the design-law polish stays Season 2.

**Surface.** The bar spans the surf-zone width, anchored to the top edge. It
holds the address input (scheme hint + star + back/forward/reload glyphs) and,
below it, one of two bodies: the favorites as clickable **chips** (title + star,
click navigates) while the input is untouched, or the CD-07 live suggestion
**list** while typing. The palette logic is reused wholesale — only the surface
moved. Chips reuse the empty-`input` `query_suggestions` (favorites, capped at
`command.max_results`); there is no separate favorites command (implementer's
call from the briefing). Favorites beyond the cap are not chipped in v1
(management UI is a later ticket).

**Reveal** (slide down, ~180 ms ease-out, host-side): the cursor enters the top
hot zone (the gap band above the surf zone, full surf width), OR Ctrl+L (which
also focuses + selects the input — unchanged). **Hide** (slide up, same ease):
the cursor leaves the union of hot zone + bar rect with a **~250 ms hysteresis**
(no flicker on grazing touches), OR a navigation commits, OR ESC. **Typing
exception:** while the input is focused and holds text (the prefilled URL counts,
so a keyboard user is never cut off), a mouse-out does NOT hide the bar — only
ESC, Enter (navigate), or a chip/suggestion click end it then. A **Ctrl+L reveal**
additionally only becomes subject to the mouse-out hysteresis once the cursor has
*engaged* the bar (entered it at least once), so a keyboard reveal is never
hidden before the user can type. ESC chain is now **bar -> settings -> quit**.

**Mechanics.** A tiny host-side state machine (a 0..1 slide `progress`, eased,
plus a hysteresis deadline and an "engaged" flag) drives the one shared internal
OSR view (mutually exclusive with the settings card, as before). The page renders
at the full bar size; the compositor reveals it from the top edge by
**scissor-clipping** the panel draw to `progress * height` — the bar is drawn with
square corners flush to the top, and the zone shadow dims only the visible slice.
The bar height is computed host-side from the shared theme tokens (`input_height`
+ the new `chip_row`, or `input_height + N*row_height + 2*list_pad`), so the page
CSS and the composite stay in lockstep as the body changes — no per-frame
allocation, no page-reported geometry. Two IPC additions carry the little the
host cannot derive: `autofocus` in `get_nav_state` (Ctrl+L vs hover) and a
`bar_typing` signal for the mouse-out exception (see wire-format). The loading
line stays at the surf zone's top edge; the bar lives above it in the gap and
composites over it only where a tall body overlaps the surf zone's top margin.

**Favorites bug (Stage A), measured first.** The CD-07 "only one favorite ever
shows" was diagnosed on the real DB before any code change (measure-before-
guessing): storage and the empty-input query are correct — two distinct URLs
produce two rows and `query_suggestions("")` returns both (verified on a scratch
DB and by a regression test). The fault was the display: the palette prefilled
its input with the current surf URL and filtered the suggestion list by it, so
only the favorite matching the current page ever showed; the D-0014 empty-input
favorites surface was never reached on open. The fix (committed separately as
`fix(command)`, not the briefing's `fix(memory)` — the memory layer was never at
fault) shows the full favorites list while the input holds the untouched
prefilled URL; the top bar's chips then make the favorites surface explicit.

## D-0015 - 2026-07-08 - CD-07: the settings "select" is a custom in-page dropdown

The search-engine setting needs a select control, but the internal views are
**off-screen (OSR)** and `RenderHandler::on_paint` only composites the main VIEW
element — native popup widgets (`PaintElementType::POPUP`) are deliberately
ignored (consistent with "no context menu" through Season 5). A native
`<select>` would open its option list as exactly such a popup, which would never
paint — the dropdown would be invisible.

So the "select control" is a **custom in-page dropdown**: a button plus an
absolutely-positioned `<ul>` menu, all ordinary markup that composites in the one
VIEW texture like everything else on the page. It also themes perfectly to the
token world (native option lists can't be fully styled anyway) and matches the
slider's design language. The menu opens downward within the settings card (the
search-engine row sits at the top, with room below). Reasoned deviation from the
literal "select"; the behaviour and look are a select, the mechanism is ours.

## D-0014 - 2026-07-08 - CD-07: the command palette IS the favorites/history surface

CyberDesk gains local memory — a `history` table (url, title, last_visit,
visit_count) and a `favorites` table (url, title, added_at, position) in the same
`state.db` (schema v2). Recorded on the surf view only; `cyberdesk://` and blank
navigations never enter either table. No sync, no export.

**No favorites bar — the command bar becomes a command palette.** The deliberate
design law: CyberDesk shows NO favorites bar and NO browser-chrome imitation. The
one command surface (`Ctrl+L`) is where favorites and history live — as live
suggestions below the input. A visual favorites surface with its own buttons is
**Season-2 design-law material**, not this ticket. `Ctrl+D` (or the command-bar
star) favorites the current page; the star reflects and toggles the current
page's state live.

**History cap + pruning.** History is capped at **~10,000 rows**; each insert
prunes the least-recently-visited rows past the cap (`DELETE … ORDER BY
last_visit DESC LIMIT -1 OFFSET 10000`). A visit is one upsert per real address
change (bump `visit_count`, refresh `last_visit`); the title is filled in when it
arrives (it lands after the address commit).

**Frecency (kept honest and simple).** History suggestions rank by
`visit_count * recency_weight`, where the weight is bucketed by the age of the
last visit: `<1 h → 100`, `<1 day → 80`, `<1 week → 60`, `<30 days → 40`, else
`20`. Favorites always outrank history and are shown first (in their saved
order); a favorite is excluded from the history half so it appears once. Matching
is a case-insensitive substring on url + title, with LIKE wildcards in the input
escaped. The whole ranking and matching runs **host-side** in the IPC handler;
the page only renders what it is given (query per keystroke, debounced ~90 ms).

**Palette sizing.** The palette view is resized to fit exactly `input bar + N
rows` (grows and shrinks with the live suggestion count, primed on open). The row
and input dimensions are theme tokens (`[command]` in `theme.toml`), emitted as
`--cmd-*` CSS vars, so the page CSS and the host-side rect share one source of
truth — no hardcoded sizes, and no favorites-area scrim over empty space.

## D-0013 - 2026-07-08 - CD-06: depth overhaul, ring removed, feather corrected, autonomous push

Sascha's verdict on the CD-05 visuals: the background looked like "800x600 Amiga
times" — far too little content for a 5120x1440 canvas (~1.2k primitives), the
effect too predictable, and the rotating ring in the middle still ugly. CD-06
fixes all three, plus the parked feather correction, and changes the push policy.

**Ring removed from the shell.** The rotating CARVILON ring no longer renders in
the shell pass or the `--capture` path (the capture composites the background
faithfully since CD-05, so it needs no ring backdrop). The shell background is
the Pulse Grid alone. The ring shader/module (`ring.wgsl`, `RingUniforms`,
`ring_pipeline`) stays in the tree **dormant** (`#[allow(dead_code)]`) — its
future is the start animation and the Energy Core interaction motif (Season 2),
so it is demoted, not deleted.

**Depth-layer architecture (the 10x).** The Pulse Grid is now **three depth
layers** — far → mid → near — each its own generated board, all baked additively
into the one HDR texture (draw order is cosmetic). At ultrawide this lifts the
content from ~1.2k to **12,424 baked primitives** (~10x), baked in **3.6 ms**
(still imperceptible; single-digit ms as required). Rationale: one flat layer at
any density still reads flat and repetitive; real depth (a crisp bright front, a
dimmer middle, a faint fine recede) is what makes a 1.2 m canvas read as *deep*
rather than merely busy.
- **Far**: finest lattice (~half the near cell), ~4x the near trace count,
  thinnest lines, ~0.36 brightness. **Mid**: between the two (~0.68 cell, ~2x
  count, ~0.6 brightness). **Near**: the CD-05 scale/brightness — the crisp
  bright front; the two bus lines and the flare-anchor pads live here.
- **Per-layer seeds** derive deterministically from `background.seed` (three
  sub-seeds pulled from a master splitmix64), so the determinism contract holds
  across launches (verified: byte-identical captures). The micro-lattice now
  sums three weaves (far/mid/near cells) in a single fullscreen pass.
- **Component vocabulary** kills the uniform random-walk predictability: **chip
  footprints** (outline rectangle + pin-pad rows on 2 or 4 edges, near/mid),
  **via clusters** (3–8 filled dots, all layers), **junction hubs** (pads with
  several traces routed toward them), and **varied segment distribution** (short
  zigzags mixed with occasional long straight runs, especially on far).
- **Life across depth**: pulse count, speed, brightness and head size scale per
  layer (near bright/fast, mid fewer/dimmer, far sparse/slow/faint — depth in
  motion); node flares stay near-layer. The HDR bake target and the zone shadow
  are unchanged and keep working across all three layers. All counts, sizes and
  scales are theme tokens; the generator kept the CD-05 instance/sprite pipeline
  (more primitives and vocabulary, not a new renderer).

**Feather corrected (parked CD-05 verdict).** The 34 px smoothstep feather read
as a 3D/vignette curve — the page was already >50% transparent 16 px inside its
edge and faded over a wide creamy band, so it seemed to curve away. The band is
narrowed to **12 px** (`feather_width`) and a **falloff curve exponent** token
(`feather_exp = 0.45`) applies `pow()` to the edge coverage in `page.wgsl`: the
page now stays fully opaque until ~10 px from the edge, then fades over the last
few pixels (0.55 at 4 px, 0.17 at 1 px, 0 at the edge) — a light, casual soften,
steep not creamy, with AA preserved at the boundary. The OFF state (hard 16 px
rounded corner) is untouched and the toggle still switches live in the one page
pipeline.

**Push policy (permanent, from CD-06 on).** Push **per stage, autonomously,
never ask.** The pre-push secret/IP grep stays mandatory before every push; only
the asking stops. If a push is denied by the tool permission system, note it and
continue — do not stall a stage on it. CLAUDE.md carries this rule.

## D-0012 - 2026-07-08 - CD-05: background v2 "Pulse Grid"

The Deep Field (CD-03) is too dark for the Cyber look. Its replacement is the
**Pulse Grid**: a fine circuit-board weave (micro lattice, routed traces with
pads and solder dots, two full-width bus lines) with light pulses travelling the
traces and occasional node flares. It becomes the Cyber default; the Deep Field
is **demoted, not deleted** — it survives intact as the future "Calm" template
variant, selectable via the `background.kind` token (`"pulse_grid"` |
`"deep_field"`).

**Amplitude-spec supersession.** The Deep Field's brightness discipline (the
6-8 % amplitude cap) does NOT apply to the Pulse Grid. This background is allowed
to glow. Content readability is protected by the **zone shadow** (the background
multiplies down toward `zone_shadow` under the surf zone and the open overlay,
with a soft feathered edge) instead of global darkness — glow in the margins,
calm under the page.

**Seed determinism.** The board is generated by a dependency-free splitmix64 PRNG
seeded from `background.seed`. Given the same seed, frame size and DPI scale, the
layout is identical across launches — it feels like YOUR board, not random noise
per boot. The life layer (pulses/flares) runs off the same PRNG but is
deliberately outside the determinism contract (only the static board must match).

**Bake-once architecture.** The static layer (lattice + traces + pads + dots +
bus) is rendered ONCE, at startup and on resize/seed change, into a full-
resolution offscreen texture; each frame composites it as the backmost layer,
scaled by the glow-intensity uniform. The bake is imperceptible (~0.5 ms at
1600x900, ~0.8 ms at 5120x1440). Thin lines stay crisp because the bake is full
res, not half res (the Deep Field's half-res+blit economy was tied to its
per-frame procedural cost; a baked static layer costs zero per frame). Reasoned
deviation: the bake target is **Rgba16Float** (HDR) so glow above 1.0 survives
the up-to-2.2x intensity scaling without banding.

**Settings.** The background toggle is renamed `deep_field` -> `animated_background`
(it now governs whichever background the template selects); the store migrates
the old key's value across. A new **glow-intensity** slider (50-220 %, default
from the `background.glow_default` token) is applied live and persisted; the
`set_setting` IPC now accepts a numeric value for that key (see wire-format).

**Self-test.** `--capture` now renders the full shell (Pulse Grid + ring) with
the ring on its transparent path (`is_srgb = 0`), so the PNG matches the on-
screen framebuffer and the circuit can be eyeballed without screen-scraping the
desktop. `CYBERDESK_CAPTURE_SIZE=WxH` and `CYBERDESK_CAPTURE_GLOW=<mult>` size and
brighten it for headless verification (e.g. the ultrawide target, or the 220 %
readability check).

## D-0011 - 2026-07-08 - CD-04: gesture-aware popup policy

The surf view's `LifeSpanHandler::on_before_popup` always returns `1` — no
separate browser window is ever created. When the popup carries a genuine user
gesture (`user_gesture != 0`) and the source is the surf zone, the target URL is
loaded into the surf view's **own main frame**; popups without a user gesture are
suppressed outright.

**Why the user gesture is the discriminator.** Two earlier extremes both failed:

- CD-01 navigated the surf view on *any* popup. A foreign session's ad/script
  `window.open` then hijacked the view (the foreign-session ad hijack).
- CD-02/03 suppressed *all* popups. That killed legitimate `target=_blank` links
  and click-to-open flows — clicking such a link did nothing.

CEF's `user_gesture` flag cleanly separates the two: a real click that opens a
link is a gesture; an ad/script `window.open` fired from a timer or load handler
is not. So gesture -> navigate in place; no gesture -> drop. Either way no new
window opens, which also preserves the single-surface shell model.

**Scope.** The navigate-on-gesture branch is gated on `Role::Surf`. The internal
views are already navigation-isolated (D-0010) and never spawn popups; the return
value still suppresses any window unconditionally.

## D-0010 - 2026-07-08 - CD-03: internal view uses a `cyberdesk://` custom scheme

The settings view is a second OSR browser locked to a registered custom scheme,
`cyberdesk://settings/`, rather than a reserved web host or a `data:` URL.

**Why a custom scheme.** It gives the internal UI a real, standard, secure
origin (registered via `on_register_custom_schemes` with STANDARD | SECURE |
CORS | FETCH), which is a clean security context for the message-router IPC and
lets isolation be expressed as a simple scheme check. A `data:` URL has an opaque
origin and awkward sub-resource semantics; a reserved web host would blur the
web/internal boundary we are trying to make absolute.

**Served entirely in-process.** A `SchemeHandlerFactory` + `ResourceHandler`
serve the settings document straight from embedded bytes (HTML with the theme
tokens, CSS, and JS inlined — a single document, zero sub-resource requests).
Nothing touches the network.

**Hard web isolation (D-0004).** The internal view's `RequestHandler::
on_before_browse` cancels any navigation whose URL is not `cyberdesk://`. Verified
with an opt-in self-test (`CYBERDESK_ISOLATION_SELFTEST=1`) that steers the view
at `https://example.com/` and confirms the block fires and the view stays put.

**IPC only on the internal view.** `window.cefQuery` is registered by the
renderer-side message router only for `cyberdesk://` V8 contexts, and only the
internal client forwards router messages browser-side. The surf zone never sees
the bridge. Wire format: docs/cyberdesk-wire-format.md (Settings IPC).

## D-0009 - 2026-07-08 - CD-02: accelerated OSR researched, CPU path kept for now

CD-02 ships CPU off-screen rendering: `RenderHandler::on_paint` delivers BGRA, we
upload it into a wgpu texture and composite it. This records the research into the
accelerated (zero-copy GPU) path and why we stay on CPU for now.

**GPU-process finding (good news).** The CD-01 release-only GPU sub-process crash
(D-0008c) does NOT occur under OSR. Release OSR runs with a healthy GPU process -
no STATUS_BREAKPOINT, no SwiftShader fallback. Reworking the presentation path
(OSR) resolved it. So both the CPU path (verified) and a future accelerated path
are viable on a working GPU process.

**The accelerated path exists in cef-rs.** Set `shared_texture_enabled` (and
`external_begin_frame_enabled`) in WindowInfo, handle
`RenderHandler::on_accelerated_paint` (whose `AcceleratedPaintInfo.shared_texture_handle`
is a D3D11 shared HANDLE), and `cef::osr_texture_import::SharedTextureHandle::import_texture(&wgpu::Device)`
imports it via the wgpu-hal DX12 escape hatch (`as_hal::<Dx12>`, open the D3D11
handle as a D3D12 resource, `texture_from_raw`, `create_texture_from_hal`) - exactly
the dx12 external-resource path.

**Concrete blocker.** cef-rs's importer (behind the `accelerated_osr` feature) is
built against **wgpu 29**; CyberDesk uses **wgpu 30**. wgpu `Device`/`Texture`
types are version-specific, so cef's `import_texture(&wgpu29::Device)` cannot
consume our wgpu-30 `Device` nor yield a texture usable in our wgpu-30 pipeline,
and enabling the feature would pull in a second, conflicting wgpu.

**Options.** (a) Replicate cef's ~100-line D3D11 importer against wgpu-30's hal
(open the shared HANDLE as a D3D12 resource via the `windows` crate, wrap with
`wgpu::hal::api::Dx12` `texture_from_raw` + `create_texture_from_hal`; enable wgpu's
dx12 hal feature). Keeps wgpu 30. (b) Pin the whole app to wgpu 29 to use cef's
helper directly - rejected, it regresses our stack.

**Decision.** Stay on the CPU path for CD-02. It is verified working (release,
healthy GPU process, sharp DPI text, full mouse/keyboard input, scrolling), and the
readback cost is acceptable at this stage. The accelerated path is well-scoped for a
focused follow-up (option a) once feathering (CD-03) makes per-pixel throughput
matter - a working GPU process under OSR means it will pay off. Documented and
stopped within the CD-02 time-box.

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
