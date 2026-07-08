//! CEF (Chromium Embedded Framework) integration — the CyberDesk views.
//!
//! Two off-screen (OSR) browser views live here, distinguished by a [`Role`]:
//!
//!   * [`Role::Surf`] — the surf zone (google.com), full web browsing.
//!   * [`Role::Internal`] — the settings page, locked to the internal
//!     `cyberdesk://settings/` scheme (see docs/cyberdesk-decisions.md, D-0010).
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

use std::os::raw::c_int;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cef::wrapper::message_router::{
    BrowserSideCallback, BrowserSideHandler, BrowserSideRouter, MessageRouterBrowserSide,
    MessageRouterBrowserSideHandlerCallbacks, MessageRouterConfig, MessageRouterRendererSide,
    MessageRouterRendererSideHandlerCallbacks, RendererSideRouter,
};
use cef::*;
use winit::event::MouseButton;
use winit::window::CursorIcon;

/// Page loaded into the surf-zone view.
const HOME_URL: &str = "https://www.google.com/";
/// The internal custom scheme and the settings document URL (D-0010).
const SCHEME: &str = "cyberdesk";
const SETTINGS_URL: &str = "cyberdesk://settings/";
const COMMAND_URL: &str = "cyberdesk://command/";

// cef_event_flags_t bits (modifiers for mouse/key events).
const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;

/// Which OSR view a call targets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Surf,
    Internal,
}

impl Role {
    fn idx(self) -> usize {
        match self {
            Role::Surf => 0,
            Role::Internal => 1,
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

struct ViewState {
    frame: Mutex<FrameBuffer>,
    geom: Mutex<ViewGeom>,
    browser: Mutex<Option<Browser>>,
    cursor: Mutex<Option<CursorIcon>>,
}
impl ViewState {
    fn new() -> Self {
        Self {
            frame: Mutex::new(FrameBuffer::default()),
            geom: Mutex::new(ViewGeom::default()),
            browser: Mutex::new(None),
            cursor: Mutex::new(None),
        }
    }
}

fn views() -> &'static [ViewState; 2] {
    static V: OnceLock<[ViewState; 2]> = OnceLock::new();
    V.get_or_init(|| [ViewState::new(), ViewState::new()])
}
fn view(role: Role) -> &'static ViewState {
    &views()[role.idx()]
}

static CONTEXT_READY: AtomicBool = AtomicBool::new(false);

/// Browser-side message router (settings IPC). Created on the UI thread in
/// `on_context_initialized`; read from the client/request/life-span handlers.
static BROWSER_ROUTER: OnceLock<Arc<BrowserSideRouter>> = OnceLock::new();

// --- Surf navigation state (CEF UI thread -> main thread) -------------------
// The LoadHandler / DisplayHandler callbacks fire on the CEF UI thread; the main
// thread reads these for the loading line, the window title, and get_nav_state.
static SURF_LOADING: AtomicBool = AtomicBool::new(false);
static SURF_CAN_BACK: AtomicBool = AtomicBool::new(false);
static SURF_CAN_FWD: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct SurfNav {
    url: String,
    title: String,
}
fn surf_nav() -> &'static Mutex<SurfNav> {
    static N: OnceLock<Mutex<SurfNav>> = OnceLock::new();
    N.get_or_init(|| Mutex::new(SurfNav::default()))
}

pub fn surf_loading() -> bool {
    SURF_LOADING.load(Ordering::Relaxed)
}
pub fn surf_title() -> String {
    surf_nav().lock().unwrap().title.clone()
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
pub fn surf_can_back() -> bool {
    SURF_CAN_BACK.load(Ordering::Relaxed)
}
pub fn surf_can_forward() -> bool {
    SURF_CAN_FWD.load(Ordering::Relaxed)
}
pub fn surf_url() -> String {
    surf_nav().lock().unwrap().url.clone()
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

/// Create a windowless (OSR) browser for `role`. `parent_hwnd` is used only for
/// monitor / DPI info — there is no child window.
pub fn create_browser(role: Role, parent_hwnd: isize) {
    let window_info =
        WindowInfo::default().set_as_windowless(sys::HWND(parent_hwnd as *mut sys::HWND__));

    let mut client = CyberClient::new(role);
    let url = CefString::from(match role {
        Role::Surf => HOME_URL,
        Role::Internal => SETTINGS_URL,
    });
    let background_color = match role {
        // Surf: opaque white backing (the page paints its own background).
        Role::Surf => 0xFFFF_FFFFu32,
        // Internal: opaque panel-colored backing so the settings card is solid;
        // the wgpu compositor rounds its corners. Color comes from the token set.
        Role::Internal => argb_from_hex(&crate::theme::Theme::load().colors.panel),
    };
    let browser_settings = BrowserSettings {
        windowless_frame_rate: 60,
        background_color,
        ..Default::default()
    };

    let created = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&browser_settings),
        None,
        None,
    );
    assert_eq!(created, 1, "CefBrowserHost::CreateBrowser failed");
}

