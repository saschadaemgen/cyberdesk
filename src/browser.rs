//! CEF (Chromium Embedded Framework) integration — the CyberDesk views.
//!
//! The off-screen (OSR) browser views live here, distinguished by a [`Role`]:
//!
//!   * [`Role::Slot`]`(i)` — one of the surf columns (CD-09), full web browsing.
//!     A new/empty slot loads the own start page (CD-14, `cyberdesk://start/`);
//!     no Google, no restored websites.
//!   * [`Role::Internal`] — the settings / command-bar page, locked to the
//!     internal `cyberdesk://` scheme (see docs/cyberdesk-decisions.md, D-0010).
//!
//! Both render into CPU buffers (`RenderHandler::on_paint`); the renderer
//! composites each as a texture. CEF runs a multi-threaded message loop, so
//! `on_paint` / cursor callbacks arrive on the CEF UI thread and hand off to the
//! main thread through mutex-protected per-view state.
//!
//! The internal view is hard-isolated from the web: its `RequestHandler` cancels
//! any navigation whose scheme is not `cyberdesk://` (NetGuard principle,
//! D-0004), and the settings document is served entirely in-process from
//! embedded assets — it performs no network requests at all. Native<->page
//! settings traffic goes over the CEF message router (`window.cefQuery`), which
//! is registered ONLY on `cyberdesk://` contexts, so the IPC bridge exists only
//! on the internal view.
//!
//! Sandbox note: the Windows CEF sandbox is still disabled here (`no_sandbox`);
//! see docs/cyberdesk-decisions.md, D-0008, for the tracked deviation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::c_int;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cef::wrapper::message_router::{
    BrowserSideCallback, BrowserSideHandler, BrowserSideRouter, MessageRouterBrowserSide,
    MessageRouterBrowserSideHandlerCallbacks, MessageRouterConfig, MessageRouterRendererSide,
    MessageRouterRendererSideHandlerCallbacks, RendererSideRouter,
};
use cef::*;
use winit::event::MouseButton;
use winit::window::CursorIcon;

use crate::slots::MAX_SLOTS;

/// The internal custom scheme and its document URLs (D-0010). The **start page**
/// (CD-14, D-0025) is the default content of every empty slot — Google is gone.
const SCHEME: &str = "cyberdesk";
const SETTINGS_URL: &str = "cyberdesk://settings/";
const COMMAND_URL: &str = "cyberdesk://command/";
const INFO_URL: &str = "cyberdesk://info/";
const START_URL: &str = "cyberdesk://start/";
const MFZONE_URL: &str = "cyberdesk://mfzone/";
const HUD_URL: &str = "cyberdesk://hud/";

// cef_event_flags_t bits (modifiers for mouse/key events).
const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;

/// Which OSR view a call targets. `Slot(i)` is one of the up-to-[`MAX_SLOTS`]
/// surf columns (CD-09); `Internal` is the single shared overlay view (settings
/// card / command bar / info / start); `MfZone` is the permanent right
/// Multifunctional-zone content view (CD-18, `cyberdesk://mfzone/`); `Hud` is the
/// permanent transparent top-right info strip (CD-30, `cyberdesk://hud/` — clock
/// + live protection fields). `i` is always `< MAX_SLOTS` (the shell clamps the
/// live slot count), so the indices never collide.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Slot(usize),
    Internal,
    MfZone,
    Hud,
}

impl Role {
    fn idx(self) -> usize {
        match self {
            Role::Slot(i) => i,
            Role::Internal => MAX_SLOTS,
            Role::MfZone => MAX_SLOTS + 1,
            Role::Hud => MAX_SLOTS + 2,
        }
    }
}

// --- Per-view shared state (main thread <-> CEF UI thread) ------------------

#[derive(Default)]
struct FrameBuffer {
    data: Vec<u8>, // BGRA, width*height*4
    width: u32,
    height: u32,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct ViewGeom {
    phys_w: u32,
    phys_h: u32,
    scale: f32,
}
impl Default for ViewGeom {
    fn default() -> Self {
        Self { phys_w: 1, phys_h: 1, scale: 1.0 }
    }
}

/// Per-slot navigation state (CD-09). The former global surf-nav singletons are
/// now one of these per slot; the LoadHandler / DisplayHandler write the owning
/// slot's copy, and the top bar reads the *active* slot's (see [`active_slot`]).
#[derive(Default, Clone)]
struct SlotNav {
    url: String,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
}

struct ViewState {
    frame: Mutex<FrameBuffer>,
    geom: Mutex<ViewGeom>,
    browser: Mutex<Option<Browser>>,
    cursor: Mutex<Option<CursorIcon>>,
    /// Navigation state (slots only; the internal view leaves it at default).
    nav: Mutex<SlotNav>,
}
impl ViewState {
    fn new() -> Self {
        Self {
            frame: Mutex::new(FrameBuffer::default()),
            geom: Mutex::new(ViewGeom::default()),
            browser: Mutex::new(None),
            cursor: Mutex::new(None),
            nav: Mutex::new(SlotNav::default()),
        }
    }
}

/// The per-view state array: `MAX_SLOTS` surf slots at indices `0..MAX_SLOTS`,
/// then the internal overlay view at `MAX_SLOTS`, the permanent MF-zone view at
/// `MAX_SLOTS + 1` (CD-18), and the permanent HUD strip at `MAX_SLOTS + 2` (CD-30).
fn views() -> &'static [ViewState; MAX_SLOTS + 3] {
    static V: OnceLock<[ViewState; MAX_SLOTS + 3]> = OnceLock::new();
    V.get_or_init(|| std::array::from_fn(|_| ViewState::new()))
}
fn view(role: Role) -> &'static ViewState {
    &views()[role.idx()]
}

static CONTEXT_READY: AtomicBool = AtomicBool::new(false);

/// The slot the top bar, keyboard input, and the scheme hint act on. The shell
/// sets it whenever the active column changes (click, Ctrl+1..4, Ctrl+Tab, add /
/// close). Always `< MAX_SLOTS`.
static ACTIVE_SLOT: AtomicUsize = AtomicUsize::new(0);

/// A navigation targeted at a lazy slot that has no browser yet: the CEF-UI-side
/// `navigate` handler records `(slot, url)` here and the shell's main thread
/// spawns the browser (it owns the parent HWND). Consumed via
/// [`take_pending_spawn`].
fn pending_spawn() -> &'static Mutex<Option<(usize, String)>> {
    static P: OnceLock<Mutex<Option<(usize, String)>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Browser-side message router (settings IPC). Created on the UI thread in
/// `on_context_initialized`; read from the client/request/life-span handlers.
static BROWSER_ROUTER: OnceLock<Arc<BrowserSideRouter>> = OnceLock::new();

// --- Per-slot navigation state (CEF UI thread -> main thread) ---------------
// The LoadHandler / DisplayHandler callbacks fire on the CEF UI thread and write
// the owning slot's SlotNav (in its ViewState); the main thread reads it for the
// per-slot loading lines and the window title, and the IPC reads the *active*
// slot's for get_nav_state.

/// The active slot the top bar / keyboard / scheme hint target.
pub fn active_slot() -> usize {
    ACTIVE_SLOT.load(Ordering::Relaxed).min(MAX_SLOTS - 1)
}
/// Set the active slot (called by the shell when the active column changes).
pub fn set_active_slot(i: usize) {
    ACTIVE_SLOT.store(i.min(MAX_SLOTS - 1), Ordering::Relaxed);
}

/// Does slot `i` have a live browser instance yet? (Lazy slots have none until
/// their first navigation.)
pub fn slot_has_browser(i: usize) -> bool {
    i < MAX_SLOTS && view(Role::Slot(i)).browser.lock().unwrap().is_some()
}

/// Is slot `i` currently loading a page? Drives its loading line.
pub fn slot_loading(i: usize) -> bool {
    view(Role::Slot(i)).nav.lock().unwrap().loading
}
/// Slot `i`'s current page title (empty if none / unloaded).
pub fn slot_title(i: usize) -> String {
    view(Role::Slot(i)).nav.lock().unwrap().title.clone()
}
/// Slot `i`'s current page URL (empty if none / unloaded).
pub fn slot_url(i: usize) -> String {
    view(Role::Slot(i)).nav.lock().unwrap().url.clone()
}
fn slot_can_back(i: usize) -> bool {
    view(Role::Slot(i)).nav.lock().unwrap().can_back
}
fn slot_can_forward(i: usize) -> bool {
    view(Role::Slot(i)).nav.lock().unwrap().can_forward
}

/// A navigation targeted at slot `i`: load it if its browser exists, or queue a
/// lazy spawn (the shell's main thread creates the browser). Called by the
/// `navigate` IPC (CD-12: the command carries its slot id). `url` is already
/// classified (URL vs search).
pub fn navigate_slot(i: usize, url: &str) {
    if slot_has_browser(i) {
        load_url(Role::Slot(i), url);
    } else {
        *pending_spawn().lock().unwrap() = Some((i, url.to_string()));
    }
}

/// Take a queued lazy-slot spawn `(slot, url)`, if any (main thread).
pub fn take_pending_spawn() -> Option<(usize, String)> {
    pending_spawn().lock().unwrap().take()
}

/// User-gesture popups (a real click on `target=_blank`, or a Ctrl-/middle-click
/// on a link — Chromium routes these through `on_before_popup` as tab
/// dispositions with a gesture) queued by the CEF UI thread for the main thread
/// to open in a new slot beside the source slot (D-0018). Holds
/// `(source_slot_id, target_url)`.
fn pending_new_slot() -> &'static Mutex<Vec<(usize, String)>> {
    static P: OnceLock<Mutex<Vec<(usize, String)>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}

/// Drain the queued new-slot open requests (main thread).
pub fn take_pending_new_slots() -> Vec<(usize, String)> {
    std::mem::take(&mut pending_new_slot().lock().unwrap())
}

/// A favorite-tile drag the page started (CD-12 `drag_start`): the host then owns
/// the drag (ghost + drop zones). Holds `(url, title)`.
fn pending_drag() -> &'static Mutex<Option<(String, String)>> {
    static P: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Take a queued favorite drag `(url, title)`, if any (main thread).
pub fn take_pending_drag() -> Option<(String, String)> {
    pending_drag().lock().unwrap().take()
}

// The command bar's `navigate` sets this on the CEF UI thread; the main thread
// consumes it to close the overlay after a submission.
static CLOSE_OVERLAY: AtomicBool = AtomicBool::new(false);
fn request_overlay_close() {
    CLOSE_OVERLAY.store(true, Ordering::Relaxed);
}
pub fn take_overlay_close() -> bool {
    CLOSE_OVERLAY.swap(false, Ordering::Relaxed)
}

/// Point the internal view at the settings page.
pub fn show_internal_settings() {
    load_url(Role::Internal, SETTINGS_URL);
}
/// Point the internal view at the command bar page.
pub fn show_internal_command() {
    load_url(Role::Internal, COMMAND_URL);
}
/// Point the internal view at the update-awareness info panel (CD-13).
pub fn show_internal_info() {
    load_url(Role::Internal, INFO_URL);
}

/// Toggle the favorite state of the *active* slot's current page (Ctrl+D from a
/// surf slot). Returns the new state; internal/blank URLs are ignored (memory.rs).
pub fn toggle_current_favorite() -> bool {
    let i = active_slot();
    crate::memory::toggle_favorite(&slot_url(i), &slot_title(i))
}

// --- Command band typing state (CD-07, CD-08 → CD-12) -----------------------
/// Reported by the engaged ensemble: the user is actively typing (focused input
/// holding real text). While true, a mouse-out must NOT disengage the band.
static BAR_TYPING: AtomicBool = AtomicBool::new(false);

/// Max suggestions the palette shows (theme token `command.max_results`, read
/// once).
fn command_max_results() -> usize {
    static M: OnceLock<usize> = OnceLock::new();
    *M.get_or_init(|| crate::theme::Theme::load().command.max_results.max(0) as usize)
}

/// Is the user actively typing in the engaged ensemble's capsule (reported by the
/// page)? The hysteresis disengage skips a mouse-out while this holds.
pub fn bar_typing() -> bool {
    BAR_TYPING.load(Ordering::Relaxed)
}

// --- MF-zone wide-terminal state (CD-30 Task A) ------------------------------
/// True while the MF-zone viewer shows its Terminal tab: the zone renders 2× wide
/// and the slot columns reflow narrower (`slots::frame_layout`). Reported by the
/// page over the `mf_tab` IPC; read by the shell's layout every frame.
static MF_WIDE: AtomicBool = AtomicBool::new(false);
/// One-shot "the MF width changed" flag: the main thread re-pushes view geometry
/// (the MF texture doubles / halves) and the frame state when it fires.
static MF_RELAYOUT: AtomicBool = AtomicBool::new(false);

/// Is the MF zone in its 2×-wide Terminal state?
pub fn mf_wide() -> bool {
    MF_WIDE.load(Ordering::Relaxed)
}
/// Drain the MF relayout flag (main thread).
pub fn take_pending_mf_relayout() -> bool {
    MF_RELAYOUT.swap(false, Ordering::Relaxed)
}

/// "Open the settings card" requested by a page (CD-30: the HUD Ampel's
/// "Custom…" points at the per-vector view, which lives in the settings card).
/// Drained by the main thread, which owns the overlay state.
static OPEN_SETTINGS: AtomicBool = AtomicBool::new(false);
pub fn take_pending_open_settings() -> bool {
    OPEN_SETTINGS.swap(false, Ordering::Relaxed)
}

// --- Process / lifecycle ----------------------------------------------------

/// Must be the first thing `main` does. Binds the CEF API version and runs the
/// CEF sub-process logic. Sub-processes (including the renderer, which hosts the
/// message router's renderer side) get our [`CyberApp`] so `cyberdesk://` is a
/// registered scheme and `window.cefQuery` is wired.
pub fn run_subprocess_if_needed() {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let mut app = CyberApp::new();
    let code = execute_process(Some(args.as_main_args()), Some(&mut app), ptr::null_mut());
    if code >= 0 {
        std::process::exit(code);
    }
}

/// Initialise CEF for the browser process. Multi-threaded message loop,
/// windowless (OSR) rendering enabled, sandbox disabled, isolated profile.
pub fn init_cef() {
    let mut app = CyberApp::new();

    let cache_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("cyberdesk-cache")))
        .map(|p| CefString::from(p.to_string_lossy().as_ref()))
        .unwrap_or_default();

    let settings = Settings {
        no_sandbox: 1,
        multi_threaded_message_loop: 1,
        windowless_rendering_enabled: 1,
        root_cache_path: cache_path,
        ..Default::default()
    };

    let args = args::Args::new();
    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        ptr::null_mut(),
    );
    assert_eq!(ok, 1, "CefInitialize failed");
}

