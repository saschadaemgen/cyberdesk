//! CEF (Chromium Embedded Framework) integration — the CyberDesk surf-zone.
//!
//! CD-02 replaces CD-01's child-window embed with **off-screen rendering
//! (OSR)**: CEF renders the page into a CPU buffer (`RenderHandler::on_paint`),
//! we hand the raw BGRA bytes to the renderer, and it composites the page as a
//! texture inside our own frame. There is no child window anymore.
//!
//! CEF runs with a multi-threaded message loop (from CD-01). `on_paint` and the
//! cursor callback arrive on CEF's UI thread, so the handoff to the main thread
//! goes through mutex-protected shared state; all wgpu work stays on the main
//! thread. Input (mouse/keyboard) is forwarded from winit into the BrowserHost.
//!
//! Sandbox note: the Windows CEF sandbox is still disabled here (`no_sandbox`);
//! see docs/cyberdesk-decisions.md, D-0008, for the tracked deviation.

use std::os::raw::c_int;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use cef::*;
use winit::event::MouseButton;
use winit::window::CursorIcon;

/// Page loaded into the embedded surf-zone view.
const HOME_URL: &str = "https://www.google.com/";

// cef_event_flags_t bits (modifiers for mouse/key events).
const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;

// --- Shared state (main thread <-> CEF UI thread) ---------------------------

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

fn frame() -> &'static Mutex<FrameBuffer> {
    static F: OnceLock<Mutex<FrameBuffer>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(FrameBuffer::default()))
}
fn geom() -> &'static Mutex<ViewGeom> {
    static G: OnceLock<Mutex<ViewGeom>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(ViewGeom::default()))
}
fn browser_slot() -> &'static Mutex<Option<Browser>> {
    static B: OnceLock<Mutex<Option<Browser>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(None))
}
fn cursor_slot() -> &'static Mutex<Option<CursorIcon>> {
    static C: OnceLock<Mutex<Option<CursorIcon>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}
static CONTEXT_READY: AtomicBool = AtomicBool::new(false);

// --- Process / lifecycle ----------------------------------------------------

/// Must be the first thing `main` does. Binds the CEF API version and runs the
/// CEF sub-process logic: for CEF sub-processes this blocks until the
/// sub-process exits and then terminates the process; for the browser process
/// it returns.
pub fn run_subprocess_if_needed() {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let code = execute_process(Some(args.as_main_args()), None::<&mut App>, ptr::null_mut());
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

/// Set the surf-zone size (device pixels) and DPI scale. Call before creating
/// the browser and on every resize.
pub fn set_view_geometry(phys_w: u32, phys_h: u32, scale: f32) {
    *geom().lock().unwrap() = ViewGeom { phys_w, phys_h, scale };
}

/// Create the windowless (OSR) browser. `parent_hwnd` is used only for monitor
/// / DPI info — there is no child window.
pub fn create_browser(parent_hwnd: isize) {
    let window_info =
        WindowInfo::default().set_as_windowless(sys::HWND(parent_hwnd as *mut sys::HWND__));

    let mut client = CyberClient::new();
    let url = CefString::from(HOME_URL);
    let browser_settings = BrowserSettings {
        windowless_frame_rate: 60,
        background_color: 0xFFFF_FFFF, // opaque backing (page paints its own bg)
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

// --- Handoffs to the main thread --------------------------------------------

/// If a fresh CEF frame has arrived, hand its BGRA bytes to `f` and clear the
/// dirty flag.
pub fn with_dirty_frame(f: impl FnOnce(&[u8], u32, u32)) {
    let mut fb = frame().lock().unwrap();
    if fb.dirty {
        f(&fb.data, fb.width, fb.height);
        fb.dirty = false;
    }
}

/// Take a pending cursor icon requested by the page (applied by winit on the
/// main thread).
pub fn take_cursor() -> Option<CursorIcon> {
    cursor_slot().lock().unwrap().take()
}

// --- Input forwarding (main thread -> BrowserHost) --------------------------

fn with_host(f: impl FnOnce(BrowserHost)) {
    let browser = browser_slot().lock().unwrap().clone();
    if let Some(browser) = browser {
        if let Some(host) = browser.host() {
            f(host);
        }
    }
}

pub fn notify_resized() {
    with_host(|host| host.was_resized());
}

/// Give or take keyboard focus from the OSR browser. Required for the page to
/// accept keyboard input (there is no window to receive OS focus).
pub fn set_focus(focused: bool) {
    with_host(|host| host.set_focus(focused as c_int));
}

pub fn send_mouse_move(x: i32, y: i32, modifiers: u32, leave: bool) {
    with_host(|host| {
        let ev = MouseEvent { x, y, modifiers };
        host.send_mouse_move_event(Some(&ev), leave as c_int);
    });
}

pub fn send_mouse_button(x: i32, y: i32, modifiers: u32, button: MouseButton, down: bool, clicks: i32) {
    let bt = match button {
        MouseButton::Left => MouseButtonType::LEFT,
        MouseButton::Middle => MouseButtonType::MIDDLE,
        MouseButton::Right => MouseButtonType::RIGHT,
        _ => return,
    };
    with_host(|host| {
        let ev = MouseEvent { x, y, modifiers };
        host.send_mouse_click_event(Some(&ev), bt, (!down) as c_int, clicks);
    });
}

pub fn send_mouse_wheel(x: i32, y: i32, modifiers: u32, delta_x: i32, delta_y: i32) {
    with_host(|host| {
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

pub fn send_key_down(vk: i32, modifiers: u32) {
    with_host(|host| host.send_key_event(Some(&key_event(KeyEventType::RAWKEYDOWN, vk, 0, modifiers))));
}
pub fn send_key_up(vk: i32, modifiers: u32) {
    with_host(|host| host.send_key_event(Some(&key_event(KeyEventType::KEYUP, vk, 0, modifiers))));
}
pub fn send_char(ch: u16, modifiers: u32) {
    // For CHAR events Chromium expects windows_key_code to carry the character.
    with_host(|host| host.send_key_event(Some(&key_event(KeyEventType::CHAR, ch as i32, ch, modifiers))));
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

// --- CEF handler implementations --------------------------------------------

wrap_app! {
    pub struct CyberApp;

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(CyberBrowserProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct CyberBrowserProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            CONTEXT_READY.store(true, Ordering::Relaxed);
        }
    }
}

wrap_client! {
    struct CyberClient;

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(CyberRenderHandler::new())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(CyberDisplayHandler::new())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(CyberLifeSpanHandler::new())
        }
    }
}

wrap_render_handler! {
    struct CyberRenderHandler;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let g = *geom().lock().unwrap();
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
                let g = *geom().lock().unwrap();
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
            let mut fb = frame().lock().unwrap();
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
    struct CyberDisplayHandler;

    impl DisplayHandler {
        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: sys::HCURSOR,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> c_int {
            *cursor_slot().lock().unwrap() = Some(map_cursor(type_));
            1
        }
    }
}

wrap_life_span_handler! {
    struct CyberLifeSpanHandler;

    impl LifeSpanHandler {
        // Suppress popups entirely (no new windows in the surf zone).
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: c_int,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                // Give the OSR browser keyboard focus so the page accepts input.
                if let Some(host) = browser.host() {
                    host.set_focus(1);
                }
                *browser_slot().lock().unwrap() = Some(browser.clone());
            }
        }
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