/// Opaque ARGB (0xFFRRGGBB) from a `#RRGGBB` token, for a CEF backing color.
fn argb_from_hex(hex: &str) -> u32 {
    let c = crate::theme::hex3(hex);
    let r = (c[0] * 255.0).round() as u32;
    let g = (c[1] * 255.0).round() as u32;
    let b = (c[2] * 255.0).round() as u32;
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Navigate a view (used by the isolation self-test). The internal view's
/// RequestHandler will refuse anything that is not `cyberdesk://`.
pub fn load_url(role: Role, url: &str) {
    let browser = view(role).browser.lock().unwrap().clone();
    if let Some(browser) = browser {
        if let Some(frame) = browser.main_frame() {
            frame.load_url(Some(&CefString::from(url)));
        }
    }
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
    if let Some(browser) = browser {
        if let Some(host) = browser.host() {
            f(host);
        }
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

/// Host-side URL-vs-search decision. A scheme, or a dot without spaces, or
/// localhost is treated as a URL (defaulting to https://); everything else
/// becomes a Google search.
fn classify_input(input: &str) -> String {
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
        format!("https://www.google.com/search?q={}", urlencode(t))
    }
}

/// Current surf navigation state as JSON, for the `get_nav_state` IPC reply.
fn nav_state_json() -> String {
    let url = surf_url();
    let scheme = scheme_of(&url);
    serde_json::json!({
        "url": url,
        "title": surf_title(),
        "can_back": surf_can_back(),
        "can_forward": surf_can_forward(),
        "loading": surf_loading(),
        "scheme": scheme,
    })
    .to_string()
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
                .and_then(|x| x.as_bool())
                .ok_or((2, "missing or non-boolean 'value'".to_string()))?;
            crate::settings::set(key, value).map_err(|e| (3, e))
        }
        // Command bar / navigation (CD-04).
        "get_nav_state" => Ok(nav_state_json()),
        "navigate" => {
            let input = v
                .get("input")
                .and_then(|x| x.as_str())
                .ok_or((2, "missing 'input'".to_string()))?;
            let url = classify_input(input);
            load_url(Role::Surf, &url);
            request_overlay_close();
            Ok(serde_json::json!({ "ok": true, "url": url }).to_string())
        }
        "go_back" => {
            go_back(Role::Surf);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        "go_forward" => {
            go_forward(Role::Surf);
            Ok(serde_json::json!({ "ok": true }).to_string())
        }
        "reload" => {
            reload(Role::Surf);
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

            CONTEXT_READY.store(true, Ordering::Relaxed);
        }
    }
}

wrap_client! {
    struct CyberClient {
        role: Role,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(CyberRenderHandler::new(self.role))
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(CyberDisplayHandler::new(self.role))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            // Only the surf view drives the loading line / nav state.
            match self.role {
                Role::Surf => Some(CyberLoadHandler::new(self.role)),
                Role::Internal => None,
            }
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(CyberLifeSpanHandler::new(self.role))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            // Web isolation + IPC lifecycle only on the internal view.
            match self.role {
                Role::Internal => Some(InternalRequestHandler::new()),
                Role::Surf => None,
            }
        }
        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> c_int {
            if self.role == Role::Internal {
                if let Some(router) = BROWSER_ROUTER.get() {
                    if router.on_process_message_received(
                        browser.map(|b| b.clone()),
                        frame.map(|f| f.clone()),
                        source_process,
                        message.map(|m| m.clone()),
                    ) {
                        return 1;
                    }
                }
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
                let dip_w = (g.phys_w as f32 / g.scale).round() as i32;
                let dip_h = (g.phys_h as f32 / g.scale).round() as i32;
                info.rect = Rect { x: 0, y: 0, width: dip_w, height: dip_h };
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
            if self.role == Role::Surf {
                surf_nav().lock().unwrap().url = url.map(|u| u.to_string()).unwrap_or_default();
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if self.role == Role::Surf {
                surf_nav().lock().unwrap().title = title.map(|t| t.to_string()).unwrap_or_default();
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
            if self.role == Role::Surf {
                SURF_LOADING.store(is_loading != 0, Ordering::Relaxed);
                SURF_CAN_BACK.store(can_go_back != 0, Ordering::Relaxed);
                SURF_CAN_FWD.store(can_go_forward != 0, Ordering::Relaxed);
            }
        }
    }
}

wrap_life_span_handler! {
    struct CyberLifeSpanHandler {
        role: Role,
    }

    impl LifeSpanHandler {
        // Popup policy (D-0011): a genuine user gesture (a click on a
        // target=_blank link) navigates THIS view to the target and suppresses
        // the popup window; popups without a gesture (ad/script `window.open`)
        // are suppressed outright. Either way, no separate window is ever opened.
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
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
            if self.role == Role::Surf && user_gesture != 0 {
                if let (Some(browser), Some(url)) = (browser, target_url) {
                    if let Some(frame) = browser.main_frame() {
                        frame.load_url(Some(url));
                    }
                }
            }
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                // Give the OSR browser keyboard focus so the page accepts input.
                if let Some(host) = browser.host() {
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
            // Route by host/path: cyberdesk://command/ -> command bar, everything
            // else under the scheme -> settings.
            let url = request
                .as_ref()
                .map(|r| CefString::from(&r.url()).to_string())
                .unwrap_or_default();
            let doc = if url.contains("//command") {
                command_document()
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
        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            // Expose window.cefQuery ONLY on cyberdesk:// contexts, so the IPC
            // bridge exists solely on the internal settings view.
            let is_internal = match &frame {
                Some(f) => CefString::from(&f.url()).to_string().starts_with("cyberdesk://"),
                None => false,
            };
            if is_internal {
                render_router().on_context_created(
                    browser.map(|b| b.clone()),
                    frame.map(|f| f.clone()),
                    context.map(|c| c.clone()),
                );
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