pub fn shutdown_cef() {
    cef::shutdown();
}

pub fn context_ready() -> bool {
    CONTEXT_READY.load(Ordering::Relaxed)
}

/// Set a view's size (device pixels) and DPI scale. Call before creating the
/// browser and on every resize.
pub fn set_view_geometry(role: Role, phys_w: u32, phys_h: u32, scale: f32) {
    *view(role).geom.lock().unwrap() = ViewGeom { phys_w, phys_h, scale };
}

/// Create a windowless (OSR) browser for `role` at its default page: the own
/// **start page** for a slot (CD-14, no Google), the settings page for the
/// internal view.
pub fn create_browser(role: Role, parent_hwnd: isize) {
    let url = match role {
        Role::Slot(_) => START_URL,
        Role::Internal => SETTINGS_URL,
        Role::MfZone => MFZONE_URL,
        Role::Hud => HUD_URL,
    };
    create_browser_url(role, parent_hwnd, url);
}

/// Create a windowless (OSR) browser for `role`, loading `url` immediately. Used
/// both for the eager slot 0 / internal view and for lazy slots on their first
/// navigation. `parent_hwnd` is used only for monitor / DPI info — no child
/// window. The view geometry must be set (see [`set_view_geometry`]) first.
pub fn create_browser_url(role: Role, parent_hwnd: isize, url: &str) {
    // A Tor slot's browser must be created under its OWN proxied request context,
    // and `set_preference` is UI-thread-only (MTML) — so post the whole creation to
    // the CEF UI thread (CD-15 Stage B). Clearnet slots / the internal view use the
    // direct global context on the current thread, unchanged.
    // Capture the slot generation now (CD-25): for a respawn, the preceding
    // close_slot has already bumped it, so this create is stamped with the current
    // generation and a later close/respawn will supersede it.
    let generation = match role {
        Role::Slot(i) => slot_gen(i),
        Role::Internal | Role::MfZone | Role::Hud => 0,
    };
    if let Role::Slot(i) = role
        && slot_is_tor(i)
    {
        let mut task = TorSpawnTask::new(i, url.to_string(), parent_hwnd, generation);
        post_task(ThreadId::UI, Some(&mut task));
        return;
    }

    let window_info =
        WindowInfo::default().set_as_windowless(sys::HWND(parent_hwnd as *mut sys::HWND__));

    // This path is clearnet (the global/direct context) — the Tor path is
    // spawn_tor_browser. Tag the browser clearnet so on_after_created can reject it
    // if the slot has since been toggled to Tor.
    let mut client = CyberClient::new(role, false, generation);
    let url = CefString::from(url);
    let background_color = match role {
        // Slot: opaque white backing (the page paints its own background).
        Role::Slot(_) => 0xFFFF_FFFFu32,
        // Internal: TRANSPARENT backing (CD-12, D-0021) — the command band paints
        // only its floating elements over the Pulse Grid; the settings page draws
        // its own opaque panel background in CSS. OSR delivers premultiplied BGRA
        // with alpha, which the compositor blends OVER. The HUD strip (CD-30) is
        // the same floating idiom: transparent, only its capsules paint.
        Role::Internal | Role::Hud => 0x0000_0000u32,
        // MF zone: OPAQUE — it is permanent content filling its rect (CD-18), not a
        // floating overlay; an opaque backing keeps the Pulse Grid from bleeding.
        Role::MfZone => 0xFFFF_FFFFu32,
    };
    let browser_settings = BrowserSettings {
        windowless_frame_rate: 60,
        background_color,
        ..Default::default()
    };

    // CD-25: carry this slot's effective hardening config to its render process via
    // extra_info. Internal / MF-zone / HUD views load cyberdesk:// (never hardened),
    // so only slots get a config dict.
    let mut extra = match role {
        Role::Slot(i) => harden_extra_info(i),
        Role::Internal | Role::MfZone | Role::Hud => None,
    };
    let created = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&browser_settings),
        extra.as_mut(),
        None,
    );
    assert_eq!(created, 1, "CefBrowserHost::CreateBrowser failed");
}

// --- Per-window Tor (CD-15 Stage B, D-0027) ---------------------------------
// Each slot tracks its connection mode. A Tor slot's browser is created under its
// OWN CefRequestContext whose `proxy` pref points at that slot's local SOCKS5 port
// (per-slot circuit, CD-15 Stage A) plus the WebRTC leak prefs — NEVER the global
// context (the classic "proxy changes for all windows" bug). Clearnet slots keep
// the global (direct) context. The toggle tears the browser down and respawns it
// under the other context at the start page (a fresh identity, no state bleed).

static SLOT_TOR: [AtomicBool; MAX_SLOTS] = [const { AtomicBool::new(false) }; MAX_SLOTS];

/// Set slot `i`'s mode (Tor on/off). Read at browser-creation time to pick the
/// request context. Set by the toggle BEFORE the respawn.
pub fn set_slot_tor(i: usize, on: bool) {
    if i < MAX_SLOTS {
        SLOT_TOR[i].store(on, Ordering::Relaxed);
    }
}

/// Is slot `i` in Tor mode?
pub fn slot_is_tor(i: usize) -> bool {
    i < MAX_SLOTS && SLOT_TOR[i].load(Ordering::Relaxed)
}

/// Tor-toggle requests from the page (CEF UI thread), drained by the main thread
/// (which owns the slot lifecycle). Holds the slot ids to flip.
fn pending_tor_toggle() -> &'static Mutex<Vec<usize>> {
    static P: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}

/// Drain queued Tor toggles (main thread).
pub fn take_pending_tor_toggles() -> Vec<usize> {
    std::mem::take(&mut pending_tor_toggle().lock().unwrap())
}

/// Per-window close requests from the page's close icon (CD-18, CEF UI thread),
/// drained by the main thread which owns the slot lifecycle (and enforces
/// last-slot-refuses). Holds the slot ids to close.
fn pending_close() -> &'static Mutex<Vec<usize>> {
    static P: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}

/// Drain queued window closes (main thread).
pub fn take_pending_closes() -> Vec<usize> {
    std::mem::take(&mut pending_close().lock().unwrap())
}

/// An APPLICATION-level quit requested by the MF-zone quit buttons (CD-21, CEF UI
/// thread). `Some(true)` = "Quit & Save" (persist the session first), `Some(false)`
/// = plain "Quit" (default layout next launch). Drained on the main thread, which
/// owns the winit event loop — the IPC handler must never touch it directly. This
/// is distinct from `pending_close` (which closes ONE slot).
fn pending_quit() -> &'static Mutex<Option<bool>> {
    static P: OnceLock<Mutex<Option<bool>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Request an application quit (`save` = persist the session before exiting).
fn request_quit(save: bool) {
    *pending_quit().lock().unwrap() = Some(save);
}

/// Take a pending application-quit request `Some(save)`, if any (main thread).
pub fn take_pending_quit() -> Option<bool> {
    pending_quit().lock().unwrap().take()
}

wrap_task! {
    struct TorSpawnTask {
        slot: usize,
        url: String,
        hwnd: isize,
        // The slot generation captured when the respawn was REQUESTED (CD-25), carried
        // across the deferred UI-thread hop so the Tor browser is stamped with the
        // request-time generation, not a fresher one read after a later close.
        generation: u32,
    }

    impl Task {
        fn execute(&self) {
            spawn_tor_browser(self.slot, &self.url, self.hwnd, self.generation);
        }
    }
}

/// Create slot `slot`'s browser under a Tor request context (runs on the CEF UI
/// thread). FAIL-CLOSED: the browser is bound to a proxied request context and only
/// that context; the proxy is applied on the context before it serves any request
/// (TorContextHandler), and if context creation fails NO browser is created — a
/// "Tor" browser must never fall back to a direct connection and leak the real IP.
///
/// The browser is created IMMEDIATELY (CD-15-HOTFIX Stage B): it does NOT wait on
/// arti's bootstrap. Its first URL is the local `cyberdesk://start` page (no
/// network); real navigation happens long after the context is initialized and the
/// proxy applied, and until arti is ready a real fetch simply cannot complete
/// (safe — fail-closed), never a direct one.
fn spawn_tor_browser(slot: usize, url: &str, hwnd: isize, generation: u32) {
    let port = crate::tor::socks_port(slot);
    tracing::info!(slot, port, "spawn_tor_browser: begin (CEF UI thread)");
    let Some(mut ctx) = build_tor_context(slot, port) else {
        tracing::error!(slot, "request context creation failed; Tor browser NOT created (fail-closed)");
        return;
    };
    tracing::info!(slot, "tor context created; calling CreateBrowser");
    let window_info = WindowInfo::default().set_as_windowless(sys::HWND(hwnd as *mut sys::HWND__));
    let mut client = CyberClient::new(Role::Slot(slot), true, generation); // built for Tor
    let url = CefString::from(url);
    let browser_settings = BrowserSettings {
        windowless_frame_rate: 60,
        background_color: 0xFFFF_FFFFu32,
        ..Default::default()
    };
    // CD-25: this slot's effective hardening config rides extra_info (5th arg),
    // independent of the Tor proxy on the request context (6th arg) — they compose.
    let mut extra = harden_extra_info(slot);
    let created = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&browser_settings),
        extra.as_mut(),
        Some(&mut ctx),
    );
    tracing::info!(slot, created, "spawn_tor_browser: CreateBrowser returned");
}

wrap_request_context_handler! {
    struct TorContextHandler {
        slot: usize,
        port: u16,
    }

    impl RequestContextHandler {
        /// Apply the proxy + WebRTC leak prefs the moment the context is ready.
        ///
        /// A freshly created `CefRequestContext` initializes ASYNCHRONOUSLY:
        /// `SetPreference` fails silently (empty error string) until this callback
        /// fires (the CD-15-HOTFIX root cause — the old synchronous set never
        /// applied, so every Tor slot fell to the fail-closed exit and looked
        /// frozen). Setting the prefs here is also exactly on time for fail-closed:
        /// the browser's network requests wait for context initialization, so the
        /// proxy is on the context BEFORE any traffic can leave.
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let Some(ctx) = request_context else {
                tracing::error!(slot = self.slot, "tor context init: no context in callback");
                return;
            };
            let proxy_ok = set_proxy_pref(ctx, self.port);
            // WebRTC forced through the proxy so it can't surface the real IP:
            // `disable_non_proxied_udp` blocks any UDP path that bypasses the proxy.
            // (The legacy `webrtc.multiple_routes_enabled` / `nonproxied_udp_enabled`
            // prefs are unregistered in CEF 149 — this single policy supersedes them.)
            let webrtc_ok = set_pref_string(ctx, "webrtc.ip_handling_policy", "disable_non_proxied_udp");
            // CD-17 (D-0041): the same phone-home preference silencing as the global
            // context, so a Tor slot is de-Googled exactly like clearnet.
            apply_degoogle_context_prefs(ctx);
            tracing::info!(
                slot = self.slot,
                port = self.port,
                proxy_ok,
                webrtc_ok,
                "tor context initialized; prefs applied"
            );
            if !proxy_ok {
                // Must not happen on an initialized context (proxy is a settable
                // pref), but if it ever did the slot would NOT be protected — close
                // it rather than let it reach the network directly (fail-closed).
                tracing::error!(
                    slot = self.slot,
                    "tor proxy pref failed on initialized context; closing slot (fail-closed)"
                );
                close_slot(self.slot);
            }
        }
    }
}

/// Build a Tor request context: a fresh context whose `proxy` + WebRTC leak prefs
/// are applied asynchronously by `TorContextHandler` once CEF finishes
/// initializing it (see the callback above). Returns `None` only if context
/// CREATION itself fails — that synchronous null is the fail-closed gate here; the
/// proxy application is deferred (and self-closes the slot if it ever fails).
///
/// Leak checklist (D-0027): `proxy` = fixed SOCKS5 to the slot's Tor port; QUIC is
/// off globally (App::on_before_command_line_processing).
fn build_tor_context(slot: usize, port: u16) -> Option<RequestContext> {
    let on_ui = currently_on(ThreadId::UI);
    tracing::info!(
        slot,
        port,
        on_ui,
        "build_tor_context: creating request context (proxy applied on init)"
    );
    let settings = RequestContextSettings::default();
    let mut handler = TorContextHandler::new(slot, port);
    match request_context_create_context(Some(&settings), Some(&mut handler)) {
        Some(ctx) => {
            // CD-21 Task A: a per-slot Tor context is a PRIVATE `CefRequestContext`
            // and does NOT inherit the global scheme-handler-factory (registered on
            // the global context in `on_context_initialized`). Without this, the
            // slot's very first page — `cyberdesk://start/` — returns
            // ERR_UNKNOWN_URL_SCHEME in a Tor slot (the "no usable start page in the
            // Tor window" bug). Register the same in-process factory on THIS context
            // so the own start page renders with ZERO network egress, before/without
            // arti being bootstrapped. Fail-closed still holds: the page is served
            // in-process, so nothing leaves the machine.
            let mut factory = InternalSchemeFactory::new();
            ctx.register_scheme_handler_factory(
                Some(&CefString::from(SCHEME)),
                Some(&CefString::from("")),
                Some(&mut factory),
            );
            tracing::debug!(
                slot,
                "tor request context created; internal scheme factory registered; prefs apply on init"
            );
            Some(ctx)
        }
        None => {
            tracing::error!(slot, "request_context_create_context returned None");
            None
        }
    }
}

/// Apply one preference on `ctx`, passing a REAL (non-null) error out-param.
///
/// CD-15-HOTFIX ROOT CAUSE: CEF's `SetPreference` returns false (0) when handed a
/// NULL `error` pointer, and `CefString::default()` is `Borrowed(None)` which
/// marshals to null — so EVERY pref set silently failed. The proxy never applied,
/// so the fail-closed guard destroyed the Tor browser: that is the "front-end
/// frozen, no browser opens" symptom Sascha saw. A `BorrowedMut` over a stack
/// `cef_string_t` gives CEF a place to write, and the set succeeds; on genuine
/// failure the message is now captured and logged instead of lost.
fn apply_pref(ctx: &RequestContext, key: &str, val: &mut Value) -> bool {
    let mut raw: sys::_cef_string_utf16_t = unsafe { std::mem::zeroed() };
    let mut err = CefString::from(&mut raw as *mut sys::_cef_string_utf16_t);
    let ok = ctx.set_preference(Some(&CefString::from(key)), Some(val), Some(&mut err)) == 1;
    if !ok {
        tracing::error!(key, error = %err.to_string(), "set_preference failed");
    }
    ok
}

/// Set the `proxy` preference to a fixed SOCKS5 server on the slot's loopback port.
fn set_proxy_pref(ctx: &RequestContext, port: u16) -> bool {
    let Some(mut dict) = dictionary_value_create() else {
        return false;
    };
    dict.set_string(Some(&CefString::from("mode")), Some(&CefString::from("fixed_servers")));
    let server = format!("socks5://127.0.0.1:{port}");
    dict.set_string(Some(&CefString::from("server")), Some(&CefString::from(server.as_str())));
    let Some(mut val) = value_create() else {
        return false;
    };
    val.set_dictionary(Some(&mut dict));
    apply_pref(ctx, "proxy", &mut val)
}

fn set_pref_string(ctx: &RequestContext, key: &str, value: &str) -> bool {
    let Some(mut val) = value_create() else {
        return false;
    };
    val.set_string(Some(&CefString::from(value)));
    apply_pref(ctx, key, &mut val)
}

// --- CD-17 (D-0041): de-Google preference application -----------------------
// The verified NAME/VALUE table lives in `crate::degoogle`; the CEF calls live
// here, reusing the proven `apply_pref` error-pointer idiom (the CD-15-HOTFIX
// root cause was a null error out-param that made every set silently fail).

/// Build a `CefValue` for a [`degoogle::PrefValue`].
fn degoogle_value(v: crate::degoogle::PrefValue) -> Option<Value> {
    use crate::degoogle::PrefValue;
    let val = value_create()?;
    match v {
        PrefValue::Bool(b) => val.set_bool(c_int::from(b)),
        PrefValue::Int(n) => val.set_int(n as c_int),
        PrefValue::Str(s) => val.set_string(Some(&CefString::from(s))),
    };
    Some(val)
}

/// Log the de-Google enforcement manifest once (CD-17 audit aid): the switches
/// and, per preference, its Chromium source and the phone-home traffic it closes.
/// Lands in the rolling log Sascha reviews alongside the net-log, so the enforced
/// set is self-documenting at debug level (no spam at info).
fn log_degoogle_manifest() {
    let valued: Vec<String> = crate::degoogle::VALUED_SWITCHES
        .iter()
        .map(|s| format!("{}={}", s.name, s.value))
        .collect();
    tracing::info!(
        switches = ?crate::degoogle::SWITCHES,
        valued_switches = ?valued,
        disable_features = ?crate::degoogle::DISABLE_FEATURES,
        "de-Google: process-global kill switches"
    );
    for p in crate::degoogle::CONTEXT_PREFS
        .iter()
        .chain(crate::degoogle::GLOBAL_PREFS)
    {
        tracing::debug!(pref = p.name, source = p.source, closes = p.closes, "de-Google vector");
    }
    for s in crate::degoogle::VALUED_SWITCHES {
        tracing::debug!(switch = s.name, value = s.value, source = s.source, closes = s.closes, "de-Google vector");
    }
}

/// Apply every PROFILE de-Google preference to a request context — the global
/// (clearnet) context and each per-slot Tor context alike. Each set is logged;
/// a name that fails to apply is an error line, never a silent no-op.
fn apply_degoogle_context_prefs(ctx: &RequestContext) {
    let prefs = crate::degoogle::CONTEXT_PREFS;
    let mut applied = 0usize;
    for p in prefs {
        if let Some(mut val) = degoogle_value(p.value)
            && apply_pref(ctx, p.name, &mut val)
        {
            applied += 1;
        }
    }
    tracing::info!(
        applied,
        total = prefs.len(),
        "de-Google context prefs applied"
    );
}

/// Apply the GLOBAL (local-state) de-Google preferences (e.g. secure-DNS mode)
/// through the global preference manager, guarded by `CanSetPreference` — these
/// live in local state, out of reach of per-context `SetPreference`. UI thread.
fn apply_degoogle_global_prefs() {
    let Some(pm) = preference_manager_get_global() else {
        tracing::warn!("global preference manager unavailable; local-state de-Google prefs skipped");
        return;
    };
    for p in crate::degoogle::GLOBAL_PREFS {
        let name = CefString::from(p.name);
        if pm.can_set_preference(Some(&name)) != 1 {
            // Command-line/policy-controlled, or unregistered in this build.
            tracing::warn!(pref = p.name, "global pref not settable here — skipped (documented)");
            continue;
        }
        let Some(mut val) = degoogle_value(p.value) else {
            continue;
        };
        let mut raw: sys::_cef_string_utf16_t = unsafe { std::mem::zeroed() };
        let mut err = CefString::from(&mut raw as *mut sys::_cef_string_utf16_t);
        if pm.set_preference(Some(&name), Some(&mut val), Some(&mut err)) == 1 {
            tracing::info!(pref = p.name, "global de-Google pref applied");
        } else {
            tracing::error!(pref = p.name, error = %err.to_string(), "global de-Google pref set failed");
        }
    }
}

/// Close slot `i`'s browser cleanly (Ctrl+W, or a resize that drops columns).
/// The slot becomes lazy again — browser taken and force-closed, nav state and
/// frame reset — so a later navigation re-spawns it. No-op if it has no browser.
pub fn close_slot(i: usize) {
    if i >= MAX_SLOTS {
        return;
    }
    // Bump the generation (CD-25): any browser whose creation for this slot is still
    // in flight is now stale and closes itself on arrival (see on_after_created).
    SLOT_GEN[i].fetch_add(1, Ordering::Relaxed);
    let browser = view(Role::Slot(i)).browser.lock().unwrap().take();
    if let Some(browser) = browser
        && let Some(host) = browser.host()
    {
        // force_close = 1: shut down without the before-unload prompt.
        host.close_browser(1);
    }
    *view(Role::Slot(i)).nav.lock().unwrap() = SlotNav::default();
    *view(Role::Slot(i)).frame.lock().unwrap() = FrameBuffer::default();
}

/// The current command-band frame state JSON (CD-12), so the page can pull it on
/// load (`get_frame`) in addition to the host's on-change push.
fn frame_state() -> &'static Mutex<String> {
    static F: OnceLock<Mutex<String>> = OnceLock::new();
    F.get_or_init(|| Mutex::new("{}".to_string()))
}

/// The frame state a page pulling `get_frame` receives, with the LIVE Tor engine
/// status re-stamped (CD-23). The engine reaches READY on a background thread, so the
/// cached payload's `tor_status` could be stale; re-stamping it here means any
/// (re)created / (re)subscribing consumer (a reloaded command band, a new ensemble)
/// gets the CURRENT state on demand — never a latched "connecting". Falls back to the
/// raw cache if it is not a JSON object.
fn current_frame_state() -> String {
    restamp_tor_status(&frame_state().lock().unwrap(), crate::tor::status())
}

/// Re-stamp `tor_status` in a frame-state JSON object with `status` (pure, unit-tested).
/// Falls back to the input unchanged if it is not a JSON object (e.g. the "{}" seed or
/// malformed cache), so an odd cache can never wedge the pull.
fn restamp_tor_status(cached: &str, status: u8) -> String {
    match serde_json::from_str::<serde_json::Value>(cached) {
        Ok(mut v) if v.is_object() => {
            v["tor_status"] = serde_json::json!(status);
            v.to_string()
        }
        _ => cached.to_string(),
    }
}

/// Store the frame state and push it to the command band page: calls
/// `window.cdFrame(json)` on the internal view (`json` = {slots, engaged,
/// autofocus}, embedded as a JS string literal). Pushed on change (not per frame)
/// — the page glides its ensembles via CSS transitions (CD-11 cadence).
pub fn set_frame_state(json: &str) {
    *frame_state().lock().unwrap() = json.to_string();
    let browser = view(Role::Internal).browser.lock().unwrap().clone();
    if let Some(browser) = browser
        && let Some(frame) = browser.main_frame()
    {
        let escaped = json.replace('\\', "\\\\").replace('\'', "\\'");
        let code = format!("window.cdFrame&&window.cdFrame('{escaped}')");
        frame.execute_java_script(Some(&CefString::from(code.as_str())), None, 0);
    }
}

/// The current HUD state JSON (CD-30 Task B), so the HUD page can pull it on load
/// (`get_hud_state`) in addition to the host's on-change push.
fn hud_state() -> &'static Mutex<String> {
    static F: OnceLock<Mutex<String>> = OnceLock::new();
    F.get_or_init(|| Mutex::new("{}".to_string()))
}

/// Store the HUD state and push it to the HUD page (`window.cdHud(json)`), the
/// same on-change cadence as [`set_frame_state`]. Every field is a REAL live
/// value (rule 0.1 of CD-30: the display never claims a state that isn't active);
/// the page only ticks the clock and the countdown locally between pushes.
pub fn set_hud_state(json: &str) {
    *hud_state().lock().unwrap() = json.to_string();
    let browser = view(Role::Hud).browser.lock().unwrap().clone();
    if let Some(browser) = browser
        && let Some(frame) = browser.main_frame()
    {
        let escaped = json.replace('\\', "\\\\").replace('\'', "\\'");
        let code = format!("window.cdHud&&window.cdHud('{escaped}')");
        frame.execute_java_script(Some(&CefString::from(code.as_str())), None, 0);
    }
}

/// Navigate a view (used by the isolation self-test). The internal view's
/// RequestHandler will refuse anything that is not `cyberdesk://`.
pub fn load_url(role: Role, url: &str) {
    let browser = view(role).browser.lock().unwrap().clone();
    if let Some(browser) = browser
        && let Some(frame) = browser.main_frame()
    {
        frame.load_url(Some(&CefString::from(url)));
    }
}

// --- Fingerprinting hardening (CD-16, D-0039) -------------------------------
// Coherent, per-session TRACKING-RESISTANCE (not anonymity, no OS/UA spoofing —
// EC-01). A fresh random seed per browser LAUNCH keys deterministic readback
// farbling (canvas/WebGL/audio/rects) injected at document-start into every WEB
// frame, so a site cannot link one session to the next while everything stays
// stable within a session. The seed is generated once in the browser process and
// handed to every child process via a command-line switch, so all render
// processes derive identical per-origin noise. The injected script (hardening.js)
// is the sole mechanism: Chromium exposes no stable pref for these vectors, so —
// like Brave, which patches Blink/C++ — we patch the JS surface an embedder owns.

/// Command-line switch carrying the per-session hardening seed to child processes.
const FP_SEED_SWITCH: &str = "cyberdesk-fp-seed";

/// Lowercase-hex of `buf` (no `hex` crate; runs once per process).
fn hex_of(buf: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(buf.len() * 2);
    for b in buf {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A fresh 16-byte random seed, hex-encoded. The farble basis: a new seed ⇒ a
/// different, unlinkable fingerprint across every farbled vector.
fn fresh_seed_hex() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_err() {
        // Practically unreachable on Windows; a non-zero fallback keeps the farbling
        // from silently collapsing to a fixed all-zero seed.
        let pid = std::process::id().to_le_bytes();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            .to_le_bytes();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = pid[i % 4] ^ nanos[i % 4] ^ (i as u8).wrapping_mul(31).wrapping_add(0x9e);
        }
    }
    hex_of(&buf)
}

// --- CD-29: per-session identity ROTATION (D-0046) --------------------------
// The farble seed is the IDENTITY. CD-16 fixed it once per launch; CD-29 makes it
// ROTATABLE so a user can produce a fresh, unlinkable fingerprint on demand:
//   * ON RESTART  — a fresh global seed each launch (default; the persisted-seed
//     path keeps a stable identity across launches when the user turns it off);
//   * MANUAL      — a per-window "new identity now" re-rolls that slot's seed and
//     respawns it (the "burn it now" cross-session-linkage killer);
//   * AUTOMATIC   — a global re-roll every N minutes (the Pulse Grid countdown
//     showpiece), re-seeding every window.
// The EFFECTIVE per-slot seed (override else global) rides the CreateBrowser
// `extra_info` dict alongside the hardening config, so a respawn applies it. The
// argv switch below stays as a fallback for any render process created without a
// per-slot seed (never a hardened slot in practice).

/// The GLOBAL identity seed (browser process). Initialized by [`init_identity_seed`]
/// at startup; re-rolled by [`rotate_global_identity`].
fn identity_seed() -> &'static Mutex<String> {
    static S: OnceLock<Mutex<String>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(fresh_seed_hex()))
}

/// Per-slot identity-seed OVERRIDE (`None` = follow the global). Set by a manual
/// per-window rotation; cleared by a global rotation and on slot reuse.
static SLOT_SEED: [Mutex<Option<String>>; MAX_SLOTS] = [const { Mutex::new(None) }; MAX_SLOTS];

/// When the GLOBAL identity was minted, as unix epoch ms (CD-30 Task B: the honest
/// "identity age" field). Set at startup and on every global rotation; persisted
/// alongside the seed so a stable cross-launch identity reports its REAL age, not
/// the process uptime.
static IDENTITY_BORN_MS: AtomicU64 = AtomicU64::new(0);
/// Bumped on every GLOBAL identity rotation (CD-30): part of the HUD push
/// signature, so the countdown/age display re-syncs exactly when a re-roll lands.
static ROTATION_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Milliseconds since the unix epoch (never panics; clamps to 0 pre-epoch).
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How long the current GLOBAL identity has existed, in ms.
pub fn identity_age_ms() -> u64 {
    now_unix_ms().saturating_sub(IDENTITY_BORN_MS.load(Ordering::Relaxed))
}
/// The global-rotation epoch counter (see [`ROTATION_EPOCH`]).
pub fn rotation_epoch() -> u64 {
    ROTATION_EPOCH.load(Ordering::Relaxed)
}

/// Initialize the global identity seed at startup (browser process, after settings
/// load). Fresh each launch when "new identity on restart" is on (the default), else
/// the persisted seed (a stable cross-launch identity) — minting + storing one the
/// first time. The seed's mint time is tracked (and persisted with a persisted
/// seed), so the HUD's "identity age" is real in both modes.
pub fn init_identity_seed() {
    let (seed, born) = if crate::settings::rotate_on_restart() {
        (fresh_seed_hex(), now_unix_ms())
    } else {
        match crate::settings::persisted_identity_seed() {
            Some(s) if !s.is_empty() => {
                // A persisted seed's age spans launches: prefer its stored mint
                // time; a pre-CD-30 store has none, so stamp (and persist) now —
                // an age UNDER-statement once, never an overstatement.
                let born = crate::settings::persisted_identity_born().unwrap_or_else(|| {
                    let b = now_unix_ms();
                    crate::settings::store_identity_born(b);
                    b
                });
                (s, born)
            }
            _ => {
                let s = fresh_seed_hex();
                let b = now_unix_ms();
                crate::settings::store_identity_seed(&s);
                crate::settings::store_identity_born(b);
                (s, b)
            }
        }
    };
    *identity_seed().lock().unwrap() = seed;
    IDENTITY_BORN_MS.store(born, Ordering::Relaxed);
}

/// A snapshot of the current global identity seed (for the argv fallback switch).
fn identity_seed_snapshot() -> String {
    identity_seed().lock().unwrap().clone()
}

/// The EFFECTIVE identity seed for slot `i` (its override, else the global). Carried
/// to the render process via `extra_info` so a respawn adopts it.
fn slot_effective_seed(i: usize) -> String {
    if i < MAX_SLOTS
        && let Some(s) = SLOT_SEED[i].lock().unwrap().clone()
    {
        return s;
    }
    identity_seed_snapshot()
}

/// MANUAL rotation: re-roll slot `i`'s identity (its own fresh seed). The caller
/// respawns the slot so the fresh document injects under the new seed. If the
/// "also new Tor circuit" setting is on, rotate the slot's circuit too.
pub fn rotate_slot_identity(i: usize) {
    if i >= MAX_SLOTS {
        return;
    }
    *SLOT_SEED[i].lock().unwrap() = Some(fresh_seed_hex());
    if crate::settings::rotate_new_circuit() && slot_is_tor(i) {
        crate::tor::new_identity();
    }
    tracing::info!(slot = i, "identity: manual per-window rotation");
}

/// GLOBAL rotation: re-roll the global identity and clear every per-slot override,
/// so every window adopts the fresh identity. The caller respawns all slots. If the
/// "also new Tor circuit" setting is on, rotate circuits too. Used by the automatic
/// timer (the countdown showpiece).
pub fn rotate_global_identity() {
    *identity_seed().lock().unwrap() = fresh_seed_hex();
    for s in SLOT_SEED.iter() {
        *s.lock().unwrap() = None;
    }
    let born = now_unix_ms();
    IDENTITY_BORN_MS.store(born, Ordering::Relaxed);
    ROTATION_EPOCH.fetch_add(1, Ordering::Relaxed);
    // A stable cross-launch identity that gets rotated is a NEW identity — keep
    // the persisted seed + mint time in step so the next launch restores it.
    if !crate::settings::rotate_on_restart() {
        crate::settings::store_identity_seed(&identity_seed_snapshot());
        crate::settings::store_identity_born(born);
    }
    if crate::settings::rotate_new_circuit() {
        crate::tor::new_identity();
    }
    tracing::info!("identity: global rotation (all windows re-seeded)");
}

/// Clear slot `i`'s identity-seed override (on slot reuse — a fresh window follows
/// the global identity, never a closed window's rotated seed).
pub fn clear_slot_identity(i: usize) {
    if i < MAX_SLOTS {
        *SLOT_SEED[i].lock().unwrap() = None;
    }
}

/// Persist the CURRENT global identity seed (called when the user turns OFF "new
/// identity on restart", so the very identity they are using now is the one restored
/// next launch — not a fresh one). Its mint time rides along so the restored
/// identity reports its real age (CD-30).
pub fn persist_current_identity() {
    crate::settings::store_identity_seed(&identity_seed_snapshot());
    crate::settings::store_identity_born(IDENTITY_BORN_MS.load(Ordering::Relaxed));
}

/// The hardening seed as seen by a RENDER process: read from the command-line switch
/// the browser appended (parsed from argv directly, so it does not depend on any CEF
/// callback ordering). This is the FALLBACK only — a hardened slot's authoritative
/// seed rides `extra_info` (`cd_seed`). Falls back to a fresh per-process random seed
/// if the switch is somehow absent.
fn render_seed() -> &'static str {
    static SEED: OnceLock<String> = OnceLock::new();
    SEED.get_or_init(|| {
        let prefix = format!("--{FP_SEED_SWITCH}=");
        if let Some(v) = std::env::args().find_map(|a| a.strip_prefix(&prefix).map(str::to_string))
            && !v.is_empty()
        {
            return v;
        }
        fresh_seed_hex()
    })
}

/// The document-start injection payload for a given per-window EFFECTIVE config
/// (CD-25) and identity seed (CD-29): the embedded hardening script with the seed AND
/// the config JSON substituted. Rebuilt per injection (both vary per window / per
/// rotation) — cheap next to a navigation.
fn hardening_payload(config_json: &str, seed: &str) -> String {
    include_str!("hardening.js")
        .replace("__CYBERDESK_FP_SEED__", seed)
        .replace("__CYBERDESK_FP_CONFIG__", config_json)
}

/// Whether a frame with this URL is a WEB frame that must be hardened. Our own
/// `cyberdesk://` UI and the browser-internal schemes are left untouched (farbling
/// them is pointless and could break the internal views). Whether hardening is
/// actually applied (or skipped for an Off config) is decided separately, per
/// browser, in the render handler.
fn should_harden(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    const SKIP: [&str; 5] = [
        "cyberdesk://",
        "devtools://",
        "chrome://",
        "chrome-devtools://",
        "chrome-extension://",
    ];
    !SKIP.iter().any(|p| url.starts_with(p))
}

/// Run the hardening script in `frame`'s freshly-created V8 context under `config`
/// and identity `seed`. Called from the render-side `on_context_created`, which fires
/// before any page script, so our patches are in place first.
fn inject_hardening(frame: &mut Frame, config_json: &str, seed: &str) {
    frame.execute_java_script(
        Some(&CefString::from(hardening_payload(config_json, seed).as_str())),
        None,
        0,
    );
}

// --- CD-25 / CD-29: per-window hardening override (D-0040 / D-0045) ----------
// The GLOBAL preset lives in settings.rs; each slot may hold a per-window OVERRIDE
// (session-ephemeral — not persisted). Since CD-29 the override can be a full
// per-vector CUSTOM config, not just a preset, so every vector is settable
// per-window as well as globally (Task C). The EFFECTIVE config (override else
// global) is resolved at browser-CREATE time and carried to the render process via
// the CreateBrowser `extra_info` dictionary — per-window, alongside the
// session-global seed switch. A level change RESPAWNS the slot's browser so the
// fresh document injects under the new config: a live context can't be re-hardened
// (the patches are irreversible, applied before page scripts). The dict rides
// `extra_info` (5th CreateBrowser arg) and composes cleanly with the Tor proxy.

/// A per-window hardening override: a preset level, plus the per-vector flags used
/// only when `level == Custom`. Session-ephemeral. `None` (no override stored) means
/// "inherit the global preset".
#[derive(Clone, Copy)]
pub struct SlotOverride {
    pub level: crate::harden::Level,
    pub custom: crate::harden::Config,
}

/// Per-slot hardening override (`None` = inherit the global). Read at browser-create
/// time and by the frame push (not per rendered frame), so a Mutex is fine.
static SLOT_HARDENING: [Mutex<Option<SlotOverride>>; MAX_SLOTS] =
    [const { Mutex::new(None) }; MAX_SLOTS];

/// Set slot `i`'s hardening override (`None` = inherit the global preset).
pub fn set_slot_hardening(i: usize, ov: Option<SlotOverride>) {
    if i < MAX_SLOTS {
        *SLOT_HARDENING[i].lock().unwrap() = ov;
    }
}

/// Slot `i`'s hardening override, if any (`None` = inheriting the global preset).
pub fn slot_hardening_override(i: usize) -> Option<SlotOverride> {
    if i >= MAX_SLOTS {
        return None;
    }
    *SLOT_HARDENING[i].lock().unwrap()
}

/// The EFFECTIVE resolved config for slot `i` (its override, else the global preset).
pub fn slot_effective_config(i: usize) -> crate::harden::Config {
    match slot_hardening_override(i) {
        Some(ov) => crate::harden::resolve(ov.level, ov.custom),
        None => crate::settings::hardening_global_config(),
    }
}

/// The EFFECTIVE Ampel level for slot `i` (its override's level, else the global).
/// CD-30: the shell keys the red bunker mode — viewport lock + transition — on
/// this, so the lock can only ever accompany a level that is actually applied.
pub fn slot_effective_level(i: usize) -> crate::harden::Level {
    slot_hardening_override(i)
        .map(|o| o.level)
        .unwrap_or_else(crate::settings::hardening_level)
}

/// Per-slot browser GENERATION (CD-25). Browsers are created ASYNCHRONOUSLY (MTML:
/// `on_after_created` fires later on the CEF UI thread), so within one main-thread
/// pass a `close_slot` cannot tear down a browser whose creation is still in flight.
/// Every `close_slot` bumps the slot's generation; a browser captures the generation
/// at CREATE time and, on arrival, closes itself if the slot has since moved on. This
/// closes the double-respawn orphan (two same-slot respawns in one drain) AND the
/// respawn-then-close ghost (a live page/circuit registered for a closed slot) — the
/// `tor`-mode guard alone missed both because a hardening respawn does not flip Tor.
static SLOT_GEN: [AtomicU32; MAX_SLOTS] = [const { AtomicU32::new(0) }; MAX_SLOTS];

fn slot_gen(i: usize) -> u32 {
    if i < MAX_SLOTS { SLOT_GEN[i].load(Ordering::Relaxed) } else { 0 }
}

/// Build the CreateBrowser `extra_info` dict carrying slot `i`'s effective hardening
/// config to its render process. `None` only if the dict can't be created (the render
/// side then fails safe to Standard — never silently Off).
fn harden_extra_info(i: usize) -> Option<DictionaryValue> {
    let dict = dictionary_value_create()?;
    dict.set_string(
        Some(&CefString::from("cd_harden")),
        Some(&CefString::from(slot_effective_config(i).to_json().as_str())),
    );
    // CD-29: the slot's effective identity seed rides alongside the config, so a
    // respawn after a rotation injects under the new seed.
    dict.set_string(
        Some(&CefString::from("cd_seed")),
        Some(&CefString::from(slot_effective_seed(i).as_str())),
    );
    Some(dict)
}

// --- CD-29: reported screen-size preset, per-window override ----------------
// Web slots report a COMMON, real monitor resolution for `screen.*` (default
// 1920x1080), so every machine looks ordinary. The reported size is never smaller
// than the actual column viewport — `common_screen_for` buckets the real display
// up a common-resolution ladder with the user's preset as a floor, so the VIEWPORT
// (view_rect / innerWidth, left untouched) never contradicts `screen.*` (a
// reported/measured contradiction is itself a fingerprint). A change respawns the
// slot (reuses the hardening respawn path) so the new value takes effect on load.

/// Per-slot screen-preset override: [`SCREEN_INHERIT`] = follow the global preset,
/// else a preset code (0=720p, 1=900p, 2=1080p). Read on the CEF UI thread in
/// `screen_info` and by the respawn path.
const SCREEN_INHERIT: u8 = u8::MAX;
static SLOT_SCREEN: [AtomicU8; MAX_SLOTS] = [const { AtomicU8::new(SCREEN_INHERIT) }; MAX_SLOTS];

/// Set slot `i`'s screen-preset override (`None` = inherit the global preset).
pub fn set_slot_screen(i: usize, code: Option<u8>) {
    if i < MAX_SLOTS {
        SLOT_SCREEN[i].store(code.unwrap_or(SCREEN_INHERIT), Ordering::Relaxed);
    }
}

/// Slot `i`'s screen-preset override code, if any (`None` = inheriting the global).
pub fn slot_screen_override(i: usize) -> Option<u8> {
    if i >= MAX_SLOTS {
        return None;
    }
    match SLOT_SCREEN[i].load(Ordering::Relaxed) {
        SCREEN_INHERIT => None,
        c => Some(c),
    }
}

/// The effective (override else global) screen-preset dimensions for slot `i`.
fn slot_screen_preset_dims(i: usize) -> (u32, u32) {
    match slot_screen_override(i) {
        Some(code) => crate::settings::screen_dims(code),
        None => crate::settings::screen_preset_dims(),
    }
}

/// The effective (override else global) screen-preset CODE for slot `i` (CD-30:
/// the red bunker mode starts its viewport-lock ladder at this preset).
pub fn slot_effective_screen_code(i: usize) -> u8 {
    slot_screen_override(i).unwrap_or_else(crate::settings::screen_preset_code)
}

/// The common-resolution ladder: the small set of real desktop sizes a slot's
/// `screen.*` may report. Anything larger than 1080p is only ever reported when the
/// actual viewport genuinely needs it (a big single-column layout on a large
/// monitor) — bucketed to a common value, never the exact pixel size.
const SCREEN_LADDER: [(u32, u32); 5] =
    [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];

/// The COMMON reported screen size for a viewport of `(vw, vh)` DIP under `preset`.
/// Pure + unit-tested. Reported ≥ max(preset, viewport) on BOTH axes (so it never
/// contradicts what the page measures of its real viewport), snapped UP to the
/// smallest common ladder entry that fits; if the viewport exceeds the whole ladder,
/// the real viewport size is reported (coherent — an unusually large window can't be
/// hidden, only the exact monitor size is withheld).
pub fn common_screen_for(preset: (u32, u32), viewport: (u32, u32)) -> (u32, u32) {
    let need_w = preset.0.max(viewport.0);
    let need_h = preset.1.max(viewport.1);
    for &(w, h) in SCREEN_LADDER.iter() {
        if w >= need_w && h >= need_h {
            return (w, h);
        }
    }
    // Larger than every ladder rung: report the actual viewport (never a decoy).
    (need_w, need_h)
}

/// The reported `screen.*` dimensions for slot `i` given its current viewport DIP.
pub fn slot_screen_dims(i: usize, viewport: (u32, u32)) -> (u32, u32) {
    common_screen_for(slot_screen_preset_dims(i), viewport)
}

/// Per-window hardening changes from the page (CEF UI thread), drained by the main
/// thread which owns the slot lifecycle and performs the respawn.
fn pending_slot_hardening() -> &'static Mutex<Vec<usize>> {
    static P: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}
pub fn take_pending_slot_hardening() -> Vec<usize> {
    std::mem::take(&mut pending_slot_hardening().lock().unwrap())
}

/// Set when the GLOBAL hardening preset changes (CEF UI thread), so the main thread
/// respawns every slot that INHERITS the global (whose effective config changed).
fn pending_global_hardening() -> &'static Mutex<bool> {
    static P: OnceLock<Mutex<bool>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(false))
}
pub fn take_pending_global_hardening() -> bool {
    std::mem::replace(&mut pending_global_hardening().lock().unwrap(), false)
}

/// Per-window screen-preset changes (CD-29), drained by the main thread which
/// respawns the slot so `screen.*` reports the new value on load.
fn pending_slot_screen() -> &'static Mutex<Vec<usize>> {
    static P: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}
pub fn take_pending_slot_screen() -> Vec<usize> {
    std::mem::take(&mut pending_slot_screen().lock().unwrap())
}

/// Set when the GLOBAL screen preset changes (CD-29): the main thread respawns every
/// slot that INHERITS the global screen (independent of the hardening inheritance,
/// which a slot may override separately).
fn pending_global_screen() -> &'static Mutex<bool> {
    static P: OnceLock<Mutex<bool>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(false))
}
pub fn take_pending_global_screen() -> bool {
    std::mem::replace(&mut pending_global_screen().lock().unwrap(), false)
}

// Render-side per-browser hardening config, keyed by Browser::identifier() (the
// stable cross-process id). Populated in on_browser_created from extra_info, read in
// on_context_created, cleared in on_browser_destroyed. Thread-local like the render
// router — every render callback runs on the render main thread.
#[derive(Clone)]
struct RenderHardenCfg {
    inject: bool,
    json: String,
    seed: String,
}
thread_local! {
    static RENDER_HARDEN: RefCell<HashMap<c_int, RenderHardenCfg>> = RefCell::new(HashMap::new());
}
fn render_store_cfg(id: c_int, json: String, seed: String) {
    let inject = crate::harden::Config::from_json(&json).on;
    // An absent per-browser seed falls back to the argv seed (never empty → the
    // farble seed is always present).
    let seed = if seed.is_empty() { render_seed().to_string() } else { seed };
    RENDER_HARDEN.with(|m| m.borrow_mut().insert(id, RenderHardenCfg { inject, json, seed }));
}
/// The render config for browser `id`. Fail-safe: an unknown browser (extra_info was
/// absent / a create path we did not cover) defaults to Standard — always PROTECTED,
/// never silently Off.
fn render_get_cfg(id: c_int) -> RenderHardenCfg {
    RENDER_HARDEN.with(|m| {
        m.borrow().get(&id).cloned().unwrap_or_else(|| RenderHardenCfg {
            inject: true,
            json: crate::harden::STANDARD_JSON.to_string(),
            seed: render_seed().to_string(),
        })
    })
}
fn render_remove_cfg(id: c_int) {
    RENDER_HARDEN.with(|m| {
        m.borrow_mut().remove(&id);
    });
}

// --- Handoffs to the main thread --------------------------------------------

/// If a fresh CEF frame has arrived for `role`, hand its BGRA bytes to `f`.
pub fn with_dirty_frame(role: Role, f: impl FnOnce(&[u8], u32, u32)) {
    let mut fb = view(role).frame.lock().unwrap();
    if fb.dirty {
        f(&fb.data, fb.width, fb.height);
        fb.dirty = false;
    }
}

/// Take a pending cursor icon requested by a view.
pub fn take_cursor(role: Role) -> Option<CursorIcon> {
    view(role).cursor.lock().unwrap().take()
}

// --- Input forwarding (main thread -> BrowserHost) --------------------------

fn with_host(role: Role, f: impl FnOnce(BrowserHost)) {
    let browser = view(role).browser.lock().unwrap().clone();
    if let Some(browser) = browser
        && let Some(host) = browser.host()
    {
        f(host);
    }
}

fn with_browser(role: Role, f: impl FnOnce(&Browser)) {
    let browser = view(role).browser.lock().unwrap().clone();
    if let Some(browser) = browser {
        f(&browser);
    }
}

// Navigation controls (surf view). No-op until the browser exists.
pub fn go_back(role: Role) {
    with_browser(role, |b| b.go_back());
}
pub fn go_forward(role: Role) {
    with_browser(role, |b| b.go_forward());
}
pub fn reload(role: Role) {
    with_browser(role, |b| b.reload());
}
pub fn reload_ignore_cache(role: Role) {
    with_browser(role, |b| b.reload_ignore_cache());
}
// Wired to the command bar's reload/stop glyph in Stage B.
#[allow(dead_code)]
pub fn stop_load(role: Role) {
    with_browser(role, |b| b.stop_load());
}

pub fn notify_resized(role: Role) {
    with_host(role, |host| host.was_resized());
}

/// Give or take keyboard focus from a view's OSR browser.
pub fn set_focus(role: Role, focused: bool) {
    with_host(role, |host| host.set_focus(focused as c_int));
}

pub fn send_mouse_move(role: Role, x: i32, y: i32, modifiers: u32, leave: bool) {
    with_host(role, |host| {
        let ev = MouseEvent { x, y, modifiers };
        host.send_mouse_move_event(Some(&ev), leave as c_int);
    });
}

pub fn send_mouse_button(
    role: Role,
    x: i32,
    y: i32,
    modifiers: u32,
    button: MouseButton,
    down: bool,
    clicks: i32,
) {
    let bt = match button {
        MouseButton::Left => MouseButtonType::LEFT,
        MouseButton::Middle => MouseButtonType::MIDDLE,
        MouseButton::Right => MouseButtonType::RIGHT,
        _ => return,
    };
    with_host(role, |host| {
        let ev = MouseEvent { x, y, modifiers };
        host.send_mouse_click_event(Some(&ev), bt, (!down) as c_int, clicks);
    });
}

pub fn send_mouse_wheel(role: Role, x: i32, y: i32, modifiers: u32, delta_x: i32, delta_y: i32) {
    with_host(role, |host| {
        let ev = MouseEvent { x, y, modifiers };
        host.send_mouse_wheel_event(Some(&ev), delta_x, delta_y);
    });
}

fn key_event(type_: KeyEventType, vk: i32, character: u16, modifiers: u32) -> KeyEvent {
    KeyEvent {
        type_,
        modifiers,
        windows_key_code: vk,
        native_key_code: vk,
        character,
        unmodified_character: character,
        ..Default::default()
    }
}

pub fn send_key_down(role: Role, vk: i32, modifiers: u32) {
    with_host(role, |host| {
        host.send_key_event(Some(&key_event(KeyEventType::RAWKEYDOWN, vk, 0, modifiers)))
    });
}
pub fn send_key_up(role: Role, vk: i32, modifiers: u32) {
    with_host(role, |host| {
        host.send_key_event(Some(&key_event(KeyEventType::KEYUP, vk, 0, modifiers)))
    });
}
pub fn send_char(role: Role, ch: u16, modifiers: u32) {
    // For CHAR events Chromium expects windows_key_code to carry the character.
    with_host(role, |host| {
        host.send_key_event(Some(&key_event(KeyEventType::CHAR, ch as i32, ch, modifiers)))
    });
}

// --- Cursor mapping ---------------------------------------------------------

fn map_cursor(t: CursorType) -> CursorIcon {
    if t == CursorType::HAND {
        CursorIcon::Pointer
    } else if t == CursorType::IBEAM {
        CursorIcon::Text
    } else if t == CursorType::CROSS {
        CursorIcon::Crosshair
    } else if t == CursorType::WAIT {
        CursorIcon::Wait
    } else if t == CursorType::HELP {
        CursorIcon::Help
    } else if t == CursorType::MOVE {
        CursorIcon::Move
    } else if t == CursorType::EASTWESTRESIZE || t == CursorType::COLUMNRESIZE {
        CursorIcon::EwResize
    } else if t == CursorType::NORTHSOUTHRESIZE || t == CursorType::ROWRESIZE {
        CursorIcon::NsResize
    } else if t == CursorType::NORTHEASTSOUTHWESTRESIZE {
        CursorIcon::NeswResize
    } else if t == CursorType::NORTHWESTSOUTHEASTRESIZE {
        CursorIcon::NwseResize
    } else {
        CursorIcon::Default
    }
}

/// Modifier helpers used by the winit input translation in `app`.
pub fn modifier_flags(shift: bool, ctrl: bool, alt: bool) -> u32 {
    let mut m = 0;
    if shift {
        m |= EVENTFLAG_SHIFT_DOWN;
    }
    if ctrl {
        m |= EVENTFLAG_CONTROL_DOWN;
    }
    if alt {
        m |= EVENTFLAG_ALT_DOWN;
    }
    m
}

// --- Settings document + IPC ------------------------------------------------

/// The full settings HTML, built once: theme tokens + CSS + JS inlined into a
/// single self-contained document (no sub-resource requests).
fn settings_document() -> String {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        let theme = crate::theme::Theme::load();
        include_str!("settings.html")
            .replace("/*__TOKENS__*/", &theme.to_css_vars())
            .replace("/*__CSS__*/", include_str!("settings.css"))
            .replace("/*__JS__*/", include_str!("settings.js"))
    })
    .clone()
}

/// The command-bar HTML, built once (same inlining discipline as the settings
/// page — one self-contained document, no sub-resource requests).
fn command_document() -> String {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        let theme = crate::theme::Theme::load();
        include_str!("command.html")
            .replace("/*__TOKENS__*/", &theme.to_css_vars())
            .replace("/*__CSS__*/", include_str!("command.css"))
            .replace("/*__JS__*/", include_str!("command.js"))
    })
    .clone()
}

/// The own start page HTML, built once (same self-contained inlining discipline
/// as the settings / command / info pages). It is the default content of every
/// empty slot (CD-14); zero network — no fonts, images, or remote resources.
fn start_document() -> String {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        let theme = crate::theme::Theme::load();
        include_str!("start.html")
            .replace("/*__TOKENS__*/", &theme.to_css_vars())
            .replace("/*__CSS__*/", include_str!("start.css"))
            .replace("/*__JS__*/", include_str!("start.js"))
    })
    .clone()
}

/// The update-awareness info panel HTML, built once (same self-contained
/// inlining discipline as the settings / command pages).
fn info_document() -> String {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        let theme = crate::theme::Theme::load();
        include_str!("info.html")
            .replace("/*__TOKENS__*/", &theme.to_css_vars())
            .replace("/*__CSS__*/", include_str!("info.css"))
            .replace("/*__JS__*/", include_str!("info.js"))
    })
    .clone()
}

/// The MF-zone tabbed viewer page (CD-18): Tor status + log stream, the full app
/// log, and a reserved Terminal placeholder. Served into the permanent right zone.
fn mfzone_document() -> String {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        let theme = crate::theme::Theme::load();
        include_str!("mfzone.html")
            .replace("/*__TOKENS__*/", &theme.to_css_vars())
            .replace("/*__CSS__*/", include_str!("mfzone.css"))
            .replace("/*__JS__*/", include_str!("mfzone.js"))
    })
    .clone()
}

/// The floating HUD strip page (CD-30 Task B): digital clock + live info fields
/// (protection level, vectors active, active-window route, identity rotation).
/// Served into the permanent transparent top-right view.
fn hud_document() -> String {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        let theme = crate::theme::Theme::load();
        include_str!("hud.html")
            .replace("/*__TOKENS__*/", &theme.to_css_vars())
            .replace("/*__CSS__*/", include_str!("hud.css"))
            .replace("/*__JS__*/", include_str!("hud.js"))
    })
    .clone()
}

/// Scheme of a URL, for the command bar's lock/warn hint.
fn scheme_of(url: &str) -> &'static str {
    if url.starts_with("https://") {
        "https"
    } else if url.starts_with("http://") {
        "http"
    } else {
        "other"
    }
}

/// Percent-encode a search query (application/x-www-form-urlencoded style).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The query→search-URL mapping for one engine (CD-07; the engine allowlist
/// lives in settings.rs). CyberDesk OWNS this routing: both address bars (the
/// command band and the start page) send `navigate` over the message router and
/// the host builds the search URL here — Chromium's omnibox default-search-engine
/// (TemplateURLService) is never consulted, so the selector is authoritative by
/// construction (CD-27, D-0043). Pure and engine-parameterized so the unit tests
/// can pin every engine without touching the live setting.
fn search_url_for(engine: &str, query: &str) -> String {
    let q = urlencode(query);
    match engine {
        "google" => format!("https://www.google.com/search?q={q}"),
        "bing" => format!("https://www.bing.com/search?q={q}"),
        "startpage" => format!("https://www.startpage.com/sp/search?query={q}"),
        "brave" => format!("https://search.brave.com/search?q={q}"),
        // The factory default — and the fallback for any unknown name: never
        // silently Google (CD-27, D-0043).
        _ => format!("https://duckduckgo.com/?q={q}"),
    }
}

/// Host-side URL-vs-search decision for `engine`. A scheme, or a dot without
/// spaces, or localhost is treated as a URL (defaulting to https://); everything
/// else becomes a search on that engine.
fn classify_input_for(engine: &str, input: &str) -> String {
    let t = input.trim();
    if t.is_empty() {
        return "about:blank".to_string();
    }
    if t.contains("://") {
        return t.to_string();
    }
    let is_localhost =
        t == "localhost" || t.starts_with("localhost:") || t.starts_with("localhost/");
    let looks_url = is_localhost || (t.contains('.') && !t.contains(char::is_whitespace));
    if looks_url {
        format!("https://{t}")
    } else {
        search_url_for(engine, t)
    }
}

/// [`classify_input_for`] on the LIVE `search_engine` setting — the `navigate`
/// IPC path shared by the command band and the start page.
fn classify_input(input: &str) -> String {
    classify_input_for(crate::settings::search_engine(), input)
}

/// Slot `i`'s navigation state as JSON, for the `get_nav_state` IPC reply. Since
/// CD-12 each floating ensemble reads and drives its own column, so the command
/// carries the slot id (falling back to the active slot).
fn nav_state_json(i: usize) -> String {
    let url = slot_url(i);
    let scheme = scheme_of(&url);
    serde_json::json!({
        "url": url,
        "title": slot_title(i),
        "can_back": slot_can_back(i),
        "can_forward": slot_can_forward(i),
        "loading": slot_loading(i),
        "scheme": scheme,
        "favorite": crate::memory::is_favorite(&url),
    })
    .to_string()
}

/// The slot a command targets (CD-12): its `slot` field, clamped, else the
/// keyboard-active slot.
fn target_slot(v: &serde_json::Value) -> usize {
    v.get("slot")
        .and_then(|s| s.as_u64())
        .map(|n| (n as usize).min(MAX_SLOTS - 1))
        .unwrap_or_else(active_slot)
}

/// Handle one internal-view query string (see docs/cyberdesk-wire-format.md).
/// Returns the JSON reply on success, or `(error_code, message)` on failure.
fn handle_internal_query(request: &str) -> Result<String, (i32, String)> {
    let v: serde_json::Value =
        serde_json::from_str(request).map_err(|e| (1, format!("bad request json: {e}")))?;
    match v.get("cmd").and_then(|c| c.as_str()).unwrap_or("") {
        // Settings (CD-03).
        "get_settings" => Ok(crate::settings::snapshot_json()),
        "set_setting" => {
            let key = v
                .get("key")
                .and_then(|k| k.as_str())
                .ok_or((2, "missing 'key'".to_string()))?;
            let value = v
                .get("value")
                .ok_or((2, "missing 'value'".to_string()))?;
            // search_engine carries a string, glow_intensity a number (percent);
            // the toggles carry bools.
            if key == crate::settings::KEY_SEARCH_ENGINE {
                let s = value
                    .as_str()
                    .ok_or((2, "'value' must be a string for search_engine".to_string()))?;
                crate::settings::set_search_engine(s).map_err(|e| (3, e))
            } else if key == crate::settings::KEY_SCREEN_PRESET {
                // A global screen-size change respawns every slot that inherits the
                // global screen (CD-29) so `screen.*` reports the new common value.
                let s = value
                    .as_str()
                    .ok_or((2, "'value' must be a string for screen_preset".to_string()))?;
                let reply = crate::settings::set_screen_preset(s).map_err(|e| (3, e))?;
                *pending_global_screen().lock().unwrap() = true;
                Ok(reply)
            } else if key == crate::settings::KEY_GLOW_INTENSITY {
                let n = value
                    .as_i64()
                    .or_else(|| value.as_f64().map(|f| f.round() as i64))
                    .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
                    .ok_or((2, "'value' must be numeric for glow_intensity".to_string()))?;
                crate::settings::set_glow_intensity(n).map_err(|e| (3, e))
            } else if key == crate::settings::KEY_ROTATE_INTERVAL {
                let n = value
                    .as_i64()
                    .or_else(|| value.as_f64().map(|f| f.round() as i64))
                    .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
                    .ok_or((2, "'value' must be numeric for rotate_interval_min".to_string()))?;
                crate::settings::set_rotate_interval(n).map_err(|e| (3, e))
            } else {
                let b = value
                    .as_bool()
                    .ok_or((2, "'value' must be boolean".to_string()))?;
                let reply = crate::settings::set(key, b).map_err(|e| (3, e))?;
                // Turning OFF "new identity on restart" persists the CURRENT identity
                // seed, so the identity in use now is the one restored next launch
                // (CD-29). Turning it back on leaves the stored seed for a launch to
                // ignore in favor of a fresh one.
                if key == crate::settings::KEY_ROTATE_ON_RESTART && !b {
                    persist_current_identity();
                }
                Ok(reply)
            }
        }
        // Command band frame state pull (CD-12): the page loads it on start, then
        // the host pushes updates via set_frame_state.
        "get_frame" => Ok(current_frame_state()),
        // Command / navigation (CD-04; CD-12 carries the ensemble's slot id).
        "get_nav_state" => Ok(nav_state_json(target_slot(&v))),
        "navigate" => {
            let slot = target_slot(&v);
            let input = v
                .get("input")
                .and_then(|x| x.as_str())
                .ok_or((2, "missing 'input'".to_string()))?;
            let url = classify_input(input);
            // Load that slot (spawning it if lazy — see navigate_slot).
            navigate_slot(slot, &url);
            request_overlay_close();
            Ok(serde_json::json!({ "ok": true, "url": url }).to_string())
        }
        "go_back" => {
            go_back(Role::Slot(target_slot(&v)));
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        "go_forward" => {
            go_forward(Role::Slot(target_slot(&v)));
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        "reload" => {
            reload(Role::Slot(target_slot(&v)));
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // Command palette (CD-07): live suggestions from favorites + history.
        // Empty input returns the top favorites (the shared launcher tiles).
        "query_suggestions" => {
            let input = v.get("input").and_then(|x| x.as_str()).unwrap_or("");
            let suggestions = crate::memory::query_suggestions(input, command_max_results());
            let arr: Vec<serde_json::Value> = suggestions
                .iter()
                .map(|s| serde_json::json!({ "url": s.url, "title": s.title, "favorite": s.favorite }))
                .collect();
            Ok(serde_json::Value::Array(arr).to_string())
        }
        // Favorites (CD-07). Toggles the favorite state of an explicit URL — used
        // by the star glyph in the command bar; the surf-view Ctrl+D toggles
        // host-side (see `toggle_current_favorite`).
        "toggle_favorite" => {
            let url = v
                .get("url")
                .and_then(|x| x.as_str())
                .ok_or((2, "missing 'url'".to_string()))?;
            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("");
            let favorite = crate::memory::toggle_favorite(url, title);
            Ok(serde_json::json!({ "favorite": favorite }).to_string())
        }
        // Favorite-tile drag start (CD-12): the page reports a tile drag; the host
        // takes over (ghost + drop zones). Internal view only, allowlisted.
        "drag_start" => {
            let url = v
                .get("url")
                .and_then(|x| x.as_str())
                .ok_or((2, "missing 'url'".to_string()))?;
            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("");
            *pending_drag().lock().unwrap() = Some((url.to_string(), title.to_string()));
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // Bar typing state (CD-08): the page reports whether the user is actively
        // typing so the host's mouse-out hide does not interrupt them.
        "bar_typing" => {
            let active = v.get("active").and_then(|x| x.as_bool()).unwrap_or(false);
            BAR_TYPING.store(active, Ordering::Relaxed);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // Update-awareness info panel (CD-13 → CD-22). Internal view only,
        // allowlisted. Read-only now: the panel derives every component status from
        // the client-side table (no live fetch), so the CD-13 `dismiss_item` /
        // `check_updates` commands were retired in CD-22 along with the manifest feed.
        "get_info_items" => Ok(crate::updates::info_snapshot_json()),
        // Per-window Tor toggle (CD-15 Stage B): flip the ensemble's slot between
        // clearnet and Tor. The main thread respawns the browser under the new
        // context; queued here because it owns the slot lifecycle.
        "toggle_tor" => {
            let slot = target_slot(&v);
            pending_tor_toggle().lock().unwrap().push(slot);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // Per-window close (CD-18): the ensemble's close icon. Queued for the main
        // thread; it enforces last-slot-refuses + neighbor promotion.
        "close_slot" => {
            let slot = target_slot(&v);
            pending_close().lock().unwrap().push(slot);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // GLOBAL hardening preset (CD-25): off/standard/strict, or custom with a
        // `vectors` object of per-vector booleans. `confirm:true` is REQUIRED to
        // weaken (drop a vector / turn off) — the host re-validates the two-
        // confirmation gate rather than trusting the page to have run it. On success
        // every slot that inherits the global respawns under the new config.
        "set_hardening" => {
            let level = v
                .get("level")
                .and_then(|l| l.as_str())
                .ok_or((2, "missing 'level'".to_string()))?;
            let confirm = v.get("confirm").and_then(|c| c.as_bool()).unwrap_or(false);
            // Every vector from the object is applied; absent keys stay ON, so a page
            // that omits a vector never silently weakens it (CD-29).
            let vectors = v.get("vectors").map(crate::harden::Config::from_vectors_value);
            let reply = crate::settings::set_hardening(level, vectors, confirm).map_err(|e| (3, e))?;
            *pending_global_hardening().lock().unwrap() = true;
            Ok(reply)
        }
        // PER-WINDOW reported-screen preset (CD-29): "inherit" or a preset name
        // (1280x720 / 1600x900 / 1920x1080). Queued for the main thread, which
        // respawns the slot so `screen.*` reports the new value on load. No weakening
        // gate: every option is a common real resolution (never a decoy).
        "set_slot_screen" => {
            let slot = target_slot(&v);
            let value = v
                .get("value")
                .and_then(|x| x.as_str())
                .ok_or((2, "missing 'value'".to_string()))?;
            let code = if value == "inherit" {
                None
            } else {
                Some(
                    crate::settings::screen_code(value)
                        .ok_or((2, format!("unknown screen preset: {value}")))?,
                )
            };
            set_slot_screen(slot, code);
            pending_slot_screen().lock().unwrap().push(slot);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // Per-window screen state (CD-29): the slot's effective preset name + whether
        // it inherits the global — for the per-window screen cycler.
        "get_slot_screen" => {
            let slot = target_slot(&v);
            let ov = slot_screen_override(slot);
            let code = ov.unwrap_or_else(crate::settings::screen_preset_code);
            Ok(format!(
                "{{\"value\":\"{}\",\"inherited\":{}}}",
                crate::settings::screen_name(code),
                ov.is_none()
            ))
        }
        // Per-window hardening state (CD-29): the slot's effective level, whether it
        // is inheriting the global, and the full per-vector config — so the per-window
        // Custom detail can paint each vector's current state without bloating the
        // frame push. Read-only.
        "get_slot_hardening" => {
            let slot = target_slot(&v);
            let ov = slot_hardening_override(slot);
            let level = ov
                .map(|o| o.level)
                .unwrap_or_else(crate::settings::hardening_level);
            let eff = slot_effective_config(slot);
            Ok(format!(
                "{{\"level\":\"{}\",\"inherited\":{},\"config\":{}}}",
                level.as_str(),
                ov.is_none(),
                eff.to_json()
            ))
        }
        // PER-WINDOW hardening override (CD-25 / CD-29): inherit/off/standard/strict,
        // OR custom with a per-vector `vectors` object (CD-29 Task C — every vector
        // settable per-window too). Same weakening gate as the global; the host
        // re-validates it. Queued for the main thread, which respawns the slot.
        "set_slot_hardening" => {
            let slot = target_slot(&v);
            let level_str = v
                .get("level")
                .and_then(|l| l.as_str())
                .ok_or((2, "missing 'level'".to_string()))?;
            let confirm = v.get("confirm").and_then(|c| c.as_bool()).unwrap_or(false);
            let ov: Option<SlotOverride> = match level_str {
                "inherit" => None,
                "custom" => {
                    // Vectors from the page, else the slot's existing custom, else
                    // Standard (fail-protected). Absent keys stay ON (from_vectors_value).
                    let custom = v
                        .get("vectors")
                        .map(crate::harden::Config::from_vectors_value)
                        .or_else(|| slot_hardening_override(slot).map(|o| o.custom))
                        .unwrap_or(crate::harden::Config::STANDARD);
                    Some(SlotOverride { level: crate::harden::Level::Custom, custom })
                }
                other => {
                    let l = crate::harden::Level::parse(other)
                        .filter(|l| *l != crate::harden::Level::Custom)
                        .ok_or((2, format!("bad level: {other}")))?;
                    Some(SlotOverride { level: l, custom: crate::harden::Config::STANDARD })
                }
            };
            let current = slot_effective_config(slot);
            let target = match ov {
                Some(o) => crate::harden::resolve(o.level, o.custom),
                None => crate::settings::hardening_global_config(),
            };
            if crate::harden::is_weakening(&current, &target) && !confirm {
                return Err((3, "weakening hardening requires confirmation".to_string()));
            }
            set_slot_hardening(slot, ov);
            pending_slot_hardening().lock().unwrap().push(slot);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // APPLICATION-level quit (CD-21): the two floating MF-zone quit buttons.
        // Distinct from `close_slot` (one window) — these end the whole shell.
        // `quit` = no save (default layout next launch); `quit_save` = persist the
        // full session first, then quit (restored exactly next launch). Queued for
        // the main thread, which owns the winit event loop.
        "quit" => {
            request_quit(false);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        "quit_save" => {
            request_quit(true);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // The Tor engine's bootstrap status + (on failure) the reason, for the
        // settings readout (CD-15 Stage C / HOTFIX). `reason` is empty unless failed.
        "tor_status" => Ok(serde_json::json!({
            "status": crate::tor::status(),
            "reason": crate::tor::fail_reason(),
            // The embedded arti (Tor engine) version, for the settings info (CD-18).
            "version": crate::updates::current_tor_version(),
        })
        .to_string()),
        // "New circuit / new identity" (CD-18): rotate the per-slot isolated clients
        // so new streams ride fresh circuits. A lock-free epoch bump — safe here.
        "tor_new_circuit" => {
            crate::tor::new_identity();
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // Manual per-window "new identity now" (CD-29 Task D): re-roll THIS window's
        // farble seed (and, if enabled, its Tor circuit), then respawn it so the fresh
        // document injects under the new identity — the "burn it now" control. This is
        // the strongest per-window unlinkability lever (a fresh seed AND a reload, so
        // it applies immediately, not just to future loads).
        "new_identity" => {
            let slot = target_slot(&v);
            rotate_slot_identity(slot);
            pending_slot_hardening().lock().unwrap().push(slot);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // The MF-zone viewer's log stream (CD-18): the last ring-buffer lines matching
        // an optional {filter:{target_prefix,level_min}, since_seq}. Pull-based +
        // incremental — the page sends back the highest seq it has seen.
        "get_log_lines" => Ok(crate::logging::log_snapshot_json(&v)),
        // The HUD strip pulls its state once on load (CD-30 Task B) — the same
        // pull-then-push pattern as `get_frame`. The cached payload's countdown
        // fields are elapsed-based, so the page re-anchors them at receive time.
        "get_hud_state" => Ok(hud_state().lock().unwrap().clone()),
        // Open the settings card (CD-30: the HUD Ampel's "Custom…" — the
        // per-vector view lives there). Queued for the main thread.
        "open_settings" => {
            OPEN_SETTINGS.store(true, Ordering::Relaxed);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        // The MF-zone viewer reports its active tab (CD-30 Task A): while the
        // Terminal tab is shown the MF zone renders 2× wide and the slot columns
        // reflow narrower; any other tab returns it to the permanent width. The
        // main thread drains the relayout flag (view geometry is main-thread work).
        "mf_tab" => {
            let tab = v.get("tab").and_then(|t| t.as_str()).unwrap_or("");
            let wide = tab == "term";
            if MF_WIDE.swap(wide, Ordering::Relaxed) != wide {
                MF_RELAYOUT.store(true, Ordering::Relaxed);
            }
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        other => Err((4, format!("unknown cmd: {other}"))),
    }
}

/// Browser-side query handler. Runs on the browser UI thread.
struct SettingsQueryHandler;
impl BrowserSideHandler for SettingsQueryHandler {
    fn on_query_str(
        &self,
        _browser: Option<Browser>,
        _frame: Option<Frame>,
        _query_id: i64,
        request: &str,
        _persistent: bool,
        callback: Arc<Mutex<dyn BrowserSideCallback>>,
    ) -> bool {
        let cb = callback.lock().unwrap();
        match handle_internal_query(request) {
            Ok(json) => cb.success_str(&json),
            Err((code, msg)) => cb.failure(code, &msg),
        }
        true
    }
}

// --- CEF handler implementations --------------------------------------------

wrap_app! {
    pub struct CyberApp;

    impl App {
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if let Some(reg) = registrar {
                // Standard, secure origin so the settings page is a proper
                // security context; fetch/CORS enabled for completeness. Served
                // entirely in-process — no network ever.
                let options = SchemeOptions::STANDARD.get_raw()
                    | SchemeOptions::SECURE.get_raw()
                    | SchemeOptions::CORS_ENABLED.get_raw()
                    | SchemeOptions::FETCH_ENABLED.get_raw();
                reg.add_custom_scheme(Some(&CefString::from(SCHEME)), options);
            }
        }

        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cmd) = command_line else { return };

            // Disable QUIC globally (CD-15, D-0027): QUIC rides UDP and can bypass a
            // SOCKS proxy, leaking a Tor window's real IP. There is no per-context
            // QUIC pref, so it is off everywhere — clearnet still works over TCP
            // (QUIC is only a transport optimization; no site breaks).
            cmd.append_switch(Some(&CefString::from("disable-quic")));

            // CD-17 (D-0041): silence Chromium's phone-home vectors. Boolean,
            // process-global switches — names verified against Chromium
            // 149.0.7827.201 (see src/degoogle.rs). Applied for every process so a
            // feature/behaviour toggle agrees browser<->renderer.
            for sw in crate::degoogle::SWITCHES {
                cmd.append_switch(Some(&CefString::from(*sw)));
            }
            // CD-26 (D-0042): valued switches — signin off + the GAIA origin
            // redirected to a dead loopback, closing the eager ListAccounts
            // vector that no pref/policy/feature gates (see src/degoogle.rs).
            for sw in crate::degoogle::VALUED_SWITCHES {
                cmd.append_switch_with_value(
                    Some(&CefString::from(sw.name)),
                    Some(&CefString::from(sw.value)),
                );
            }
            // Merge our disabled features into any existing --disable-features:
            // base::CommandLine keeps switches in a map, so a bare second
            // --disable-features would CLOBBER whatever CEF already set. Read →
            // merge (idempotent, dedups) → re-set.
            let features_key = CefString::from("disable-features");
            let existing = if cmd.has_switch(Some(&features_key)) == 1 {
                CefString::from(&cmd.switch_value(Some(&features_key))).to_string()
            } else {
                String::new()
            };
            let merged = crate::degoogle::merge_disable_features(&existing);
            if !merged.is_empty() {
                if cmd.has_switch(Some(&features_key)) == 1 {
                    cmd.remove_switch(Some(&features_key));
                }
                cmd.append_switch_with_value(
                    Some(&features_key),
                    Some(&CefString::from(merged.as_str())),
                );
            }

            // Opt-in net-log capture for the de-Google audit (CD-17 §2). Browser
            // process only — the network service writes ONE file. OFF unless the
            // env var names a path, so nothing lands on disk in a normal run
            // (anti-forensic tenet). CEF/Chromium's network service reads
            // --log-net-log=<path> and records every outbound connection.
            let is_browser_process = process_type
                .map(|p| p.to_string())
                .unwrap_or_default()
                .is_empty();
            if is_browser_process
                && let Ok(path) = std::env::var(crate::degoogle::AUDIT_NETLOG_ENV)
                && !path.trim().is_empty()
            {
                cmd.append_switch_with_value(
                    Some(&CefString::from("log-net-log")),
                    Some(&CefString::from(path.as_str())),
                );
                tracing::warn!(
                    %path,
                    "net-log capture ENABLED (audit mode) — outbound connections are being logged to disk"
                );
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(CyberBrowserProcessHandler::new())
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(CyberRenderProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct CyberBrowserProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            // Serve every cyberdesk:// host (settings, command) from the
            // in-process factory (empty domain = all hosts under the scheme).
            let mut factory = InternalSchemeFactory::new();
            register_scheme_handler_factory(
                Some(&CefString::from(SCHEME)),
                Some(&CefString::from("")),
                Some(&mut factory),
            );

            // Browser side of the settings IPC.
            let router =
                <BrowserSideRouter as MessageRouterBrowserSide>::new(MessageRouterConfig::default());
            router.add_handler(Arc::new(SettingsQueryHandler) as Arc<dyn BrowserSideHandler>, false);
            let _ = BROWSER_ROUTER.set(router);

            // CD-17 (D-0041): silence Chromium's phone-home PREFERENCES on the
            // GLOBAL (clearnet) request context. Per-slot Tor contexts get the same
            // set in TorContextHandler::on_request_context_initialized, so clearnet
            // and Tor are de-Googled alike. This callback is on the browser-process
            // UI thread — required for SetPreference.
            log_degoogle_manifest();
            if let Some(ctx) = request_context_get_global_context() {
                apply_degoogle_context_prefs(&ctx);
            } else {
                tracing::error!("global request context unavailable; clearnet de-Google prefs NOT applied");
            }
            // Local-state prefs (secure-DNS mode) live in global preferences, not
            // the profile — set them through the global preference manager.
            apply_degoogle_global_prefs();

            CONTEXT_READY.store(true, Ordering::Relaxed);
        }

        /// Hand the per-session fingerprinting-hardening seed (CD-16, D-0039) to
        /// every child process. Appending it here (browser process, per child
        /// launch) makes it a real argv entry every render process reads back, so
        /// all renderers share ONE seed and derive identical per-origin farbling.
        fn on_before_child_process_launch(&self, command_line: Option<&mut CommandLine>) {
            if let Some(cmd) = command_line {
                // The current global identity seed as a fallback for any render
                // process without a per-slot `cd_seed` (CD-29). The per-slot seed in
                // extra_info is authoritative for hardened slots.
                cmd.append_switch_with_value(
                    Some(&CefString::from(FP_SEED_SWITCH)),
                    Some(&CefString::from(identity_seed_snapshot().as_str())),
                );
            }
        }
    }
}

wrap_client! {
    struct CyberClient {
        role: Role,
        // The connection mode this browser was CREATED for (CD-15). Validated in
        // on_after_created against the slot's CURRENT mode: a rapid re-toggle can
        // leave a browser built under a stale mode racing to register, and
        // installing a clearnet browser on a Tor slot (or vice versa) is a fail-open
        // IP leak — so a mismatched browser is closed instead of installed.
        tor: bool,
        // The slot generation this browser was created for (CD-25). Validated in
        // on_after_created against the slot's CURRENT generation: a respawn or close
        // that raced this creation bumped the generation, so a stale browser closes
        // itself instead of registering (fixes the same-tor double-respawn orphan and
        // the respawn-then-close ghost the tor guard alone missed).
        generation: u32,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(CyberRenderHandler::new(self.role))
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(CyberDisplayHandler::new(self.role))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            // Only surf slots drive their loading line / nav state.
            match self.role {
                Role::Slot(_) => Some(CyberLoadHandler::new(self.role)),
                Role::Internal | Role::MfZone | Role::Hud => None,
            }
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(CyberLifeSpanHandler::new(self.role, self.tor, self.generation))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            // Web isolation (cyberdesk:// only) on the internal views — the shared
            // overlay AND the MF-zone content view (CD-18).
            match self.role {
                Role::Internal | Role::MfZone | Role::Hud => Some(InternalRequestHandler::new()),
                Role::Slot(_) => None,
            }
        }
        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> c_int {
            // Route the message-router query for every view, not just the internal
            // one: the CD-14 start page lives in a SLOT and needs IPC (navigate,
            // query_suggestions). This is safe — `window.cefQuery` is exposed ONLY
            // on `cyberdesk://` frames (the render-side on_context_created gate), so
            // a slot showing a web page has no query bridge; only our own start
            // page (the sole cyberdesk:// content a slot ever shows) can send here.
            if let Some(router) = BROWSER_ROUTER.get()
                && router.on_process_message_received(
                    browser.map(|b| b.clone()),
                    frame.map(|f| f.clone()),
                    source_process,
                    message.map(|m| m.clone()),
                )
            {
                return 1;
            }
            0
        }
    }
}

wrap_render_handler! {
    struct CyberRenderHandler {
        role: Role,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let g = *view(self.role).geom.lock().unwrap();
                let dip_w = (g.phys_w as f32 / g.scale).round().max(1.0) as i32;
                let dip_h = (g.phys_h as f32 / g.scale).round().max(1.0) as i32;
                rect.x = 0;
                rect.y = 0;
                rect.width = dip_w;
                rect.height = dip_h;
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> c_int {
            if let Some(info) = screen_info {
                let g = *view(self.role).geom.lock().unwrap();
                info.device_scale_factor = g.scale;
                let dip_w = (g.phys_w as f32 / g.scale).round().max(1.0) as u32;
                let dip_h = (g.phys_h as f32 / g.scale).round().max(1.0) as u32;
                // CD-29: a web slot reports a COMMON, real monitor resolution for
                // `screen.*` (clamped to the preset, bucketed up if the viewport
                // needs it) so every machine looks ordinary — while the VIEWPORT
                // (view_rect / innerWidth) stays the actual column size (no decoy).
                // The internal / MF-zone cyberdesk:// UI is never fingerprinted and
                // keeps its real geometry.
                let (sw, sh) = match self.role {
                    Role::Slot(i) => slot_screen_dims(i, (dip_w, dip_h)),
                    Role::Internal | Role::MfZone | Role::Hud => (dip_w, dip_h),
                };
                info.rect = Rect { x: 0, y: 0, width: sw as i32, height: sh as i32 };
                info.available_rect = info.rect.clone();
                return 1;
            }
            0
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: c_int,
            height: c_int,
        ) {
            // Only the main view; ignore native popup widgets for now.
            if type_ != PaintElementType::VIEW || buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let (w, h) = (width as u32, height as u32);
            let len = (w * h * 4) as usize;
            let src = unsafe { std::slice::from_raw_parts(buffer, len) };
            let mut fb = view(self.role).frame.lock().unwrap();
            if fb.data.len() != len {
                fb.data.resize(len, 0);
            }
            fb.data.copy_from_slice(src);
            fb.width = w;
            fb.height = h;
            fb.dirty = true;
        }
    }
}

wrap_display_handler! {
    struct CyberDisplayHandler {
        role: Role,
    }

    impl DisplayHandler {
        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: sys::HCURSOR,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> c_int {
            *view(self.role).cursor.lock().unwrap() = Some(map_cursor(type_));
            1
        }

        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if let Role::Slot(i) = self.role {
                let new_url = url.map(|u| u.to_string()).unwrap_or_default();
                // Record a visit only when the address actually changes, so the
                // repeated address-change events one navigation can emit don't
                // over-count. The title arrives later (on_title_change), so this
                // records with an empty title and lets that update fill it in.
                // All slots record into the one shared history (CD-09).
                let changed = {
                    let mut nav = view(Role::Slot(i)).nav.lock().unwrap();
                    let changed = nav.url != new_url;
                    nav.url = new_url.clone();
                    changed
                };
                if changed {
                    crate::memory::record_visit(&new_url, "");
                }
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Role::Slot(i) = self.role {
                let new_title = title.map(|t| t.to_string()).unwrap_or_default();
                let url = {
                    let mut nav = view(Role::Slot(i)).nav.lock().unwrap();
                    nav.title = new_title.clone();
                    nav.url.clone()
                };
                crate::memory::update_title(&url, &new_title);
            }
        }
    }
}

wrap_load_handler! {
    struct CyberLoadHandler {
        role: Role,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ) {
            if let Role::Slot(i) = self.role {
                let mut nav = view(Role::Slot(i)).nav.lock().unwrap();
                nav.loading = is_loading != 0;
                nav.can_back = can_go_back != 0;
                nav.can_forward = can_go_forward != 0;
            }
        }
    }
}

wrap_life_span_handler! {
    struct CyberLifeSpanHandler {
        role: Role,
        tor: bool,
        generation: u32,
    }

    impl LifeSpanHandler {
        // Popup policy (D-0011 → D-0018): a genuine user-gesture popup (a click on
        // a `target=_blank` link, or a Ctrl-/middle-click on a link — Chromium
        // routes these here as tab dispositions with a gesture) is queued to open
        // in a NEW slot beside the source (the main thread decides capacity and
        // falls back to navigate-in-place when the grid is full). Popups without a
        // gesture (ad/script `window.open`) are dropped. No window ever opens.
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            if let Role::Slot(i) = self.role
                && user_gesture != 0
                && let Some(url) = target_url
            {
                pending_new_slot().lock().unwrap().push((i, url.to_string()));
            }
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                // FAIL-CLOSED (CD-15, D-0027): reject a browser built under a mode
                // that no longer matches the slot (a rapid re-toggle raced two
                // creations). Installing a clearnet browser on a Tor slot — or the
                // reverse — is an IP leak, so close it instead of registering it.
                if let Role::Slot(i) = self.role
                    && self.tor != slot_is_tor(i)
                {
                    if let Some(host) = browser.host() {
                        host.close_browser(1);
                    }
                    return;
                }
                // FAIL-CLOSED lifecycle (CD-25): a respawn or a close raced this
                // creation and bumped the slot generation, so this browser is stale —
                // close it instead of registering. Without this, two same-slot
                // respawns in one drain orphan the first browser, and a respawn drained
                // before a close leaves a live page/circuit on a closed slot (the tor
                // guard above misses both — a hardening respawn never flips Tor).
                if let Role::Slot(i) = self.role
                    && self.generation != slot_gen(i)
                {
                    if let Some(host) = browser.host() {
                        host.close_browser(1);
                    }
                    return;
                }
                // Give the OSR browser keyboard focus so the page accepts input, but
                // for a SLOT only when it is the ACTIVE slot (CD-21). The multi-slot
                // boot/restore creates several slots — each start page autofocuses its
                // search box — and browsers are created ASYNCHRONOUSLY (a Tor slot even
                // later, via TorSpawnTask), so an unconditional focus here would leave
                // the last-created slot, not the active one, holding the caret (two
                // carets at a 2-slot boot). Every creation path sets the slot active
                // BEFORE create, so the active slot still focuses on spawn. The MF-zone
                // view is mouse-driven and never wants the keyboard; the shared Internal
                // overlay focuses on create (harmless — it is not composited until an
                // overlay opens, which re-asserts its focus).
                let want_focus = match self.role {
                    Role::Slot(i) => i == active_slot(),
                    Role::Internal => true,
                    // The MF-zone and HUD views are mouse-driven / display-only
                    // and never want the keyboard.
                    Role::MfZone | Role::Hud => false,
                };
                if want_focus && let Some(host) = browser.host() {
                    host.set_focus(1);
                }
                *view(self.role).browser.lock().unwrap() = Some(browser.clone());
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(router) = BROWSER_ROUTER.get() {
                router.on_before_close(browser.map(|b| b.clone()));
            }
        }
    }
}

wrap_request_handler! {
    struct InternalRequestHandler;

    impl RequestHandler {
        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: c_int,
            _is_redirect: c_int,
        ) -> c_int {
            // Hard web isolation: the internal view may only ever navigate
            // within the cyberdesk:// scheme (NetGuard principle, D-0004).
            let url = request
                .as_ref()
                .map(|r| CefString::from(&r.url()).to_string())
                .unwrap_or_default();
            let allowed = url.is_empty() || url.starts_with("cyberdesk://");
            if !allowed {
                eprintln!("[isolation] internal view blocked navigation to: {url}");
                return 1; // cancel the navigation
            }
            // Allowed navigation: let the message router drop stale queries.
            if let Some(router) = BROWSER_ROUTER.get() {
                router.on_before_browse(browser.map(|b| b.clone()), frame.map(|f| f.clone()));
            }
            0 // proceed
        }

        fn on_render_process_terminated(
            &self,
            browser: Option<&mut Browser>,
            _status: TerminationStatus,
            _error_code: c_int,
            _error_string: Option<&CefString>,
        ) {
            if let Some(router) = BROWSER_ROUTER.get() {
                router.on_render_process_terminated(browser.map(|b| b.clone()));
            }
        }
    }
}

wrap_scheme_handler_factory! {
    struct InternalSchemeFactory;

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            // Route by host/path: cyberdesk://command/ -> command bar,
            // cyberdesk://info/ -> the info panel, everything else -> settings.
            let url = request
                .as_ref()
                .map(|r| CefString::from(&r.url()).to_string())
                .unwrap_or_default();
            let doc = if url.contains("//command") {
                command_document()
            } else if url.contains("//info") {
                info_document()
            } else if url.contains("//start") {
                start_document()
            } else if url.contains("//mfzone") {
                mfzone_document()
            } else if url.contains("//hud") {
                hud_document()
            } else {
                settings_document()
            };
            Some(InternalResourceHandler::new(
                Arc::new(doc.into_bytes()),
                Arc::new(AtomicUsize::new(0)),
                "text/html".to_string(),
            ))
        }
    }
}

wrap_resource_handler! {
    struct InternalResourceHandler {
        data: Arc<Vec<u8>>,
        offset: Arc<AtomicUsize>,
        mime: String,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut c_int>,
            _callback: Option<&mut Callback>,
        ) -> c_int {
            // Handle synchronously and continue immediately.
            if let Some(handle_request) = handle_request {
                *handle_request = 1;
            }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            if let Some(response) = response {
                response.set_status(200);
                response.set_status_text(Some(&CefString::from("OK")));
                response.set_mime_type(Some(&CefString::from(self.mime.as_str())));
            }
            if let Some(response_length) = response_length {
                *response_length = self.data.len() as i64;
            }
        }

        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: c_int,
            bytes_read: Option<&mut c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> c_int {
            let Some(bytes_read) = bytes_read else {
                return 0;
            };
            if bytes_to_read < 1 {
                *bytes_read = 0;
                return 0;
            }
            let off = self.offset.load(Ordering::Relaxed);
            let remaining = self.data.len().saturating_sub(off);
            if remaining == 0 {
                *bytes_read = 0;
                return 0; // complete
            }
            let n = remaining.min(bytes_to_read as usize);
            unsafe {
                std::ptr::copy_nonoverlapping(self.data.as_ptr().add(off), data_out, n);
            }
            self.offset.store(off + n, Ordering::Relaxed);
            *bytes_read = n as c_int;
            1
        }
    }
}

// --- Renderer-side message router (render process) --------------------------

/// The renderer-side router lives on the render process main thread. It is not
/// `Sync` (it holds V8 handles), so it is kept in thread-local storage rather
/// than a global — every render callback runs on that same thread.
fn render_router() -> Arc<RendererSideRouter> {
    thread_local! {
        static R: Arc<RendererSideRouter> =
            <RendererSideRouter as MessageRouterRendererSide>::new(MessageRouterConfig::default());
    }
    R.with(|r| r.clone())
}

wrap_render_process_handler! {
    struct CyberRenderProcessHandler;

    impl RenderProcessHandler {
        /// Capture the per-window hardening config the browser process stamped into
        /// `extra_info` (CD-25), keyed by the stable Browser::identifier(). Runs
        /// before any of this browser's contexts are created.
        fn on_browser_created(
            &self,
            browser: Option<&mut Browser>,
            extra_info: Option<&mut DictionaryValue>,
        ) {
            let Some(b) = browser else { return };
            let id = b.identifier();
            let (json, seed) = match extra_info {
                Some(d) => {
                    let json = CefString::from(&d.string(Some(&CefString::from("cd_harden")))).to_string();
                    let seed = CefString::from(&d.string(Some(&CefString::from("cd_seed")))).to_string();
                    // Fail safe: no key -> Standard (protected), never Off.
                    let json = if json.is_empty() { crate::harden::STANDARD_JSON.to_string() } else { json };
                    (json, seed)
                }
                None => (crate::harden::STANDARD_JSON.to_string(), String::new()),
            };
            render_store_cfg(id, json, seed);
        }

        fn on_browser_destroyed(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                render_remove_cfg(b.identifier());
            }
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            // Two mutually-exclusive jobs, by frame scheme:
            //  * cyberdesk:// (our internal UI) — expose window.cefQuery so the IPC
            //    bridge exists SOLELY on the internal views, never on the web.
            //  * a web frame — inject the CD-16 fingerprinting hardening at
            //    document-start (before any page script), under THIS browser's
            //    effective per-window config (CD-25). Off => skip injection entirely.
            //    Never both: our own UI is trusted and must not be farbled; web frames
            //    get no IPC bridge.
            let bid = browser.as_ref().map(|b| b.identifier()).unwrap_or(0);
            let url = frame
                .as_ref()
                .map(|f| CefString::from(&f.url()).to_string())
                .unwrap_or_default();
            if url.starts_with("cyberdesk://") {
                render_router().on_context_created(
                    browser.map(|b| b.clone()),
                    frame.map(|f| f.clone()),
                    context.map(|c| c.clone()),
                );
            } else if should_harden(&url)
                && let Some(f) = frame
            {
                let cfg = render_get_cfg(bid);
                if cfg.inject {
                    inject_hardening(f, &cfg.json, &cfg.seed);
                }
            }
        }

        fn on_context_released(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            render_router().on_context_released(
                browser.map(|b| b.clone()),
                frame.map(|f| f.clone()),
                context.map(|c| c.clone()),
            );
        }

        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> c_int {
            if render_router().on_process_message_received(
                browser.map(|b| b.clone()),
                frame.map(|f| f.clone()),
                Some(source_process),
                message.map(|m| m.clone()),
            ) {
                1
            } else {
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_input_for, common_screen_for, fresh_seed_hex, restamp_tor_status, search_url_for};

    // --- Identity seed (CD-29): rotation produces fresh, well-formed seeds ------

    /// A fresh seed is 32 lowercase-hex chars (16 bytes) and two draws differ — the
    /// property that makes a rotation actually change the fingerprint (Task E).
    #[test]
    fn fresh_seed_is_hex_and_unique() {
        let a = fresh_seed_hex();
        let b = fresh_seed_hex();
        assert_eq!(a.len(), 32, "16 bytes -> 32 hex chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b, "two rotations must yield different seeds");
    }

    // --- Screen presets (CD-29): common-and-consistent, never a decoy ---------

    /// The reported screen is the preset when the viewport fits inside it — every
    /// ordinary window on the default 1080p preset reports exactly 1920x1080.
    #[test]
    fn screen_reports_the_preset_when_viewport_fits() {
        assert_eq!(common_screen_for((1920, 1080), (800, 1000)), (1920, 1080));
        assert_eq!(common_screen_for((1280, 720), (600, 700)), (1280, 720));
        assert_eq!(common_screen_for((1600, 900), (1500, 850)), (1600, 900));
    }

    /// The reported screen is NEVER smaller than the viewport on either axis (a
    /// viewport larger than the monitor is an impossible contradiction). A big
    /// single-column layout bumps up to the next COMMON ladder rung, not the exact
    /// pixel size.
    #[test]
    fn screen_never_contradicts_the_viewport() {
        // A 2000-wide column on the 1080p preset can't report 1920 — bump to 2560x1440.
        assert_eq!(common_screen_for((1920, 1080), (2000, 1300)), (2560, 1440));
        // A tall column forces a taller common resolution (a 1000-tall column on the
        // 720p preset can't report 720 or 900 — bump to 1920x1080).
        assert_eq!(common_screen_for((1280, 720), (600, 1000)), (1920, 1080));
        // Reported ≥ viewport on both axes, always.
        for &(vw, vh) in &[(3000u32, 1600u32), (5000, 2000), (1000, 1000)] {
            let (sw, sh) = common_screen_for((1920, 1080), (vw, vh));
            assert!(sw >= vw && sh >= vh, "reported {sw}x{sh} must contain viewport {vw}x{vh}");
        }
    }

    /// A viewport larger than the whole ladder falls back to its real size — an
    /// unusually large window can't be hidden, only the exact monitor size withheld;
    /// still never a decoy (reported == the real viewport, not something smaller).
    #[test]
    fn screen_falls_back_to_real_size_past_the_ladder() {
        assert_eq!(common_screen_for((1920, 1080), (5000, 3000)), (5000, 3000));
    }

    // --- Search routing (CD-27, D-0043): the selector is authoritative --------

    /// Every allowlisted engine routes a query to ITS OWN host — the engine
    /// shown is the engine used (CD-27 acceptance 3-5, headless half).
    #[test]
    fn each_engine_routes_to_its_own_host() {
        let cases = [
            ("duckduckgo", "https://duckduckgo.com/?q="),
            ("bing", "https://www.bing.com/search?q="),
            ("startpage", "https://www.startpage.com/sp/search?query="),
            ("brave", "https://search.brave.com/search?q="),
            ("google", "https://www.google.com/search?q="),
        ];
        for (engine, prefix) in cases {
            let url = search_url_for(engine, "rust borrow checker");
            assert!(
                url.starts_with(prefix),
                "{engine} must search on its own host, got {url}"
            );
        }
    }

    /// A non-Google engine must never leak the query anywhere near Google —
    /// the de-Google guarantee for the chosen-search path (CD-27 acceptance 1).
    #[test]
    fn non_google_engines_never_touch_google() {
        for engine in ["duckduckgo", "bing", "startpage", "brave"] {
            let url = search_url_for(engine, "how to test a search engine");
            assert!(
                !url.contains("google"),
                "{engine} search leaked toward google: {url}"
            );
        }
    }

    /// An unknown engine name falls back to the factory default, DuckDuckGo —
    /// a mis-stored or future value must never silently search Google (CD-27,
    /// D-0043).
    #[test]
    fn unknown_engine_falls_back_to_duckduckgo() {
        assert!(search_url_for("", "x").starts_with("https://duckduckgo.com/?q="));
        assert!(search_url_for("altavista", "x").starts_with("https://duckduckgo.com/?q="));
    }

    /// The URL-vs-search decision: bare domains, schemes, and localhost forms
    /// navigate; free text (even with dots among spaces) searches; empty input
    /// is inert. Same rules for every engine — only the search host differs.
    #[test]
    fn query_vs_url_classification() {
        // Free text searches on the given engine.
        assert!(
            classify_input_for("duckduckgo", "rust borrow checker")
                .starts_with("https://duckduckgo.com/?q=")
        );
        // A bare domain navigates (https default), it does not search.
        assert_eq!(
            classify_input_for("duckduckgo", "example.com"),
            "https://example.com"
        );
        // An explicit scheme passes through untouched.
        assert_eq!(
            classify_input_for("duckduckgo", "http://example.com/a"),
            "http://example.com/a"
        );
        assert_eq!(
            classify_input_for("duckduckgo", "cyberdesk://start/"),
            "cyberdesk://start/"
        );
        // localhost forms navigate.
        assert_eq!(
            classify_input_for("duckduckgo", "localhost:8080"),
            "https://localhost:8080"
        );
        // A dot inside spaced text is still a search, not a URL.
        assert!(
            classify_input_for("duckduckgo", "what is web 2.0")
                .starts_with("https://duckduckgo.com/?q=")
        );
        // Empty / whitespace input is inert.
        assert_eq!(classify_input_for("duckduckgo", "  "), "about:blank");
    }

    /// Queries are form-urlencoded: spaces become `+`, reserved bytes are
    /// percent-escaped — no raw query text ever lands in the URL.
    #[test]
    fn queries_are_urlencoded() {
        assert_eq!(
            search_url_for("duckduckgo", "a b&c=d?"),
            "https://duckduckgo.com/?q=a+b%26c%3Dd%3F"
        );
    }

    /// The get_frame pull re-stamps the LIVE Tor status into the cached frame payload,
    /// so a (re)created command-band consumer never reads a latched "connecting" (CD-23).
    #[test]
    fn restamp_overwrites_stale_tor_status() {
        // A cached payload whose tor_status is stale (1 = bootstrapping)…
        let cached = r#"{"slots":[{"id":0,"x":0,"w":800,"tor":true}],"engaged":null,"autofocus":false,"tor_status":1}"#;
        // …is re-stamped with the current engine status (2 = ready).
        let out = restamp_tor_status(cached, 2);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["tor_status"], 2, "live status replaces the cached one");
        // The rest of the payload is preserved untouched.
        assert_eq!(v["slots"][0]["tor"], true);
        assert_eq!(v["engaged"], serde_json::Value::Null);
    }

    #[test]
    fn restamp_adds_tor_status_when_absent_and_tolerates_non_objects() {
        // An object with no tor_status gains it.
        let added = restamp_tor_status(r#"{"slots":[]}"#, 3);
        let v: serde_json::Value = serde_json::from_str(&added).unwrap();
        assert_eq!(v["tor_status"], 3);
        // The "{}" seed round-trips with the status stamped in.
        let seed = restamp_tor_status("{}", 0);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&seed).unwrap()["tor_status"], 0);
        // A non-object (malformed / unexpected) cache is returned unchanged, never a panic.
        assert_eq!(restamp_tor_status("not json", 2), "not json");
        assert_eq!(restamp_tor_status("[1,2,3]", 2), "[1,2,3]");
    }
}
