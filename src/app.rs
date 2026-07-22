//! winit application: window, render loop, surf-zone + settings geometry, input
//! forwarding into CEF (OSR), the gear button, cursor feedback, ESC handling.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId, WindowLevel};

use zeroize::Zeroize;

use crate::browser::{self, Role};
use crate::renderer::{self, InfoGlyph, SlotView, SurfaceRenderer};
use crate::settings;
use crate::slots::{self, MAX_SLOTS};
use crate::theme::Theme;

/// Grace period after the cursor leaves the engaged band region before it
/// disengages (hysteresis - no flicker on grazing touches, CD-08 → CD-12).
const BAR_HIDE_HYSTERESIS: Duration = Duration::from_millis(250);

/// After the band disengages, keep it composited this long so the page's
/// per-ensemble fade-out (CSS ~220 ms) completes before compositing stops (CD-12).
const BAND_FADE_LINGER: Duration = Duration::from_millis(300);

/// Per-frame ease factor for the CD-11 frame reflow (side zones retreating to
/// rails + the slot recenter). Exponential approach, ~220 ms ease-out at ~60 fps
/// - the same host-side interpolation pattern as the top-bar slide.
const FRAME_EASE: f32 = 0.22;

/// Exponentially ease a rect toward a target by factor `k` (per frame).
fn ease_rect(cur: slots::Rect, target: slots::Rect, k: f32) -> slots::Rect {
    slots::Rect {
        x: cur.x + (target.x - cur.x) * k,
        y: cur.y + (target.y - cur.y) * k,
        w: cur.w + (target.w - cur.w) * k,
        h: cur.h + (target.h - cur.h) * k,
    }
}

/// Settings card rectangle in device pixels: a centered panel, clamped so it
/// stays a readable size on both the dev window and a 4K fullscreen shell.
fn panel_rect(width: u32, height: u32) -> (f32, f32, f32, f32) {
    let (w, h) = (width as f32, height as f32);
    let pw = (w * 0.42).clamp(420.0, 760.0).min(w);
    let ph = (h * 0.64).clamp(360.0, 600.0).min(h);
    let px = ((w - pw) * 0.5).round();
    let py = ((h - ph) * 0.5).round();
    (px, py, pw.round(), ph.round())
}

/// Gear button geometry in device pixels: (center_x, center_y, radius),
/// top-right, DPI-scaled.
fn gear_geom(width: u32, scale: f32) -> (f32, f32, f32) {
    let w = width as f32;
    let r = 22.0 * scale;
    let margin = 30.0 * scale;
    (w - margin - r, margin + r, r)
}

/// The local wall-clock's offset from UTC in minutes (CD-30: the HUD's digital
/// clock). The PROCESS runs under TZ=UTC (the CD-16 timezone clamp - honest and
/// global), so local time is derived from the OS timezone via Win32 - never from
/// the (deliberately clamped) C-runtime timezone, which would silently show UTC.
#[cfg(windows)]
fn local_utc_offset_minutes() -> i32 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SystemTime16 {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime16,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime16,
        daylight_bias: i32,
    }
    unsafe extern "system" {
        fn GetTimeZoneInformation(tz: *mut TimeZoneInformation) -> u32;
    }
    let mut tz = TimeZoneInformation {
        bias: 0,
        standard_name: [0; 32],
        standard_date: SystemTime16::default(),
        standard_bias: 0,
        daylight_name: [0; 32],
        daylight_date: SystemTime16::default(),
        daylight_bias: 0,
    };
    // Returns 0 unknown / 1 standard / 2 daylight; UTC offset = -(bias + active).
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let active = if id == 2 { tz.daylight_bias } else { tz.standard_bias };
    -(tz.bias + active)
}
#[cfg(not(windows))]
fn local_utc_offset_minutes() -> i32 {
    0
}

/// Read the clipboard as text - the host-side paste path for vault secret
/// capture (CD-40): Ctrl+V during a capture appends clipboard text to the
/// locked input buffer without the renderer ever seeing it. Same direct-extern
/// style as the timezone read above; user32/kernel32 are in the link set.
#[cfg(windows)]
fn clipboard_text() -> Option<String> {
    use core::ffi::c_void;
    unsafe extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn GlobalLock(h: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(h: *mut c_void) -> i32;
    }
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let mut out = None;
        let handle = GetClipboardData(CF_UNICODETEXT);
        if !handle.is_null() {
            let p = GlobalLock(handle).cast::<u16>();
            if !p.is_null() {
                // CF_UNICODETEXT is NUL-terminated by contract.
                let mut len = 0usize;
                while *p.add(len) != 0 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        out
    }
}

#[cfg(not(windows))]
fn clipboard_text() -> Option<String> {
    None
}

/// Which internal overlay (if any) is currently shown. `Command` is the CD-12
/// floating command band; `Settings` is the gear card; `Info` is the CD-13
/// update-awareness panel. All mutually exclusive - one shared internal OSR view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Closed,
    Settings,
    Command,
    Info,
    /// The start-authorization gate (CD-40, D-0058): the internal view shows
    /// `cyberdesk://lock/` and nothing else exists - no slots, no MF zone, no
    /// HUD. Every keystroke is captured by the HOST (never the page) while
    /// this overlay is up; leaving it is only possible by unlocking (or quit).
    Lock,
}

pub fn run(windowed: bool) {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    // Opens and takes ownership of the app-state store (state.db) and loads the
    // persisted toggles; the settings IPC writes through it live.
    settings::init();
    // Load the vault state (CD-40, D-0058): with a vault present the shell boots
    // LOCKED; with none it boots into MANDATORY first-launch setup (CD-42,
    // D-0062) - only the lock/setup page exists until the start-authorization
    // gate opens. Must follow settings::init (shares the app-data dir) and
    // precede everything that might touch sealed state.
    crate::vault::init();
    let locked = crate::vault::gate_closed();
    // Anti-forensic browsing-residue purge (CD-34, D-0051): wipe the CEF
    // browsing-cache/profile dir BEFORE init_cef (below) - the only moment CEF does
    // not yet hold its files open. Reads the `purge_residue` toggle just loaded by
    // settings::init; never touches the Tor state, session, or config (a disjoint tree).
    crate::forensic::purge_on_launch();
    // Initialize the global identity seed (CD-29): fresh each launch, or the
    // persisted seed when "new identity on restart" is off. Must follow settings::init.
    // While the vault gate is closed this is DEFERRED to the unlock transition -
    // the persisted seed is a sealed tenant and unreadable before the VMK exists.
    if !locked {
        browser::init_identity_seed();
    }

    let mut app = Shell {
        windowed,
        locked,
        window: None,
        renderer: None,
        theme: Theme::load(),
        start: Instant::now(),
        rot_anchor: Instant::now(),
        rot_flash_until: None,
        red_flash_until: None,
        red_slots: 0,
        lock_view_started: false,
        cef_inited: false,
        views_started: false,
        scale: 1.0,
        cursor_phys: PhysicalPosition::new(0.0, 0.0),
        key_mods: 0,
        mods: ModifiersState::empty(),
        button_flags: 0,
        applied_cursor: CursorIcon::Default,
        overlay: Overlay::Closed,
        gear_hover: 0.0,
        gear_hover_target: 0.0,
        info_hover: 0.0,
        info_hover_target: 0.0,
        info_active: 0.0,
        order: vec![0],
        active_slot: 0,
        mouse_role: None,
        loading: [0.0; MAX_SLOTS],
        width_units: [1; MAX_SLOTS],
        disp_rects: [None; MAX_SLOTS],
        disp_left: slots::Rect::default(),
        disp_right: slots::Rect::default(),
        disp_left_width: 0.0,
        frame_inited: false,
        applied_title: String::new(),
        applied_topmost: false,
        isolation_tested: false,
        applied_internal: (0, 0),
        engaged_slot: None,
        bar_hide_at: None,
        band_off_at: None,
        frame_sig: String::new(),
        hud_sig: String::new(),
        tor_status_pushed: u8::MAX,
        drag: None,
    };
    event_loop.run_app(&mut app).expect("event loop error");

    // Belt-and-braces (CD-40 acceptance: zeroized on exit): statics never drop
    // on process exit, so the vault wipes its key material explicitly on every
    // deliberate shutdown path.
    crate::vault::wipe_for_exit();
    browser::shutdown_cef();
}

struct Shell {
    windowed: bool,
    /// The start-authorization gate is closed (CD-40, D-0058): the VMK is not
    /// in memory - either a vault exists (unlock) or none does yet (mandatory
    /// first-launch setup, CD-42). While true, only the lock view boots; the
    /// workspace follows after the unlock/setup outcome arrives.
    locked: bool,
    /// The lock view has been created (one-shot, the locked sibling of
    /// `views_started`).
    lock_view_started: bool,
    window: Option<Arc<Window>>,
    renderer: Option<SurfaceRenderer>,
    theme: Theme,
    start: Instant,
    /// Automatic identity-rotation cycle anchor (CD-29): the start of the current
    /// countdown. Reset on each rotation and whenever auto-rotation is (re)enabled.
    rot_anchor: Instant,
    /// A brief post-rotation flash deadline - drives the Pulse Grid re-roll burst.
    rot_flash_until: Option<Instant>,
    /// The red-transition burst deadline (CD-30 Task E): armed by
    /// [`Shell::trigger_red_transition`] when Red actually engages.
    red_flash_until: Option<Instant>,
    /// How many windows were at effective Red last pass - the truthful edge
    /// detector for the transition (fires only on an INCREASE, i.e. a window
    /// actually entering Red).
    red_slots: usize,
    cef_inited: bool,
    views_started: bool,
    scale: f32,
    cursor_phys: PhysicalPosition<f64>,
    key_mods: u32,
    mods: ModifiersState,
    button_flags: u32,
    applied_cursor: CursorIcon,
    overlay: Overlay,
    gear_hover: f32,
    gear_hover_target: f32,
    /// Info glyph hover glow (eased) + its target, and the eased "updates
    /// available" fraction (0 idle → 1 active) so the glyph fills in smoothly (CD-13).
    info_hover: f32,
    info_hover_target: f32,
    info_active: f32,
    /// Live slots in left-to-right display order, by stable id (an index into the
    /// fixed per-slot browser/texture arrays). Length 1..=MAX_SLOTS. A slot keeps
    /// its id for life, so its CEF handlers (which bake in `Role::Slot(id)`) and
    /// its texture never move; only its position in this list changes.
    order: Vec<usize>,
    /// The active slot id: keyboard input, the top bar and the scheme hint act on
    /// it. Always a member of `order`.
    active_slot: usize,
    /// The view the mouse was last over (a slot or the internal overlay), so a
    /// move onto another view sends a mouse-leave to the old one and the cursor
    /// icon is taken from the hovered view (CD-09 Stage C). `None` over a gutter
    /// / margin / the gear.
    mouse_role: Option<Role>,
    /// Per-slot loading-line intensity, eased toward on (loading) / off (done).
    loading: [f32; MAX_SLOTS],
    /// Per-slot width in units (1 or 2, CD-10). Indexed by slot id.
    width_units: [u32; MAX_SLOTS],
    /// Animated frame (CD-11): the on-screen (interpolated) rect per slot id, and
    /// the eased side zones. Rendering AND input read these - one per-frame
    /// geometry, so the reflow animation can never desync. `disp_rect[id]` is
    /// `None` until a slot's first frame (it then grows from a collapsed sliver).
    disp_rects: [Option<slots::Rect>; MAX_SLOTS],
    disp_left: slots::Rect,
    disp_right: slots::Rect,
    /// The eased width of the flexible LEFT (Spine) zone (D-0022). The right MF
    /// zone is permanent (its stepped CD-31 width), so only the left animates.
    disp_left_width: f32,
    /// False until the first frame snaps the animated frame to the target (so the
    /// startup layout does not animate in from zero).
    frame_inited: bool,
    applied_title: String,
    applied_topmost: bool,
    isolation_tested: bool,
    /// Internal-view size (device px) currently applied, so the resize in
    /// `about_to_wait` fires only when it changes.
    applied_internal: (u32, u32),
    /// The slot whose floating ensemble is currently engaged (revealed + driven),
    /// or `None` (CD-12). Follows the band hover / Ctrl+L; pushed to the page.
    engaged_slot: Option<usize>,
    /// Armed when the cursor leaves the engaged band's interactive region; the
    /// band disengages when it fires (hysteresis, reused from CD-08).
    bar_hide_at: Option<Instant>,
    /// After disengaging, keep the band composited until this instant so the
    /// page's fade-out completes, then finalise to `Closed` (CD-12).
    band_off_at: Option<Instant>,
    /// The last frame state pushed to the page, so a push fires only on change
    /// (target rects + engaged slot) - not per frame (the CD-11 IPC cadence).
    frame_sig: String,
    /// The last HUD state pushed (CD-30 Task B) - same on-change cadence.
    hud_sig: String,
    /// The Tor engine status last carried in a frame push (CD-23). The engine reaches
    /// READY on a BACKGROUND thread with no user action, so `about_to_wait` compares
    /// this against `tor::status()` and re-pushes the frame on a transition - otherwise
    /// the per-window anonymity indicator (cdFrame-driven) stays latched on the last
    /// pushed value ("Connecting") while Tor is actually Ready. `u8::MAX` = never pushed.
    tor_status_pushed: u8,
    /// An in-progress favorite-tile drag `(url, title)` (CD-12): the host owns it
    /// (ghost + drop zones) and slot views receive no mouse until it ends.
    drag: Option<(String, String)>,
}

/// One slot to spawn during boot/restore (CD-21): its id, and the URL to open -
/// `None` = the own start page (a Tor slot, or a clearnet slot with no saved URL),
/// `Some(url)` = reload that clearnet URL. The slot's mode/width/order are applied
/// to the `Shell` during the plan phase; this carries only what the spawn phase needs.
struct SlotPlan {
    id: usize,
    url: Option<String>,
}

fn window_hwnd(window: &Window) -> isize {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window
        .window_handle()
        .expect("failed to get window handle")
        .as_raw()
    {
        RawWindowHandle::Win32(handle) => handle.hwnd.get(),
        _ => panic!("expected a Win32 window handle"),
    }
}

/// Map a winit physical key to a Windows virtual-key code (enough for a search:
/// letters, digits, and the editing/navigation keys). Text itself is delivered
/// through CHAR events, so unmapped keys still type.
fn keycode_to_vk(code: KeyCode) -> i32 {
    use KeyCode::*;
    match code {
        KeyA => 0x41, KeyB => 0x42, KeyC => 0x43, KeyD => 0x44, KeyE => 0x45,
        KeyF => 0x46, KeyG => 0x47, KeyH => 0x48, KeyI => 0x49, KeyJ => 0x4A,
        KeyK => 0x4B, KeyL => 0x4C, KeyM => 0x4D, KeyN => 0x4E, KeyO => 0x4F,
        KeyP => 0x50, KeyQ => 0x51, KeyR => 0x52, KeyS => 0x53, KeyT => 0x54,
        KeyU => 0x55, KeyV => 0x56, KeyW => 0x57, KeyX => 0x58, KeyY => 0x59,
        KeyZ => 0x5A,
        Digit0 => 0x30, Digit1 => 0x31, Digit2 => 0x32, Digit3 => 0x33, Digit4 => 0x34,
        Digit5 => 0x35, Digit6 => 0x36, Digit7 => 0x37, Digit8 => 0x38, Digit9 => 0x39,
        Enter | NumpadEnter => 0x0D,
        Backspace => 0x08,
        Tab => 0x09,
        Space => 0x20,
        Delete => 0x2E,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Escape => 0x1B,
        _ => 0,
    }
}

impl Shell {
    /// The view keyboard input is currently routed to (internal when an overlay
    /// is open, else the active surf slot).
    fn active_role(&self) -> Role {
        if self.overlay == Overlay::Closed {
            Role::Slot(self.active_slot)
        } else {
            Role::Internal
        }
    }

    /// The live slots' width in units, in display order (CD-10).
    fn units_in_order(&self) -> Vec<u32> {
        self.order.iter().map(|&id| self.width_units[id]).collect()
    }

    /// The full target frame layout for a given surface size (CD-30/CD-31: one
    /// source of truth - includes the stepped MF width, column compression, and
    /// the red-mode viewport locks).
    fn frame_target(&self, w: u32, h: u32) -> slots::FrameLayout {
        slots::frame_layout(
            w,
            h,
            &self.units_in_order(),
            self.scale,
            &self.theme.slots,
            &self.red_locks(w, h),
        )
    }

    /// Per-display-position viewport locks (CD-30 Task D - the red "bunker"
    /// mode). A window whose EFFECTIVE Ampel level is Red snaps to a STANDARD
    /// viewport - its reported-screen preset (default 1920×1080), laddered DOWN
    /// (1600×900, 1280×720) to the largest standard size the frame can hold -
    /// and stays locked while Red is active (`toggle_active_width` refuses, the
    /// layout ignores its width units). With viewport == reported screen the
    /// window reads exactly like a fullscreen browser on an ordinary machine.
    /// Slots are allocated in display order (earlier locks count against later
    /// ones); if not even the smallest standard size fits this display, the slot
    /// stays zone-sized - the LEVEL and its vectors are unaffected, the lock is
    /// honestly recorded as the minor, presentational increment it is (D-0047).
    /// `width_units` are never modified, so stepping down from Red restores the
    /// user's previous layout by construction.
    fn red_locks(&self, w: u32, h: u32) -> Vec<Option<(f32, f32)>> {
        let t = &self.theme.slots;
        let (_, zh) = slots::zone_vertical(h, self.scale, t);
        let g = (t.gutter * self.scale).round();
        // CD-32 Task A (D-0049): while ANY window is at Red, protection outranks
        // the MF zone - it yields to its small step so the ladder below can reach
        // the largest common resolution the display holds (1920×1080 from ~2400px
        // wide, where CD-31's nominal step capped the same display at 1600×900).
        // `frame_layout` re-derives the same yield from the locks this returns, so
        // the two never disagree; the zone springs back when Red is released.
        let red_active = self
            .order
            .iter()
            .any(|&id| browser::slot_effective_level(id) == crate::harden::Level::Red);
        let mf = slots::mf_step_width(
            w,
            slots::nominal_group_width(&self.units_in_order(), self.scale, t),
            self.scale,
            t,
            red_active,
        );
        let rail = (t.side_rail_width * self.scale).round();
        let floor = (t.slot_min_width * self.scale).round();
        let base_avail = w as f32 - rail - mf - 2.0 * g - g * (self.order.len() as f32 - 1.0);
        let mut taken = 0.0f32; // width already claimed by earlier locked slots
        let mut floored_before = 0usize; // earlier columns assumed at the floor
        let mut locks = Vec::with_capacity(self.order.len());
        for (p, &id) in self.order.iter().enumerate() {
            if browser::slot_effective_level(id) != crate::harden::Level::Red {
                floored_before += 1;
                locks.push(None);
                continue;
            }
            // Every column not yet allocated is assumed at its compression
            // floor; earlier locked columns count at their picked size.
            let others = self.order.len() - 1 - p + floored_before;
            let avail = base_avail - taken - others as f32 * floor;
            let mut pick = None;
            let mut code = browser::slot_effective_screen_code(id).min(2) as i32;
            while code >= 0 {
                let (cw, ch) = settings::screen_dims(code as u8);
                let (dw, dh) = ((cw as f32 * self.scale).round(), (ch as f32 * self.scale).round());
                if dw <= avail && dh <= zh {
                    pick = Some((dw, dh));
                    break;
                }
                code -= 1;
            }
            match pick {
                Some((dw, _)) => taken += dw,
                None => floored_before += 1,
            }
            locks.push(pick);
        }
        locks
    }

    /// The current slot rectangles (device px, one per live column in display
    /// order) for a given surface size, honoring each slot's width units. Since
    /// CD-30 these are the FRAME rects (zone-shifted, possibly compressed), so
    /// the band/ensemble math and the displayed columns can never disagree.
    fn slot_rects_wh(&self, w: u32, h: u32) -> Vec<slots::Rect> {
        self.frame_target(w, h).slots
    }

    /// The display position (index into `order`) of the active slot.
    fn active_position(&self) -> usize {
        self.order
            .iter()
            .position(|&id| id == self.active_slot)
            .unwrap_or(0)
    }

    /// The animated on-screen rect for slot id `id` (CD-11). Rendering, mouse
    /// hit-testing and the bar all read this, so they never disagree during a
    /// reflow. Falls back to the settled target rect before the first frame.
    fn disp_rect(&self, id: usize) -> slots::Rect {
        self.disp_rects[id].unwrap_or_else(|| {
            let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
            let rects = self.slot_rects_wh(w, h);
            self.order
                .iter()
                .position(|&i| i == id)
                .and_then(|p| rects.get(p).copied())
                .unwrap_or_default()
        })
    }

    /// The animated slot rects in display order.
    fn disp_slots(&self) -> Vec<slots::Rect> {
        self.order.iter().map(|&id| self.disp_rect(id)).collect()
    }

    /// The slot id + animated rect whose rectangle contains the cursor, if any
    /// (mouse routing). Slots never overlap, so the first hit is the one.
    fn slot_at_cursor(&self) -> Option<(usize, slots::Rect)> {
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        self.order.iter().find_map(|&id| {
            let r = self.disp_rect(id);
            r.contains(cx, cy).then_some((id, r))
        })
    }

    /// Top-left origin (device px) of a role's rectangle (a slot's animated rect,
    /// or the internal overlay's).
    fn origin_of_role(&self, role: Role) -> (f32, f32) {
        match role {
            Role::Internal => {
                let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
                let (x, y, _, _) = self.internal_rect(w, h);
                (x, y)
            }
            Role::Slot(id) => {
                let r = self.disp_rect(id);
                (r.x, r.y)
            }
            // The MF-zone view's origin is the animated right-zone top-left (CD-18).
            Role::MfZone => (self.disp_right.x, self.disp_right.y),
            // The HUD strip's origin is its (target-anchored) top-left (CD-30).
            Role::Hud => {
                let r = self.hud_rect();
                (r.0, r.1)
            }
        }
    }

    /// The floating HUD strip's rect (device px, CD-30 Task B): the top margin
    /// strip from the MF zone's left edge to the window's right edge (the gear /
    /// info glyphs draw over its transparent right corner). Anchored to the
    /// TARGET frame (not the eased one) so the texture and the quad always agree.
    fn hud_rect(&self) -> (f32, f32, f32, f32) {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (zy, _) = slots::zone_vertical(h, self.scale, &self.theme.slots);
        let x = self.frame_target(w, h).right.x;
        (x, 0.0, (w as f32 - x).max(1.0), zy)
    }

    /// Advance the animated frame one step toward the target [`slots::frame_layout`]
    /// for the current slots (CD-11). Called once per frame; the eased result is
    /// what both rendering and input read.
    fn update_frame(&mut self) {
        let Some((w, h)) = self.renderer.as_ref().map(|r| r.size()) else {
            return;
        };
        let target = self.frame_target(w, h);
        let g = self.theme.slots.gutter * self.scale;

        if !self.frame_inited {
            for (p, &id) in self.order.iter().enumerate() {
                self.disp_rects[id] = Some(target.slots[p]);
            }
            self.disp_left = target.left;
            self.disp_right = target.right;
            self.disp_left_width = target.left_width;
            self.frame_inited = true;
            return;
        }

        // Ease each live slot toward its target rect; a slot with no animated rect
        // yet (freshly added) grows from a collapsed sliver at its target center.
        for (p, &id) in self.order.iter().enumerate() {
            let tr = target.slots[p];
            let cur = self.disp_rects[id].unwrap_or(slots::Rect {
                x: tr.x + tr.w * 0.5,
                y: tr.y,
                w: 0.0,
                h: tr.h,
            });
            self.disp_rects[id] = Some(ease_rect(cur, tr, FRAME_EASE));
        }

        // Ease the flexible LEFT (Spine) zone width; derive both zone rects from
        // the animated group bounds so they glide with the columns. The RIGHT MF
        // zone is permanent - its width is the constant target (no easing), it
        // only follows the group's right edge (D-0022).
        self.disp_left_width += (target.left_width - self.disp_left_width) * FRAME_EASE;
        let mf_width = target.right.w;
        let first = self.order[0];
        let last = *self.order.last().expect("order is non-empty");
        let gl = self.disp_rects[first].map(|r| r.x).unwrap_or(target.left.x);
        let gr = self.disp_rects[last]
            .map(|r| r.x + r.w)
            .unwrap_or(target.right.x);
        self.disp_left = slots::Rect {
            x: gl - g - self.disp_left_width,
            y: target.left.y,
            w: self.disp_left_width,
            h: target.left.h,
        };
        self.disp_right = slots::Rect {
            x: gr + g,
            y: target.right.y,
            w: mf_width,
            h: target.right.h,
        };
    }

    /// The view the mouse currently routes to and its origin: the internal
    /// overlay when the cursor is over its visible rect, otherwise the slot under
    /// the cursor. `None` over a gutter / margin (no page there).
    fn mouse_target(&self) -> Option<(Role, (f32, f32))> {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        // The HUD strip (CD-30) is interactive in EVERY overlay state - the Ampel
        // must stay reachable. Its rect (the top margin strip above/right of the
        // MF zone) overlaps neither the slots, the band columns, the launcher,
        // nor the MF zone, so the check is order-independent; the shell-drawn
        // gear / info glyphs consume their clicks before routing ever runs.
        let hud = self.hud_rect();
        let over_hud = self.point_in(hud);
        match self.overlay {
            Overlay::Settings | Overlay::Info => {
                let (x, y, pw, ph) = self.internal_rect(w, h);
                if cx >= x && cx <= x + pw && cy >= y && cy <= y + ph {
                    Some((Role::Internal, (x, y)))
                } else if over_hud {
                    Some((Role::Hud, (hud.0, hud.1)))
                } else {
                    None
                }
            }
            // The lock gate (CD-40): only the lock card is interactive - no HUD,
            // no MF zone, no slots exist while the gate is closed.
            Overlay::Lock => {
                let (x, y, pw, ph) = self.internal_rect(w, h);
                if cx >= x && cx <= x + pw && cy >= y && cy <= y + ph {
                    Some((Role::Internal, (x, y)))
                } else {
                    None
                }
            }
            Overlay::Command => {
                // Over the engaged ensemble's band column, or the launcher strip ->
                // the transparent band (internal view, origin at the window origin);
                // over the MF zone -> its content view (tab clicks); elsewhere the
                // slot under the cursor (so another column's gap can engage it, and
                // slots stay usable). CD-12 / CD-18.
                let over_ensemble = self.engaged_band_rect().map(|r| self.point_in(r)).unwrap_or(false);
                if over_ensemble || self.point_in(self.launcher_rect()) {
                    Some((Role::Internal, (0.0, 0.0)))
                } else if over_hud {
                    Some((Role::Hud, (hud.0, hud.1)))
                } else if self.disp_right.contains(cx, cy) {
                    Some((Role::MfZone, (self.disp_right.x, self.disp_right.y)))
                } else {
                    self.slot_at_cursor().map(|(id, r)| (Role::Slot(id), (r.x, r.y)))
                }
            }
            // The permanent MF-zone content view takes clicks over its rect (tab
            // switching / scrolling); otherwise the slot under the cursor (CD-18).
            Overlay::Closed => {
                if over_hud {
                    Some((Role::Hud, (hud.0, hud.1)))
                } else if self.disp_right.contains(cx, cy) {
                    Some((Role::MfZone, (self.disp_right.x, self.disp_right.y)))
                } else {
                    self.slot_at_cursor().map(|(id, r)| (Role::Slot(id), (r.x, r.y)))
                }
            }
        }
    }

    /// The total slot-unit budget the frame can hold at the current width - the
    /// rail-state center budget (CD-11), so slots are capped against the maximum
    /// the frame will ever fit.
    fn capacity(&self) -> usize {
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        slots::frame_capacity(w, self.scale, &self.theme.slots)
    }

    /// The product slot-count maximum (D-0022): the `slots.slot_max` token,
    /// clamped to the `MAX_SLOTS` array ceiling. Caps `order.len()` (Ctrl+T,
    /// open-in-new-slot, drag-into-gutter, restore).
    fn slot_max(&self) -> usize {
        (self.theme.slots.slot_max as usize).clamp(1, MAX_SLOTS)
    }

    /// Make slot id `id` the active slot: move CEF keyboard focus (only while no
    /// overlay is open - otherwise the internal view holds focus) and update the
    /// browser-side active-slot pointer the top bar / IPC read.
    fn set_active(&mut self, id: usize) {
        if !self.order.contains(&id) || id == self.active_slot {
            return;
        }
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), false);
        }
        self.active_slot = id;
        browser::set_active_slot(id);
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(id), true);
        }
    }

    /// Focus the slot in display position `pos1` (1-based, Ctrl+1..4). No-op if
    /// there is no slot at that position.
    fn focus_slot_position(&mut self, pos1: usize) {
        if pos1 >= 1
            && pos1 <= self.order.len()
            && let Some(&id) = self.order.get(pos1 - 1)
        {
            self.set_active(id);
        }
    }

    /// Cycle the active slot forward / backward (Ctrl+Tab / Ctrl+Shift+Tab).
    fn cycle_active(&mut self, forward: bool) {
        if self.order.len() <= 1 {
            return;
        }
        let next = slots::cycle_position(self.active_position(), self.order.len(), forward);
        self.set_active(self.order[next]);
    }

    /// Add a slot right of the active one (Ctrl+T). No-op at capacity / slot_max.
    /// The new slot spawns at the own start page (CD-14) and becomes active - its
    /// own search box is the landing surface (so the CD-12 Ctrl+T capsule
    /// auto-reveal is retired here; Ctrl+L still reveals the floating capsule).
    fn add_slot(&mut self) {
        // A new slot is one unit; it must fit both the count and the unit budget.
        if self.order.len() >= self.slot_max() || self.total_units() + 1 > self.capacity() as u32 {
            return;
        }
        let Some(free) = slots::free_id(&self.order) else {
            return;
        };
        // Drop keyboard focus from the outgoing active slot before the new one
        // spawns (its start page then takes focus via on_after_created).
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), false);
        }
        let pos = slots::insert_position(&self.order, self.active_slot);
        self.order.insert(pos, free);
        self.loading[free] = 0.0;
        self.width_units[free] = 1;
        // Set the new slot's mode explicitly (so a reused id never inherits a closed
        // Tor slot's stale mode): the "Tor for new windows" default (CD-15 Stage C),
        // else clearnet.
        let tor = settings::tor_default();
        if tor {
            crate::tor::init();
        }
        browser::set_slot_tor(free, tor);
        self.active_slot = free;
        browser::set_active_slot(free);
        // Recentre the group and re-size every view, then spawn the new slot at the
        // own start page (Energy Core + search + favorites). The lazy-slot
        // placeholder covers the brief spawn until the start page paints.
        self.push_geometry();
        if let Some(window) = self.window.clone() {
            let hwnd = window_hwnd(&window);
            browser::create_browser(Role::Slot(free), hwnd);
        }
        self.notify_all_resized();
    }

    /// Flip slot `id` between clearnet and Tor (CD-15 Stage B): start the Tor engine
    /// if needed, set the slot's mode, then tear its browser down and respawn it
    /// under the new request context at the start page - a fresh identity, no state
    /// bleed. Other slots are untouched (per-window switching, per-CefRequestContext).
    fn toggle_tor(&mut self, id: usize) {
        if !self.order.contains(&id) {
            return;
        }
        let now_tor = !browser::slot_is_tor(id);
        tracing::info!(slot = id, to_tor = now_tor, "toggle_tor: begin");
        // The engine master switch (CD-15 Stage C) gates turning Tor ON; a slot can
        // always be reverted to clearnet even if the engine is disabled.
        if now_tor && !settings::tor_enabled() {
            tracing::info!(slot = id, "toggle_tor: engine disabled in settings - no-op");
            return;
        }
        if now_tor {
            crate::tor::init(); // idempotent - ensure the engine is bootstrapping
        }
        browser::set_slot_tor(id, now_tor);
        // Respawn the slot's browser under the new context (read from the mode). This
        // is NON-blocking: a Tor slot's browser is created immediately (posted to the
        // CEF UI thread), NOT gated on bootstrap - it just cannot fetch until arti is
        // ready. The UI never waits on Tor (CD-15 HOTFIX).
        if let Some(window) = self.window.clone() {
            browser::close_slot(id);
            if let Some(r) = self.renderer.as_mut() {
                r.clear_slot(id);
            }
            self.loading[id] = 0.0;
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser(Role::Slot(id), hwnd); // → cyberdesk://start/
        }
        // Reflect the new mode/status on the glyph.
        self.push_frame(false);
        tracing::info!(slot = id, "toggle_tor: end (respawn requested, returned immediately)");
    }

    /// Switch slot `id` to Tor and load `url` there (CD-35): the onion refusal
    /// page's "switch this window to Tor". [`toggle_tor`]'s teardown/respawn -
    /// same fresh-identity, no-state-bleed semantics - except the new browser
    /// spawns at the requested URL instead of the start page, and the direction
    /// is fixed (always → Tor; a slot already on Tor just navigates). The Tor
    /// master switch was checked by the IPC that queued this.
    fn switch_slot_to_tor_url(&mut self, id: usize, url: &str) {
        if !self.order.contains(&id) {
            return;
        }
        if browser::slot_is_tor(id) {
            browser::load_url(Role::Slot(id), url);
            return;
        }
        tracing::info!(slot = id, "switch_slot_to_tor_url: begin");
        crate::tor::init(); // idempotent - ensure the engine is bootstrapping
        browser::set_slot_tor(id, true);
        if let Some(window) = self.window.clone() {
            browser::close_slot(id);
            if let Some(r) = self.renderer.as_mut() {
                r.clear_slot(id);
            }
            self.loading[id] = 0.0;
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser_url(Role::Slot(id), hwnd, url);
        }
        self.push_frame(false);
    }

    /// Respawn slot `id`'s browser at its CURRENT url (CD-25 / CD-29): the fresh
    /// document picks up the new effective fingerprint config - hardening vectors AND
    /// the reported screen preset - which a live context can't adopt (the patches are
    /// irreversible / `screen_info` is read at create time). Mirrors [`toggle_tor`]'s
    /// respawn, but RELOADS the current page (not the start page): a fingerprint-config
    /// change is not a network-identity change, so the user stays on their page. The
    /// per-slot override / global preset was already updated before this runs;
    /// `create_browser_url` reads the (unchanged) Tor mode, so a Tor slot stays Tor.
    fn respawn_slot_preserving_url(&mut self, id: usize) {
        if !self.order.contains(&id) {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        let url = browser::slot_url(id);
        browser::close_slot(id);
        if let Some(r) = self.renderer.as_mut() {
            r.clear_slot(id);
        }
        self.loading[id] = 0.0;
        self.push_geometry();
        let hwnd = window_hwnd(&window);
        if url.starts_with("http://") || url.starts_with("https://") {
            browser::create_browser_url(Role::Slot(id), hwnd, &url);
        } else {
            browser::create_browser(Role::Slot(id), hwnd); // → cyberdesk://start/
        }
        self.push_frame(false);
    }

    /// Open `url` in a new slot beside the source slot - a user-gesture popup or
    /// a Ctrl-/middle-click on a link (D-0018). The new slot is one unit, spawns
    /// immediately with the URL, and becomes active. If the grid has no room, fall
    /// back to the CD-04 behavior: navigate the source slot in place.
    fn open_in_new_slot(&mut self, source_id: usize, url: String) {
        // FAIL-CLOSED (CD-15, D-0027): a link opened from a Tor slot must STAY on
        // Tor - the new slot inherits the source's mode. (The no-room fallback
        // navigates the source's own browser in place, so it keeps the source's
        // mode already.)
        self.open_in_new_slot_mode(source_id, url, browser::slot_is_tor(source_id));
    }

    /// [`open_in_new_slot`] with the new slot's Tor mode set EXPLICITLY (CD-35):
    /// the onion refusal page opens a `.onion` from a CLEARNET source in a new
    /// TOR window, so "inherit the source's mode" is exactly wrong there. The
    /// no-room fallback differs per mode: same-mode falls back to navigating the
    /// source in place (CD-04), while a forced-Tor open must NOT (a clearnet
    /// slot would just refuse the `.onion` again) - it switches the source slot
    /// to Tor with the URL instead.
    fn open_in_new_slot_mode(&mut self, source_id: usize, url: String, tor: bool) {
        let has_room = self.order.len() < self.slot_max() && self.total_units() < self.capacity() as u32;
        let Some(free) = slots::free_id(&self.order).filter(|_| has_room) else {
            if tor && !browser::slot_is_tor(source_id) {
                self.switch_slot_to_tor_url(source_id, &url);
            } else {
                browser::load_url(Role::Slot(source_id), &url);
            }
            return;
        };
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), false);
        }
        // Insert right of the source slot (the same tested helper add_slot uses,
        // with the source id instead of the active id).
        let pos = slots::insert_position(&self.order, source_id);
        self.order.insert(pos, free);
        self.width_units[free] = 1;
        self.loading[free] = 0.0;
        self.active_slot = free;
        browser::set_active_slot(free);
        // Set the mode BEFORE the browser is created - create_browser_url reads
        // slot_is_tor to pick the request context (CD-15, D-0027).
        if tor {
            crate::tor::init();
        }
        browser::set_slot_tor(free, tor);
        // CD-25 / CD-29: a popup inherits the source window's hardening AND screen
        // overrides (so a Strict / 720p window's popups match it), read BEFORE the
        // browser is created.
        browser::set_slot_hardening(free, browser::slot_hardening_override(source_id));
        browser::set_slot_screen(free, browser::slot_screen_override(source_id));
        if let Some(window) = self.window.clone() {
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser_url(Role::Slot(free), hwnd, &url);
        }
        self.notify_all_resized();
    }

    /// Insert a new lazy slot at display position `pos`, spawn it with `url`, and
    /// make it active (CD-12 drag drop; shared with open-in-new-slot). No-op if no
    /// free id.
    fn insert_slot_at(&mut self, pos: usize, url: &str) {
        let Some(free) = slots::free_id(&self.order) else {
            return;
        };
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), false);
        }
        let pos = pos.min(self.order.len());
        self.order.insert(pos, free);
        self.width_units[free] = 1;
        self.loading[free] = 0.0;
        self.disp_rects[free] = None;
        // Set the mode explicitly (a reused id must not inherit a closed Tor slot's
        // stale mode): the "Tor for new windows" default (CD-15), else clearnet.
        let tor = settings::tor_default();
        if tor {
            crate::tor::init();
        }
        browser::set_slot_tor(free, tor);
        self.active_slot = free;
        browser::set_active_slot(free);
        if let Some(window) = self.window.clone() {
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser_url(Role::Slot(free), hwnd, url);
        }
        self.notify_all_resized();
    }

    /// The control-gutter drop zones (CD-12): a gutter-wide bar before slot 0,
    /// between each pair, and after the last slot - paired with the display
    /// position a drop there inserts at (0..=n).
    fn gutter_drops(&self) -> Vec<(usize, slots::Rect)> {
        let (w, h) = match self.renderer.as_ref().map(|r| r.size()) {
            Some(s) => s,
            None => return Vec::new(),
        };
        let rects = self.slot_rects_wh(w, h);
        if rects.is_empty() {
            return Vec::new();
        }
        let g = self.theme.slots.gutter * self.scale;
        let (sy, sh) = (rects[0].y, rects[0].h);
        let mut out = Vec::with_capacity(rects.len() + 1);
        out.push((0, slots::Rect { x: rects[0].x - g, y: sy, w: g, h: sh }));
        for p in 1..rects.len() {
            let left = rects[p - 1].x + rects[p - 1].w;
            out.push((p, slots::Rect { x: left, y: sy, w: g, h: sh }));
        }
        let last = rects[rects.len() - 1];
        out.push((rects.len(), slots::Rect { x: last.x + last.w, y: sy, w: g, h: sh }));
        out
    }

    /// Index into `gutters` nearest the cursor x.
    fn nearest_gutter(&self, gutters: &[(usize, slots::Rect)]) -> usize {
        let cx = self.cursor_phys.x as f32;
        gutters
            .iter()
            .enumerate()
            .min_by(|a, b| {
                let da = (a.1.1.x + a.1.1.w * 0.5 - cx).abs();
                let db = (b.1.1.x + b.1.1.w * 0.5 - cx).abs();
                da.total_cmp(&db)
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Whether the frame has room for one more column (drop-into-gutter allowed).
    fn drag_has_room(&self) -> bool {
        self.order.len() < self.slot_max() && self.total_units() < self.capacity() as u32
    }

    /// The drag overlay quads for the current drag (CD-12): the drop-zone gutter
    /// bars (nearest hot) with room, else a highlight on the slot under the cursor,
    /// plus the glowing ghost circle at the cursor. Empty if not dragging.
    fn drag_quads(&self) -> Vec<renderer::DragQuad> {
        if self.drag.is_none() {
            return Vec::new();
        }
        let b = crate::theme::hex3(&self.theme.colors.brand);
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        let mut out = Vec::new();
        if self.drag_has_room() {
            let gutters = self.gutter_drops();
            let hot = self.nearest_gutter(&gutters);
            for (i, (_pos, r)) in gutters.iter().enumerate() {
                let is_hot = i == hot;
                let a = if is_hot { 0.6 } else { 0.16 };
                let glow = (if is_hot { 16.0 } else { 6.0 }) * self.scale;
                out.push(renderer::DragQuad {
                    rect: (r.x, r.y, r.w, r.h),
                    color: [b[0], b[1], b[2], a],
                    radius: r.w * 0.5,
                    glow,
                    kind: 0,
                });
            }
        } else if let Some((_, r)) = self.slot_at_cursor() {
            // Full grid: dropping over a slot navigates it - hint by glowing it.
            out.push(renderer::DragQuad {
                rect: (r.x, r.y, r.w, r.h),
                color: [b[0], b[1], b[2], 0.16],
                radius: self.theme.page.corner_radius,
                glow: 10.0 * self.scale,
                kind: 0,
            });
        }
        // The ghost: a glowing brand circle at the cursor.
        let gs = 40.0 * self.scale;
        out.push(renderer::DragQuad {
            rect: (cx - gs * 0.5, cy - gs * 0.5, gs, gs),
            color: [b[0], b[1], b[2], 0.85],
            radius: gs * 0.5,
            glow: 13.0 * self.scale,
            kind: 0,
        });
        out
    }

    /// Finish a drag by releasing the mouse: drop into the nearest gutter (insert
    /// + spawn) with room, else navigate the slot under the cursor (CD-12).
    fn drop_favorite(&mut self) {
        let Some((url, _title)) = self.drag.take() else {
            return;
        };
        if self.drag_has_room() {
            let gutters = self.gutter_drops();
            if !gutters.is_empty() {
                let pos = gutters[self.nearest_gutter(&gutters)].0;
                self.insert_slot_at(pos, &url);
            }
        } else if let Some((id, _)) = self.slot_at_cursor() {
            browser::navigate_slot(id, &url);
        }
    }

    /// Cancel an in-progress drag (ESC) - no drop.
    fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// Close the active slot (Ctrl+W). The last slot cannot be closed. The
    /// browser shuts down cleanly, the group recenters, and the nearest neighbor
    /// becomes active.
    fn close_active_slot(&mut self) {
        let pos = self.active_position();
        self.close_slot_at(pos);
    }

    /// Close the slot at display position `pos` (Ctrl+W on the active slot, or a
    /// click on that slot's floating close orb - CD-12). The last slot refuses to
    /// close; a closed active slot promotes a neighbor. The frame then reflows.
    fn close_slot_at(&mut self, pos: usize) {
        if self.order.len() <= 1 || pos >= self.order.len() {
            return;
        }
        let closed_active = self.order[pos] == self.active_slot;
        let id = self.order.remove(pos);
        browser::close_slot(id);
        if let Some(r) = self.renderer.as_mut() {
            r.clear_slot(id);
        }
        self.loading[id] = 0.0;
        self.width_units[id] = 1;
        self.disp_rects[id] = None;
        // CD-25 / CD-29: slot ids are reused; clear any per-window hardening AND
        // screen overrides so a reused id starts fresh (inheriting the global), never
        // carrying a closed window's override. (Respawns use `close_slot`, not this
        // path, so an override survives a hardening/Tor/screen respawn.)
        browser::set_slot_hardening(id, None);
        browser::set_slot_screen(id, None);
        browser::clear_slot_identity(id);
        // Promote a neighbor only if the active slot itself was the one closed;
        // closing a non-active slot (via its orb) leaves the active slot as is.
        if closed_active {
            let new_pos = slots::neighbor_position(pos, self.order.len());
            self.active_slot = self.order[new_pos];
            browser::set_active_slot(self.active_slot);
            if self.overlay == Overlay::Closed {
                browser::set_focus(Role::Slot(self.active_slot), true);
            }
        }
        self.push_geometry();
        self.notify_all_resized();
    }

    // The CD-12 floating per-slot close orb (a shell-drawn corner-hover ring+cross)
    // was RETIRED in CD-18: closing a window is now an explicit in-page close icon
    // beside each ensemble's address capsule (command.js `.close-btn` →
    // `close_slot` IPC → `take_pending_closes` → `close_slot_at`). The last-slot
    // refusal + neighbor promotion still live in `close_slot_at` - the single choke
    // point for the icon, Ctrl+W, and resize-driven drops alike.

    /// Swap the active slot with its neighbor (Ctrl+Shift+Left/Right). A pure
    /// order operation - the active slot keeps its id (and its browser/texture),
    /// only its display position changes; no browser moves and no view resizes
    /// (widths are unchanged), so the compositor picks up the new positions next
    /// frame. A hard swap (no slide animation) - see D-0019. No-op at the edge.
    fn swap_active(&mut self, dir: i32) {
        let pos = self.active_position();
        let target = pos as i32 + dir;
        if target < 0 || target as usize >= self.order.len() {
            return;
        }
        self.order.swap(pos, target as usize);
    }

    /// Toggle the active slot between 1 and 2 width units (Ctrl+Shift+D). Doubling
    /// adds one unit and is a no-op if it would overflow the width capacity;
    /// halving always works. Since CD-31 a nominal-layout change can also step
    /// the MF zone (and compress neighbors), so EVERY view is re-notified, not
    /// just the toggled slot.
    fn toggle_active_width(&mut self) {
        let id = self.active_slot;
        // CD-30 Task D: while a window is at Red its size is LOCKED - resizing
        // is refused until the level steps down (through the gate). Outside Red,
        // sizing stays completely free.
        if browser::slot_effective_level(id) == crate::harden::Level::Red {
            tracing::info!(slot = id, "red mode: size is locked; step down to resize");
            return;
        }
        if self.width_units[id] == 2 {
            self.width_units[id] = 1;
        } else if self.total_units() < self.capacity() as u32 {
            self.width_units[id] = 2;
        } else {
            return; // doubling would overflow - no-op
        }
        self.push_geometry();
        self.notify_all_resized();
    }

    /// The internal view's rectangle (device px) for the current overlay: the
    /// full-width transparent command band for `Command` (CD-12), the floating
    /// top-right card for `Info` (CD-13), the centered card for `Settings`.
    fn internal_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        match self.overlay {
            Overlay::Command => (0.0, 0.0, w as f32, self.band_height()),
            Overlay::Info => self.info_rect(w, h),
            _ => panel_rect(w, h),
        }
    }

    /// The command band height in device px (a fixed token band; the ensembles
    /// float within it).
    fn band_height(&self) -> f32 {
        (self.theme.command.band_height * self.scale).round()
    }

    /// The slot whose floating-ensemble band segment the cursor is over - the top
    /// gap above a slot, within its x-range. Drives which ensemble engages.
    fn band_hot_slot(&self) -> Option<usize> {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let rects = self.slot_rects_wh(w, h);
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        let top = rects.first().map(|r| r.y).unwrap_or(0.0);
        if cy < 0.0 || cy >= top {
            return None;
        }
        self.order
            .iter()
            .enumerate()
            .find(|&(p, _)| cx >= rects[p].x && cx <= rects[p].x + rects[p].w)
            .map(|(_, &id)| id)
    }

    /// The engaged ensemble's interaction rect (device px): the band column above
    /// the engaged slot, where its capsule / orbs / suggestions live.
    fn engaged_band_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let s = self.engaged_slot?;
        let rects = self.slot_rects_wh(w, h);
        let p = self.order.iter().position(|&id| id == s)?;
        let r = rects[p];
        Some((r.x, 0.0, r.w, self.band_height()))
    }

    /// The shared favorites launcher's interaction rect (device px): a centered
    /// top strip covering the tile row.
    fn launcher_rect(&self) -> (f32, f32, f32, f32) {
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let c = &self.theme.command;
        let tiles = c.max_results.max(1) as f32;
        let lw = (tiles * (c.tile_size + c.tile_gap) * self.scale).min(w as f32 * 0.7);
        let lh = (c.launcher_top + c.tile_size + 8.0) * self.scale;
        ((w as f32 - lw) * 0.5, 0.0, lw, lh)
    }

    fn point_in(&self, r: (f32, f32, f32, f32)) -> bool {
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        cx >= r.0 && cx <= r.0 + r.2 && cy >= r.1 && cy <= r.1 + r.3
    }

    /// Engage slot `s`'s floating ensemble: reveal it and bind the band to it.
    /// `autofocus` focuses its capsule (Ctrl+L). Opens the band on first engage.
    fn engage(&mut self, s: usize, autofocus: bool) {
        let first = self.overlay != Overlay::Command;
        self.engaged_slot = Some(s);
        self.bar_hide_at = None;
        self.band_off_at = None;
        if first {
            self.overlay = Overlay::Command;
            if let Some(r) = self.renderer.as_ref() {
                let (w, h) = r.size();
                let (_, _, iw, ih) = self.internal_rect(w, h);
                browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
                browser::notify_resized(Role::Internal);
                self.applied_internal = (iw as u32, ih as u32);
            }
            browser::show_internal_command();
            browser::set_focus(Role::Slot(self.active_slot), false);
            browser::set_focus(Role::Internal, true);
        }
        self.push_frame(autofocus);
    }

    /// Disengage the band (cursor left / committed navigation / ESC): hide every
    /// ensemble and start the compositing linger so the fade-out completes.
    fn disengage(&mut self) {
        if self.engaged_slot.is_none() && self.overlay != Overlay::Command {
            return;
        }
        self.engaged_slot = None;
        self.bar_hide_at = None;
        self.band_off_at = Some(Instant::now() + BAND_FADE_LINGER);
        self.push_frame(false);
    }

    /// Build and push the frame state to the page when it changes (engaged slot or
    /// target slot rects). Band-local DIP coordinates (band origin = window
    /// origin). Pushed on change only - the page glides via CSS (CD-11 cadence).
    fn push_frame(&mut self, autofocus: bool) {
        use std::fmt::Write;
        let (w, h) = match self.renderer.as_ref().map(|r| r.size()) {
            Some(s) => s,
            None => return,
        };
        let rects = self.slot_rects_wh(w, h);
        let scale = self.scale as f64;
        // Cheap change signature FIRST (computed every frame): the engaged slot +
        // each slot's band-DIP x/w. It EXCLUDES autofocus (a transient) so a
        // per-frame push(false) can't overwrite a pending Ctrl+L focus intent
        // before the page (which pulls get_frame on load) consumes it.
        // The Tor engine status also drives the glyph, so it is part of the sig
        // (a bootstrapping→ready transition re-pushes while the band is up, CD-15).
        let tor_status = crate::tor::status();
        // Record the status this push reflects (CD-23): whether we push now or
        // early-return on an unchanged sig, consumers hold `tor_status` afterwards
        // (it is part of the sig, so an unchanged sig means an unchanged status), so
        // `about_to_wait` won't redundantly re-push for the same value.
        self.tor_status_pushed = tor_status;
        // Per-slot hardening view (CD-25; Ampel-graded CD-30): the effective level
        // code (0=off, 1=green, 2=yellow, 3=red, 4=custom), whether it is INHERITED
        // (vs a per-window override), and whether it is REDUCED below the safe
        // Green floor (off / a dropped Green-core vector - Green itself is a
        // first-class safe level, never a warning). Used for both the change-sig
        // and the payload so a level change re-pushes.
        let hview = |id: usize| -> (u8, bool, bool) {
            let ov = browser::slot_hardening_override(id);
            let level = ov.map(|o| o.level).unwrap_or_else(crate::settings::hardening_level);
            let code = match level {
                crate::harden::Level::Off => 0u8,
                crate::harden::Level::Green => 1,
                crate::harden::Level::Yellow => 2,
                crate::harden::Level::Red => 3,
                crate::harden::Level::Custom => 4,
            };
            let reduced = crate::harden::is_weakening(
                &crate::harden::Config::GREEN,
                &browser::slot_effective_config(id),
            );
            (code, ov.is_none(), reduced)
        };
        let mut sig = format!("{:?}#{tor_status}", self.engaged_slot);
        for (p, &id) in self.order.iter().enumerate() {
            let (fp, inh, red) = hview(id);
            let _ = write!(
                sig,
                ";{}:{},{},{},{},{},{}",
                id,
                (rects[p].x as f64 / scale).round(),
                (rects[p].w as f64 / scale).round(),
                browser::slot_is_tor(id) as u32,
                fp,
                inh as u32,
                red as u32
            );
        }
        if !autofocus && sig == self.frame_sig {
            return; // nothing changed - no IPC (the CD-11 on-change cadence)
        }
        self.frame_sig = sig;
        // Build + push only on a real change.
        let slots: Vec<serde_json::Value> = self
            .order
            .iter()
            .enumerate()
            .map(|(p, &id)| {
                let (fp, inh, red) = hview(id);
                serde_json::json!({
                    "id": id,
                    "x": (rects[p].x as f64 / scale).round(),
                    "w": (rects[p].w as f64 / scale).round(),
                    "tor": browser::slot_is_tor(id),
                    "fp": fp,
                    "fp_inherited": inh,
                    "fp_reduced": red,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "slots": slots,
            "engaged": self.engaged_slot,
            "autofocus": autofocus,
            "tor_status": tor_status,
        })
        .to_string();
        browser::set_frame_state(&payload);
    }

    /// Build and push the HUD state (CD-30 Task B) when it changes. Same on-change
    /// cadence as `push_frame` - the signature excludes the continuously-moving
    /// countdown/age milliseconds (the page ticks those locally off absolute
    /// anchors) but includes the rotation EPOCH, so a re-roll re-anchors the page
    /// exactly when it lands. Called once per loop pass; a couple of atomic reads
    /// when nothing changed. Every pushed field is a real live value (rule 0.1).
    fn push_hud(&mut self) {
        let level = crate::settings::hardening_level();
        let cfg = crate::settings::hardening_global_config();
        let vec_on = cfg.vector_flags().iter().filter(|&&v| v).count();
        let vec_total = crate::harden::VECTOR_KEYS.len();
        // "Reduced" = below the Green floor (CD-30): Green is a first-class safe
        // level, so only Off / a dropped Green-core vector reads as a warning.
        let reduced = crate::harden::is_weakening(&crate::harden::Config::GREEN, &cfg);
        let active_pos = self.active_position() + 1;
        let active_tor = browser::slot_is_tor(self.active_slot);
        // CD-35 Task C: "connected to an onion service" = the active window is a
        // Tor window AND its current page is a `.onion` - derived from the live
        // slot URL, never asserted. (A clearnet slot can never show a `.onion`
        // page, so the conjunction is belt-and-suspenders.) In the sig because
        // it changes on navigation alone.
        let active_onion = active_tor && browser::is_onion_url(&browser::slot_url(self.active_slot));
        let rot_auto = crate::settings::rotate_auto();
        let rot_interval = crate::settings::rotate_interval_min();
        let epoch = browser::rotation_epoch();
        let tz_offset = local_utc_offset_minutes();
        // Vault status for the tile (CD-40 1c). While the HUD exists the gate
        // is open, so the honest states are: no vault / unlocked / dev bypass.
        let vault = if crate::vault::is_unlocked() {
            "unlocked"
        } else if crate::vault::has_vault() {
            "bypassed" // a vault exists but no VMK → only the debug bypass gets here
        } else {
            "none"
        };
        let sig = format!(
            "{}|{vec_on}|{reduced}|{active_pos}|{active_tor}|{active_onion}|{rot_auto}|{rot_interval}|{epoch}|{tz_offset}|{vault}",
            level.as_str()
        );
        if sig == self.hud_sig {
            return;
        }
        self.hud_sig = sig;
        let sent_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let payload = serde_json::json!({
            "sent_ms": sent_ms,
            "tz_offset_min": tz_offset,
            "level": level.as_str(),
            "vectors_on": vec_on,
            "vectors_total": vec_total,
            "reduced": reduced,
            "route": { "window": active_pos, "slot": self.active_slot, "tor": active_tor, "onion": active_onion },
            "rotate": {
                "auto": rot_auto,
                "interval_min": rot_interval,
                "elapsed_ms": if rot_auto { self.rot_anchor.elapsed().as_millis() as u64 } else { 0 },
            },
            "identity_age_ms": browser::identity_age_ms(),
            "vault": vault,
        })
        .to_string();
        browser::set_hud_state(&payload);
    }

    /// Drive automatic identity rotation (CD-29 Task D). When auto-rotation is on and
    /// the interval elapses, re-roll the GLOBAL identity (every window's next page
    /// load / any new window gets the fresh, unlinkable fingerprint) and start the
    /// Pulse Grid re-roll flash. Honest by design: auto rotation re-seeds the basis for
    /// SUBSEQUENT loads and is the visible showpiece - it does NOT reload live pages
    /// (mid-page re-rolling is cosmetic; the manual button and on-restart are the
    /// immediate cross-session-linkage killers). Cheap: a couple of atomic reads/frame.
    fn tick_identity_rotation(&mut self) {
        if !crate::settings::rotate_auto() {
            // Keep the anchor fresh so re-enabling starts a full interval, and no stale
            // countdown is shown.
            self.rot_anchor = Instant::now();
            return;
        }
        let interval = Duration::from_secs(crate::settings::rotate_interval_min() as u64 * 60);
        if self.rot_anchor.elapsed() >= interval {
            browser::rotate_global_identity();
            self.rot_anchor = Instant::now();
            self.rot_flash_until = Some(Instant::now() + Duration::from_millis(900));
        }
    }

    /// The Pulse Grid glow multiplier for the identity-rotation countdown showpiece
    /// (CD-29). 1.0 when auto-rotation is off. Otherwise a gentle build that ramps in
    /// the final stretch of the interval (the grid visibly "charges"), then a bright
    /// decaying burst right after a re-roll (the visible re-roll). Always ≥ 1.0, so the
    /// countdown only ever ADDS energy - never dims the user's chosen glow.
    fn rotation_glow_factor(&self) -> f32 {
        // A post-rotation flash dominates while it lasts.
        if let Some(until) = self.rot_flash_until {
            let now = Instant::now();
            if now < until {
                let remain = (until - now).as_secs_f32();
                let t = (remain / 0.9).clamp(0.0, 1.0); // 1 at the burst, 0 at its end
                return 1.0 + 1.15 * t; // up to ~2.15x, decaying to 1.0
            }
        }
        if !crate::settings::rotate_auto() {
            return 1.0;
        }
        let interval = (crate::settings::rotate_interval_min() as f32 * 60.0).max(1.0);
        let phase = (self.rot_anchor.elapsed().as_secs_f32() / interval).clamp(0.0, 1.0);
        // Flat for most of the cycle; a smooth ramp over the final ~12% ("charging").
        let ramp_start = 0.88;
        if phase <= ramp_start {
            1.0
        } else {
            let t = (phase - ramp_start) / (1.0 - ramp_start); // 0..1 over the tail
            // ease-in (t^2) so the charge visibly accelerates toward the re-roll.
            1.0 + 0.55 * t * t
        }
    }

    // --- The red transition (CD-30 Task E) -----------------------------------

    /// THE red-transition entry point - the single, replaceable hook for the
    /// "bulkhead coming down" choreography. What ships here is the tasteful
    /// BASELINE, deliberately built on the CD-29 charge-and-burst mechanism (one
    /// glow scalar through the existing uniform - no new shader): a bright pulse
    /// sweeps the Pulse Grid as Red engages, while the layout snap+lock (Task D,
    /// `red_locks`) lands in the same pass. Sascha elevates the final
    /// choreography by replacing THIS function's body (and, if the finale needs
    /// more channels, extending `red_glow_factor` alongside it) - nothing else
    /// needs refactoring. Truthful by construction: the only caller is the
    /// `tick_red_state` edge detector, which fires strictly when a window's
    /// EFFECTIVE level becomes Red (level committed + respawn queued + lock
    /// applied in the same pass), never on a mere UI event.
    fn trigger_red_transition(&mut self) {
        self.red_flash_until = Some(Instant::now() + Duration::from_millis(1400));
        tracing::info!("red mode engaged: bunker transition (baseline choreography)");
    }

    /// The red-transition glow multiplier (≥ 1.0, multiplied with the CD-29
    /// rotation factor on top of the user's setting): a strong burst decaying
    /// over ~1.4 s - brighter and longer than the identity re-roll, as befits
    /// the bulkhead. 1.0 whenever no transition is live.
    fn red_glow_factor(&self) -> f32 {
        if let Some(until) = self.red_flash_until {
            let now = Instant::now();
            if now < until {
                let t = ((until - now).as_secs_f32() / 1.4).clamp(0.0, 1.0);
                // Ease-out from ~2.6× so the slam hits hard and settles smoothly.
                return 1.0 + 1.6 * t * t;
            }
        }
        1.0
    }

    /// Truthful red-engagement edge detector (CD-30, rule 0.1): counts the
    /// windows whose EFFECTIVE level is Red this pass and fires the transition
    /// only when that count INCREASES - i.e. a window genuinely entered Red
    /// (global level committed for inheriting windows, or a per-window override).
    /// Every Red-level vector is active by construction (`resolve(Red)` is the
    /// full strict config, carried by the respawn queued in the same drain).
    fn tick_red_state(&mut self) {
        let now = self
            .order
            .iter()
            .filter(|&&id| browser::slot_effective_level(id) == crate::harden::Level::Red)
            .count();
        if now > self.red_slots {
            self.trigger_red_transition();
        }
        self.red_slots = now;
    }

    /// Ctrl+L: reveal + focus the keyboard-active slot's own capsule.
    fn reveal_active_capsule(&mut self) {
        self.engage(self.active_slot, true);
    }

    /// Drive the floating command band once per frame: engage on band hover,
    /// hysteresis disengage (typing exception), and the compositing linger.
    fn update_band(&mut self) {
        // During a favorite drag the host owns the mouse - don't engage/disengage.
        if self.drag.is_some() {
            return;
        }
        match self.overlay {
            // The gate has no band - no slot exists to engage (CD-40).
            Overlay::Lock => {}
            Overlay::Closed => {
                if let Some(s) = self.band_hot_slot() {
                    self.engage(s, false);
                }
            }
            Overlay::Command => {
                let hot = self.band_hot_slot();
                let over_ensemble = self.engaged_band_rect().map(|r| self.point_in(r)).unwrap_or(false);
                let over_launcher = self.point_in(self.launcher_rect());
                if let Some(s) = hot {
                    if Some(s) != self.engaged_slot {
                        self.engage(s, false);
                    }
                    self.bar_hide_at = None;
                    self.band_off_at = None;
                } else if over_ensemble || over_launcher || browser::bar_typing() {
                    self.bar_hide_at = None;
                    self.band_off_at = None;
                } else if self.engaged_slot.is_some() {
                    let now = Instant::now();
                    match self.bar_hide_at {
                        None => self.bar_hide_at = Some(now + BAR_HIDE_HYSTERESIS),
                        Some(deadline) if now >= deadline => self.disengage(),
                        _ => {}
                    }
                }
                // Re-push if the target layout shifted (reflow) while engaged.
                self.push_frame(false);
            }
            Overlay::Settings | Overlay::Info => {}
        }

        // Compositing linger: after disengaging, keep the band composited until
        // the page's fade-out finishes, then finalise to Closed.
        if self.overlay == Overlay::Command
            && self.engaged_slot.is_none()
            && let Some(deadline) = self.band_off_at
            && Instant::now() >= deadline
        {
            self.band_off_at = None;
            self.overlay = Overlay::Closed;
            browser::set_focus(Role::Internal, false);
            browser::set_focus(Role::Slot(self.active_slot), true);
        }
    }

    /// Cursor position translated into a view's coordinates (DIP).
    fn view_coords(&self, origin: (f32, f32)) -> (i32, i32) {
        let vx = ((self.cursor_phys.x - origin.0 as f64) / self.scale as f64) as i32;
        let vy = ((self.cursor_phys.y - origin.1 as f64) / self.scale as f64) as i32;
        (vx, vy)
    }

    /// Is the cursor over the gear button (generous hit radius)?
    fn gear_hit(&self) -> bool {
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (cx, cy, r) = gear_geom(w, self.scale);
        let dx = self.cursor_phys.x as f32 - cx;
        let dy = self.cursor_phys.y as f32 - cy;
        (dx * dx + dy * dy).sqrt() <= r * 1.7
    }

    /// Open the settings card (from any state) or close it back to `Closed`. The
    /// command band (CD-12) is driven by engage/disengage, not this path.
    fn toggle_settings(&mut self) {
        if self.overlay == Overlay::Settings {
            // Closing the card aborts any vault capture begun from it (CD-40)
            // - the host must not keep swallowing the keyboard for an entry
            // field that is no longer visible.
            if crate::vault::capture_active() {
                crate::vault::cancel_capture();
                browser::push_vault_state();
            }
            self.overlay = Overlay::Closed;
            browser::set_focus(Role::Internal, false);
            browser::set_focus(Role::Slot(self.active_slot), true);
            return;
        }
        // From the band or closed → the settings card. Drop any band state.
        self.engaged_slot = None;
        self.bar_hide_at = None;
        self.band_off_at = None;
        self.overlay = Overlay::Settings;
        if let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            let (_, _, iw, ih) = self.internal_rect(w, h);
            browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
            browser::notify_resized(Role::Internal);
            self.applied_internal = (iw as u32, ih as u32);
        }
        browser::show_internal_settings();
        browser::set_focus(Role::Slot(self.active_slot), false);
        browser::set_focus(Role::Internal, true);
    }

    /// Close the settings card back to `Closed` (ESC).
    fn close_settings(&mut self) {
        if self.overlay == Overlay::Settings {
            self.overlay = Overlay::Closed;
            browser::set_focus(Role::Internal, false);
            browser::set_focus(Role::Slot(self.active_slot), true);
        }
    }

    // --- Update-awareness info glyph + panel (CD-13) ------------------------

    /// Info glyph geometry (device px): (center_x, center_y, radius), just left of
    /// the gear on the top-right row.
    fn info_geom(&self) -> (f32, f32, f32) {
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (gcx, gcy, gr) = gear_geom(w, self.scale);
        let ir = self.theme.updates.glyph_radius * self.scale;
        let gap = 18.0 * self.scale;
        (gcx - gr - gap - ir, gcy, ir)
    }

    /// Is the cursor over the info glyph (generous hit radius)?
    fn info_hit(&self) -> bool {
        let (cx, cy, r) = self.info_geom();
        let dx = self.cursor_phys.x as f32 - cx;
        let dy = self.cursor_phys.y as f32 - cy;
        (dx * dx + dy * dy).sqrt() <= r * 1.7
    }

    /// The info panel card rectangle (device px): a floating top-right card just
    /// below the glyph row (the floating law - a discrete panel, not a strip).
    fn info_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        let (wf, hf) = (w as f32, h as f32);
        let m = 24.0 * self.scale;
        let pw = (wf * 0.30).clamp(360.0, 480.0).min(wf);
        let ph = (hf * 0.58).clamp(360.0, 600.0).min(hf);
        let (_, gcy, gr) = self.info_geom();
        let top = gcy + gr + 18.0 * self.scale;
        let x = (wf - pw - m).max(0.0);
        let y = top.min((hf - ph - m).max(0.0));
        (x.round(), y.round(), pw.round(), ph.round())
    }

    /// Toggle the update-awareness info panel (from the info glyph). Mutually
    /// exclusive with settings / the command band.
    fn toggle_info(&mut self) {
        if self.overlay == Overlay::Info {
            self.overlay = Overlay::Closed;
            browser::set_focus(Role::Internal, false);
            browser::set_focus(Role::Slot(self.active_slot), true);
            return;
        }
        // From any state → the info panel. Drop any band state.
        self.engaged_slot = None;
        self.bar_hide_at = None;
        self.band_off_at = None;
        self.overlay = Overlay::Info;
        if let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            let (_, _, iw, ih) = self.internal_rect(w, h);
            browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
            browser::notify_resized(Role::Internal);
            self.applied_internal = (iw as u32, ih as u32);
        }
        browser::show_internal_info();
        browser::set_focus(Role::Slot(self.active_slot), false);
        browser::set_focus(Role::Internal, true);
    }

    /// Close the info panel back to `Closed` (ESC).
    fn close_info(&mut self) {
        if self.overlay == Overlay::Info {
            self.overlay = Overlay::Closed;
            browser::set_focus(Role::Internal, false);
            browser::set_focus(Role::Slot(self.active_slot), true);
        }
    }

    fn mouse_mods(&self) -> u32 {
        self.key_mods | self.button_flags
    }

    fn push_geometry(&mut self) {
        if let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            let rects = self.slot_rects_wh(w, h);
            for (p, &id) in self.order.iter().enumerate() {
                let rc = rects[p];
                browser::set_view_geometry(Role::Slot(id), rc.w as u32, rc.h as u32, self.scale);
            }
            let (_, _, iw, ih) = self.internal_rect(w, h);
            browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
            // MF-zone view (CD-18): its texture is sized to the zone (the CD-31
            // stepped width × slot height - identical for every tab); only its
            // X animates during reflow (carried by the render NDC rect), so
            // geometry is set here on resize / layout changes, not per frame.
            let mf = self.frame_target(w, h).right;
            browser::set_view_geometry(Role::MfZone, mf.w as u32, mf.h as u32, self.scale);
            // HUD strip (CD-30): the transparent top-right info view.
            let (_, _, hw, hh) = self.hud_rect();
            browser::set_view_geometry(Role::Hud, hw as u32, hh as u32, self.scale);
        }
    }

    /// Notify CEF that every live slot view (and the internal + MF-zone views) was
    /// resized.
    fn notify_all_resized(&self) {
        for &id in &self.order {
            browser::notify_resized(Role::Slot(id));
        }
        browser::notify_resized(Role::Internal);
        browser::notify_resized(Role::MfZone);
        browser::notify_resized(Role::Hud);
    }

    /// Total width in units of the live slots (CD-10).
    fn total_units(&self) -> u32 {
        self.order.iter().map(|&id| self.width_units[id]).sum()
    }

    /// Re-clamp the live slots to what the current width allows (called on resize
    /// / DPI change): close excess columns from the right. Websites are not saved
    /// (CD-14, D-0025), so a closed column is simply gone - no overflow. Keeps
    /// `active_slot` valid, promoting a neighbor if the active column was closed.
    fn reflow_slots(&mut self) {
        let cap = self.capacity().max(1) as u32;
        while self.total_units() > cap && self.order.len() > 1 {
            let id = self.order.pop().expect("order is non-empty");
            browser::close_slot(id);
            if let Some(r) = self.renderer.as_mut() {
                r.clear_slot(id);
            }
            self.loading[id] = 0.0;
            self.width_units[id] = 1;
            self.disp_rects[id] = None;
        }
        // A lone double-width slot narrower than the window shrinks to one unit
        // (it cannot be closed, and cannot fit at two).
        if self.total_units() > cap
            && let Some(&id) = self.order.first()
        {
            self.width_units[id] = 1;
        }
        if !self.order.contains(&self.active_slot) {
            self.active_slot = *self.order.last().expect("order is non-empty");
            browser::set_active_slot(self.active_slot);
            if self.overlay == Overlay::Closed {
                browser::set_focus(Role::Slot(self.active_slot), true);
            }
        }
    }

    // --- Session lifecycle (CD-21, D-0035) ----------------------------------

    /// Boot the workspace once the CEF context is ready. A saved "Quit & Save"
    /// session (schema v6) is restored exactly as left (per-slot mode / URL / width /
    /// layout); otherwise - a plain quit, first run, or an old/unknown schema - the
    /// default two-slot layout opens.
    ///
    /// Two phases so the invariants hold: (1) *plan* - decide `order`, per-slot mode
    /// (set BEFORE any browser exists, so a Tor slot is created under its proxied
    /// context and a reused id never inherits a stale `SLOT_TOR`), width and target
    /// URL; (2) *spawn* - push geometry for every view, then create the shared
    /// internal overlay + permanent MF-zone views FIRST and the slots LAST (so a
    /// slot, not the overlay, holds keyboard focus after startup - matching the
    /// pre-CD-21 order). `create_browser` needs geometry set first, hence the split.
    fn restore_session(&mut self, window: &Window) {
        self.restore_session_views(window, true);
    }

    /// [`Shell::restore_session`], parameterized for the vault gate (CD-40):
    /// after an unlock the internal view ALREADY exists (it carried the lock
    /// page and was just navigated back to settings), so only the MF zone, the
    /// HUD and the slots are created then.
    fn restore_session_views(&mut self, window: &Window, create_internal: bool) {
        let hwnd = window_hwnd(window);

        let plan = match crate::store::shared().lock().unwrap().take_saved_session() {
            Some(rows) => self.plan_restore(rows),
            None => self.plan_default(),
        };

        self.push_geometry();
        if create_internal {
            browser::create_browser(Role::Internal, hwnd);
        }
        browser::create_browser(Role::MfZone, hwnd); // → cyberdesk://mfzone/ (permanent)
        browser::create_browser(Role::Hud, hwnd); // → cyberdesk://hud/ (permanent, CD-30)
        for p in &plan {
            match &p.url {
                // A saved clearnet URL reloads it; `None` opens the own start page.
                // Both paths route a Tor slot through its per-slot proxied context
                // via `slot_is_tor` (set in the plan phase).
                Some(u) => browser::create_browser_url(Role::Slot(p.id), hwnd, u),
                None => browser::create_browser(Role::Slot(p.id), hwnd), // → start page
            }
        }
    }

    /// Plan the default layout (fresh start / after a plain quit, D-0035): two slots
    /// side by side - clearnet (left) + Tor (right), both on the own start page
    /// `cyberdesk://start/`. Clamped to what the frame can hold (big-monitor focus):
    /// two where they fit, else a single clearnet slot on a narrow monitor. The right
    /// slot is Tor only when the engine's master switch is enabled; otherwise both
    /// are clearnet (honest - a disabled engine cannot open a Tor window). Sets slot
    /// state; returns the spawn plan (both slots open the start page → `url: None`).
    fn plan_default(&mut self) -> Vec<SlotPlan> {
        let cap = self.capacity().max(1);
        let want = self.slot_max().min(cap).min(2); // two side by side where they fit
        let right_tor = settings::tor_enabled();

        self.order = if want >= 2 { vec![0, 1] } else { vec![0] };
        self.active_slot = 0;
        browser::set_active_slot(0);

        let ids = self.order.clone();
        for &id in &ids {
            self.width_units[id] = 1;
            self.loading[id] = 0.0;
            self.disp_rects[id] = None;
        }
        // Left slot 0: clearnet. Right slot 1 (only if it fits): Tor.
        browser::set_slot_tor(0, false);
        if ids.len() >= 2 {
            if right_tor {
                crate::tor::init();
            }
            browser::set_slot_tor(1, right_tor);
        }

        ids.iter().map(|&id| SlotPlan { id, url: None }).collect()
    }

    /// Plan a restored "Quit & Save" session (D-0035). Slots are re-created with
    /// fresh contiguous ids `0..n` (ids index fixed per-slot arrays, so a persisted
    /// raw id is never reused), clamped to the product cap AND the width the frame
    /// can hold. Each restored slot's mode is set EXPLICITLY here - before any
    /// browser is spawned - so a reused id never inherits a stale `SLOT_TOR` (the
    /// CD-15 leak trap) and a Tor slot is created under its proxied context. A Tor
    /// slot comes back as a REAL Tor slot on the start page (its URL was never
    /// persisted); a clearnet slot reloads its saved URL (empty → start page). If
    /// nothing fits, falls back to the default plan.
    fn plan_restore(&mut self, rows: Vec<crate::store::SessionSlot>) -> Vec<SlotPlan> {
        let cap = self.capacity().max(1) as u32;
        let max_n = self.slot_max().min(cap as usize);

        let mut plan: Vec<SlotPlan> = Vec::new();
        let mut any_tor = false;
        let mut units = 0u32;
        self.active_slot = 0; // fallback until a row marked active is placed
        for row in &rows {
            if plan.len() >= max_n {
                break;
            }
            let mut w = (row.width_units as u32).clamp(1, 2);
            if units + w > cap {
                w = 1; // shrink a double that no longer fits; stop if even one won't
                if units + w > cap {
                    break;
                }
            }
            let id = plan.len(); // fresh contiguous id (never the persisted raw id)
            // Reset per-slot state for the reused id, then set its mode BEFORE spawn.
            self.width_units[id] = w;
            self.loading[id] = 0.0;
            self.disp_rects[id] = None;
            browser::set_slot_tor(id, row.tor);
            any_tor |= row.tor;
            if row.active {
                self.active_slot = id;
            }
            let url = if row.tor || !crate::memory::is_recordable(&row.url) {
                None // Tor slots + internal/blank → the own start page
            } else {
                Some(row.url.clone())
            };
            plan.push(SlotPlan { id, url });
            units += w;
        }

        if plan.is_empty() {
            // Unreachable in practice (a non-empty saved session always fits ≥1
            // slot), but keeps the default as a hard floor rather than an empty frame.
            return self.plan_default();
        }
        if any_tor {
            crate::tor::init();
        }
        self.order = plan.iter().map(|p| p.id).collect();
        browser::set_active_slot(self.active_slot);
        plan
    }

    /// Build + persist the current session (the "Quit & Save" button, D-0035).
    /// Privacy: a Tor slot's URL is NEVER written to disk (it restores on the start
    /// page), and internal/blank slots persist an empty URL - only real clearnet site
    /// URLs are saved, and only on this explicit opt-in.
    fn save_session(&self) {
        let rows: Vec<crate::store::SessionSlot> = self
            .order
            .iter()
            .enumerate()
            .map(|(pos, &id)| {
                let tor = browser::slot_is_tor(id);
                let raw = browser::slot_url(id);
                let url = if tor || !crate::memory::is_recordable(&raw) {
                    String::new()
                } else {
                    raw
                };
                crate::store::SessionSlot {
                    position: pos as i64,
                    url,
                    width_units: self.width_units[id] as i64,
                    active: id == self.active_slot,
                    tor,
                }
            })
            .collect();
        crate::store::shared().lock().unwrap().save_session(&rows);
    }

    /// Foreground guard (tier 1): in fullscreen, keep the shell always-on-top
    /// while the "stay_foreground" setting is on. Dev (`--windowed`) mode is
    /// never topmost. `force` re-asserts the level even if unchanged - used by
    /// the focus-loss watchdog, since a window manager may drop the level when
    /// another window steals focus.
    fn apply_foreground(&mut self, force: bool) {
        if self.windowed {
            return;
        }
        let want = settings::stay_foreground();
        if !force && want == self.applied_topmost {
            return;
        }
        if let Some(window) = self.window.as_ref() {
            let level = if want {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            };
            window.set_window_level(level);
        }
        self.applied_topmost = want;
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attributes = Window::default_attributes().with_title("CARVILON CyberDesk");
        attributes = if self.windowed {
            // `CYBERDESK_WINDOW_SIZE=WxH` overrides the dev window size (default
            // 1600x900) - e.g. to exercise multi-slot layouts on a non-ultrawide.
            let (dw, dh) = std::env::var("CYBERDESK_WINDOW_SIZE")
                .ok()
                .and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?))
                })
                .unwrap_or((1600.0, 900.0));
            attributes.with_inner_size(LogicalSize::new(dw, dh))
        } else {
            attributes
                .with_fullscreen(Some(Fullscreen::Borderless(None)))
                .with_decorations(false)
        };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create window"),
        );
        self.scale = window.scale_factor() as f32;
        let renderer = SurfaceRenderer::new(window.clone(), self.theme.clone());
        // Register the shell HWND with the vault: the Windows Hello modal
        // (passkey enroll/assert, CD-43) parents on it from worker threads.
        crate::vault::set_shell_hwnd(window_hwnd(&window));
        self.window = Some(window);
        self.renderer = Some(renderer);

        if !self.cef_inited {
            browser::init_cef();
            self.cef_inited = true;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                crate::vault::wipe_for_exit();
                event_loop.exit();
            }

            WindowEvent::Focused(focused) => {
                browser::set_focus(self.active_role(), focused);
                // Watchdog: re-assert always-on-top when another window takes focus.
                if !focused {
                    self.apply_foreground(true);
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
                self.reflow_slots();
                self.push_geometry();
                self.notify_all_resized();
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.reflow_slots();
                self.push_geometry();
                self.notify_all_resized();
            }

            WindowEvent::ModifiersChanged(mods) => {
                let s = mods.state();
                self.mods = s;
                self.key_mods = browser::modifier_flags(
                    s.shift_key(),
                    s.control_key(),
                    s.alt_key(),
                );
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_phys = position;
                // During a favorite drag the host captures the mouse: no view gets
                // events; the cursor drives the ghost + drop zones (CD-12).
                if self.drag.is_some() {
                    return;
                }
                let over_gear = self.gear_hit();
                let over_info = self.info_hit();
                self.gear_hover_target = if over_gear { 1.0 } else { 0.0 };
                self.info_hover_target = if over_info { 1.0 } else { 0.0 };
                // Route the move to the view under the cursor (a slot, or the
                // overlay). When the cursor crosses from one view to another, send
                // a mouse-leave to the one it left so its hover states clear. The
                // gear and info glyph are shell chrome - no page gets the move.
                let target = if over_gear || over_info { None } else { self.mouse_target() };
                let next_role = target.map(|(r, _)| r);
                if self.mouse_role != next_role
                    && let Some(prev) = self.mouse_role
                {
                    let origin = self.origin_of_role(prev);
                    let (x, y) = self.view_coords(origin);
                    browser::send_mouse_move(prev, x, y, self.mouse_mods(), true);
                }
                self.mouse_role = next_role;
                if let Some((role, origin)) = target {
                    let (x, y) = self.view_coords(origin);
                    browser::send_mouse_move(role, x, y, self.mouse_mods(), false);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                // While dragging a favorite, the host owns the mouse: releasing the
                // left button drops it (into a gutter, or onto a slot); other
                // buttons are ignored, and no view receives the event (CD-12).
                if self.drag.is_some() {
                    if button == MouseButton::Left && !down {
                        self.drop_favorite();
                    }
                    return;
                }
                // The gear button toggles the settings view; the click is not
                // forwarded to any page. Inert while the vault gate is closed
                // (CD-40): nothing but the lock card is reachable.
                if button == MouseButton::Left && down && self.gear_hit() {
                    if self.overlay != Overlay::Lock {
                        self.toggle_settings();
                    }
                    return;
                }
                // The info glyph toggles the update-awareness panel (CD-13).
                if button == MouseButton::Left && down && self.info_hit() {
                    if self.overlay != Overlay::Lock {
                        self.toggle_info();
                    }
                    return;
                }
                // (CD-18: the shell-drawn corner close orb was retired - a window is
                // now closed by its in-page close icon, `close_slot` IPC.)
                let target = self.mouse_target();
                // Mouse buttons 4/5 are Back/Forward on the slot under the cursor
                // (only when a slot is the actual target - not over an overlay).
                if down
                    && let Some((Role::Slot(id), _)) = target
                {
                    match button {
                        MouseButton::Back => {
                            browser::go_back(Role::Slot(id));
                            return;
                        }
                        MouseButton::Forward => {
                            browser::go_forward(Role::Slot(id));
                            return;
                        }
                        _ => {}
                    }
                }
                let flag = match button {
                    MouseButton::Left => browser::EVENTFLAG_LEFT_MOUSE_BUTTON,
                    MouseButton::Middle => browser::EVENTFLAG_MIDDLE_MOUSE_BUTTON,
                    MouseButton::Right => browser::EVENTFLAG_RIGHT_MOUSE_BUTTON,
                    _ => 0,
                };
                if down {
                    self.button_flags |= flag;
                } else {
                    self.button_flags &= !flag;
                }
                if let Some((role, origin)) = target {
                    // Clicking inside a slot makes it active; if the bar was open,
                    // let it retreat (the click is outside its keep region).
                    if down
                        && let Role::Slot(id) = role
                    {
                        self.set_active(id);
                        if self.overlay == Overlay::Command {
                            self.disengage();
                        }
                    }
                    let (x, y) = self.view_coords(origin);
                    browser::send_mouse_button(role, x, y, self.mouse_mods(), button, down, 1);
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if self.drag.is_some() {
                    return;
                }
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                if let Some((role, origin)) = self.mouse_target() {
                    let (x, y) = self.view_coords(origin);
                    browser::send_mouse_wheel(role, x, y, self.mouse_mods(), dx as i32, dy as i32);
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let vk = match event.physical_key {
                    PhysicalKey::Code(code) => keycode_to_vk(code),
                    _ => 0,
                };
                // Vault secret capture (CD-40, D-0058) - the iron law's teeth:
                // while the lock screen is up, or a setup flow begun from the
                // settings page is capturing, the HOST consumes every key event
                // here and NO page receives a keystroke. The typed secret goes
                // straight into locked memory (vault::SecretInput); the page
                // renders dots from the pushed character count. Ctrl+V pastes
                // via the host clipboard read (never through the renderer).
                if self.overlay == Overlay::Lock || crate::vault::capture_active() {
                    if event.state == ElementState::Pressed && crate::vault::capture_active() {
                        let ctrl = self.mods.control_key();
                        match vk {
                            0x08 => crate::vault::key_backspace(), // Backspace
                            0x0D => crate::vault::key_submit(),    // Enter
                            // Esc: clears the entry, or steps back when empty
                            // (never destructive-by-surprise, CD-44 A1).
                            0x1B => crate::vault::key_escape(),
                            _ if ctrl
                                && event.physical_key == PhysicalKey::Code(KeyCode::KeyV) =>
                            {
                                if let Some(mut text) = clipboard_text() {
                                    crate::vault::key_text(&text);
                                    text.zeroize();
                                }
                            }
                            _ => {
                                if !ctrl && let Some(text) = event.text.as_ref() {
                                    crate::vault::key_text(text.as_str());
                                }
                            }
                        }
                    }
                    browser::push_vault_state();
                    return;
                }
                // ESC cancels an in-progress favorite drag before anything else.
                if self.drag.is_some() {
                    if vk == 0x1B && event.state == ElementState::Pressed {
                        self.cancel_drag();
                    }
                    return;
                }
                // Ctrl+L reveals the active slot's own capsule, focused (CD-12).
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyL)
                {
                    self.reveal_active_capsule();
                    return;
                }
                // Ctrl+D toggles the current surf page's favorite. Handled host-
                // side only while the surf view is active; when the command bar
                // is open the page owns the shortcut and updates its star live.
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && !self.mods.shift_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyD)
                    && self.overlay == Overlay::Closed
                {
                    browser::toggle_current_favorite();
                    return;
                }
                // Slot management, intercepted host-side before the page sees the
                // key: Ctrl+T add, Ctrl+W close, Ctrl+Tab / Ctrl+Shift+Tab cycle,
                // Ctrl+1..4 focus by position (CD-09); Ctrl+Shift+Left/Right swap
                // the active slot with its neighbor, Ctrl+Shift+D toggle its width
                // (CD-10).
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && let PhysicalKey::Code(code) = event.physical_key
                {
                    let shift = self.mods.shift_key();
                    match code {
                        KeyCode::KeyT => {
                            self.add_slot();
                            return;
                        }
                        KeyCode::KeyW => {
                            self.close_active_slot();
                            return;
                        }
                        KeyCode::Tab => {
                            self.cycle_active(!shift);
                            return;
                        }
                        KeyCode::ArrowLeft if shift => {
                            self.swap_active(-1);
                            return;
                        }
                        KeyCode::ArrowRight if shift => {
                            self.swap_active(1);
                            return;
                        }
                        KeyCode::KeyD if shift => {
                            self.toggle_active_width();
                            return;
                        }
                        KeyCode::Digit1 => {
                            self.focus_slot_position(1);
                            return;
                        }
                        KeyCode::Digit2 => {
                            self.focus_slot_position(2);
                            return;
                        }
                        KeyCode::Digit3 => {
                            self.focus_slot_position(3);
                            return;
                        }
                        KeyCode::Digit4 => {
                            self.focus_slot_position(4);
                            return;
                        }
                        _ => {}
                    }
                }
                // ESC chain: a drag is cancelled first (handled above); else the
                // open overlay closes (band / settings / info); else quit the shell.
                if vk == 0x1B && event.state == ElementState::Pressed {
                    match self.overlay {
                        Overlay::Command => self.disengage(),
                        Overlay::Settings => self.close_settings(),
                        Overlay::Info => self.close_info(),
                        Overlay::Closed => {
                            crate::vault::wipe_for_exit();
                            event_loop.exit();
                        }
                        // Unreachable (the capture block above swallows every
                        // key while locked); the gate never ESC-closes.
                        Overlay::Lock => {}
                    }
                    return;
                }
                // Surf navigation shortcuts (only while no overlay is open) act on
                // the active slot.
                if event.state == ElementState::Pressed
                    && self.overlay == Overlay::Closed
                    && let PhysicalKey::Code(code) = event.physical_key
                {
                    let active = Role::Slot(self.active_slot);
                    let (ctrl, alt, shift) =
                        (self.mods.control_key(), self.mods.alt_key(), self.mods.shift_key());
                    match code {
                        KeyCode::F5 => {
                            browser::reload(active);
                            return;
                        }
                        KeyCode::KeyR if ctrl => {
                            if shift {
                                browser::reload_ignore_cache(active);
                            } else {
                                browser::reload(active);
                            }
                            return;
                        }
                        KeyCode::ArrowLeft if alt => {
                            browser::go_back(active);
                            return;
                        }
                        KeyCode::ArrowRight if alt => {
                            browser::go_forward(active);
                            return;
                        }
                        _ => {}
                    }
                }
                let role = self.active_role();
                match event.state {
                    ElementState::Pressed => {
                        browser::send_key_down(role, vk, self.key_mods);
                        if let Some(text) = event.text.as_ref() {
                            for ch in text.encode_utf16() {
                                browser::send_char(role, ch, self.key_mods);
                            }
                        }
                    }
                    ElementState::Released => browser::send_key_up(role, vk, self.key_mods),
                }
            }

            WindowEvent::RedrawRequested => {
                let time = self.start.elapsed().as_secs_f32();
                let (scale, hover) = (self.scale, self.gear_hover);
                // `is_bar` now means the transparent CD-12 command band (vs the
                // opaque settings card); `open` composites the internal view.
                let is_bar = self.overlay == Overlay::Command;
                let open = self.overlay != Overlay::Closed;
                let bar_progress = 1.0;
                let size = self.renderer.as_ref().map(|r| r.size());
                if let Some((w, h)) = size {
                    let internal = self.internal_rect(w, h);
                    // Rects come from the ANIMATED frame (CD-11) - the same
                    // geometry input routing reads, so the reflow can never desync.
                    let disp = self.disp_slots();
                    // While the vault gate is closed (CD-40) no slot and no side
                    // zone exists - the frame is the Pulse Grid and the lock card,
                    // nothing else (an honest empty shell, not placeholders).
                    let slot_views: Vec<SlotView> = if self.overlay == Overlay::Lock {
                        Vec::new()
                    } else {
                        self.order
                            .iter()
                            .enumerate()
                            .map(|(p, &id)| SlotView {
                                rect: (disp[p].x, disp[p].y, disp[p].w, disp[p].h),
                                loading: self.loading[id],
                                active: id == self.active_slot,
                                index: id,
                                pending: None,
                            })
                            .collect()
                    };
                    let sides = if self.overlay == Overlay::Lock {
                        [(0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.0)]
                    } else {
                        [
                            (self.disp_left.x, self.disp_left.y, self.disp_left.w, self.disp_left.h),
                            (self.disp_right.x, self.disp_right.y, self.disp_right.w, self.disp_right.h),
                        ]
                    };
                    // The topmost overlay pass carries the favorite-drag visuals
                    // while a drag is live (CD-12). The per-slot close orbs were
                    // retired in CD-18 (in-page close icon), so the pass is
                    // drag-only now.
                    let overlay_quads = if self.drag.is_some() {
                        self.drag_quads()
                    } else {
                        Vec::new()
                    };
                    // The update-awareness info glyph (CD-13): a status light beside
                    // the gear, filled + pulsing + counted when updates are available.
                    let (icx, icy, ir) = self.info_geom();
                    let pulse =
                        0.5 + 0.5 * (time * std::f32::consts::TAU / self.theme.updates.pulse_period).sin();
                    let info = InfoGlyph {
                        center: (icx, icy),
                        radius: ir,
                        hover: self.info_hover,
                        active: self.info_active,
                        pulse,
                        count: crate::updates::update_count() as f32,
                    };
                    // CD-29: the identity-rotation countdown modulates the glow (charge
                    // → re-roll burst), and CD-30 layers the red-transition burst on
                    // top - both multiply the user's setting and are computed before
                    // the mutable renderer borrow below (as is the HUD rect, CD-30).
                    let glow =
                        settings::glow_intensity() * self.rotation_glow_factor() * self.red_glow_factor();
                    let hud = self.hud_rect();
                    if let Some(r) = self.renderer.as_mut() {
                        r.render(
                            time,
                            &slot_views,
                            &sides,
                            &overlay_quads,
                            internal,
                            hud,
                            gear_geom(w, scale),
                            settings::feather_edges(),
                            settings::animated_background(),
                            glow,
                            scale,
                            open,
                            is_bar,
                            bar_progress,
                            hover,
                            &info,
                        );
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The start-authorization gate (CD-40, D-0058): the shell boots with
        // only the internal view, showing the lock page - unlocking an
        // existing vault, or the MANDATORY first-launch master-password setup
        // when none exists yet (CD-42, D-0062). Either way the host is already
        // capturing; nothing else is created and no sealed state is readable
        // until the gate opens below.
        if self.locked
            && !self.lock_view_started
            && browser::context_ready()
            && let Some(window) = self.window.clone()
        {
            self.overlay = Overlay::Lock;
            let hwnd = window_hwnd(&window);
            if let Some(r) = self.renderer.as_ref() {
                let (w, h) = r.size();
                let (_, _, iw, ih) = self.internal_rect(w, h);
                browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
                self.applied_internal = (iw as u32, ih as u32);
            }
            browser::create_browser_url(Role::Internal, hwnd, browser::LOCK_URL);
            browser::set_focus(Role::Internal, true);
            let purpose = if crate::vault::has_vault() { "unlock_pass" } else { "setup_pass" };
            let _ = crate::vault::begin_capture(purpose);
            browser::push_vault_state();
            self.lock_view_started = true;
        }

        // The gate opens: an unlock or first-launch-setup outcome arrived from
        // a vault worker. The deferred boot runs - sealed identity seed first
        // (it feeds browser creation), then the workspace; the internal view
        // leaves the lock page for its normal settings document.
        if let Some(outcome) = crate::vault::take_outcome() {
            match outcome {
                crate::vault::Outcome::Unlocked | crate::vault::Outcome::SetupDone
                    if self.locked =>
                {
                    self.locked = false;
                    self.overlay = Overlay::Closed;
                    browser::init_identity_seed();
                    browser::show_internal_settings();
                    if let Some(window) = self.window.clone() {
                        self.restore_session_views(&window, false);
                        self.views_started = true;
                    }
                }
                // Failures (and unlocked-session re-wraps) just re-push state.
                _ => {}
            }
            browser::push_vault_state();
        }

        // "Lock now" (CD-40, D-0059): relaunch the shell cold. The exec image
        // restarts with every CEF child gone, and the next boot IS the gate -
        // the same teardown a quit does, plus a fresh start. Secrets are wiped
        // explicitly first (statics never drop on exit).
        if crate::vault::take_relaunch() {
            crate::vault::wipe_for_exit();
            if let Ok(exe) = std::env::current_exe() {
                let mut cmd = std::process::Command::new(exe);
                if self.windowed {
                    cmd.arg("--windowed");
                }
                if let Err(e) = cmd.spawn() {
                    tracing::error!("lock relaunch failed to spawn: {e}");
                }
            }
            event_loop.exit();
            return;
        }

        // Boot the workspace once the CEF context is initialised (CD-21, D-0035): the
        // default two-slot layout (clearnet + Tor at the own start page), or an opt-in
        // "Quit & Save" session restored exactly once. Gated on the vault above.
        if !self.locked
            && !self.views_started
            && browser::context_ready()
            && let Some(window) = self.window.clone()
        {
            self.restore_session(&window);
            self.views_started = true;
        }

        // Drive automatic identity rotation + its Pulse Grid countdown (CD-29).
        if self.views_started {
            self.tick_identity_rotation();
            // Red-engagement edge detector (CD-30 Task E): fires the bunker
            // transition strictly when a window's effective level becomes Red.
            self.tick_red_state();
            // Keep the HUD strip current (CD-30): a cheap signature compare per
            // pass; an IPC push fires only when a displayed value really changed.
            self.push_hud();
        }

        // Spawn a lazy slot's browser on its first navigation (queued by the
        // `navigate` IPC; done here because the main thread owns the HWND).
        if self.views_started
            && let Some((slot, url)) = browser::take_pending_spawn()
            && let Some(window) = self.window.clone()
        {
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser_url(Role::Slot(slot), hwnd, &url);
        }

        // Open queued user-gesture popups / modified-clicks in new slots (D-0018).
        if self.views_started {
            for (source, url) in browser::take_pending_new_slots() {
                self.open_in_new_slot(source, url);
            }
        }

        // Per-window Tor toggles queued by the command glyph (CD-15 Stage B).
        if self.views_started {
            for id in browser::take_pending_tor_toggles() {
                self.toggle_tor(id);
            }
        }

        // Onion handling (CD-35): request-level clearnet refusals land the slot
        // on the honest refusal page; the page's two offers open the `.onion`
        // in a NEW Tor window (beside the refusing one) or switch THAT window
        // to Tor with the URL. All three queued on the CEF UI thread, executed
        // here because the main thread owns the slot lifecycle.
        if self.views_started {
            for (slot, refusal_url) in browser::take_pending_onion_refusals() {
                // The slot may have been closed between the UI-thread cancel and
                // this drain - navigating it then would ghost-spawn a browser
                // for a slot outside the layout.
                if self.order.contains(&slot) {
                    browser::navigate_slot(slot, &refusal_url);
                }
            }
            for (source, url) in browser::take_pending_onion_tor() {
                self.open_in_new_slot_mode(source, url, true);
            }
            for (slot, url) in browser::take_pending_onion_switch() {
                self.switch_slot_to_tor_url(slot, &url);
            }
        }

        // Per-window hardening overrides + a global-preset change (CD-25), and the
        // reported-screen preset (CD-29). Each respawns the affected slot(s) under the
        // new config. A global change respawns only slots that INHERIT it (an
        // overridden slot keeps its own). Screen inheritance is tracked separately
        // from hardening inheritance - a slot may override one and inherit the other -
        // so a slot is respawned at most once even if both changed this pass.
        if self.views_started {
            let mut respawn: Vec<usize> = Vec::new();
            let mark = |id: usize, respawn: &mut Vec<usize>| {
                if !respawn.contains(&id) {
                    respawn.push(id);
                }
            };
            for id in browser::take_pending_slot_hardening() {
                mark(id, &mut respawn);
            }
            for id in browser::take_pending_slot_screen() {
                mark(id, &mut respawn);
            }
            if browser::take_pending_global_hardening() {
                for &id in &self.order {
                    if browser::slot_hardening_override(id).is_none() {
                        mark(id, &mut respawn);
                    }
                }
            }
            if browser::take_pending_global_screen() {
                for &id in &self.order {
                    if browser::slot_screen_override(id).is_none() {
                        mark(id, &mut respawn);
                    }
                }
            }
            for id in respawn {
                self.respawn_slot_preserving_url(id);
            }
        }

        // "Open settings" requested by the HUD Ampel's Custom… (CD-30): the
        // per-vector view lives in the settings card.
        if self.views_started
            && browser::take_pending_open_settings()
            && self.overlay != Overlay::Settings
        {
            self.toggle_settings();
        }

        // Per-window closes queued by the ensemble's close icon (CD-18). The
        // position is resolved fresh (closes shift `order`); last-slot-refuses +
        // neighbor promotion live in close_slot_at.
        if self.views_started {
            for id in browser::take_pending_closes() {
                if let Some(pos) = self.order.iter().position(|&s| s == id) {
                    self.close_slot_at(pos);
                }
            }
        }

        // APPLICATION-level quit queued by the MF-zone quit buttons (CD-21, D-0035).
        // Distinct from the per-slot closes above: this ends the whole shell.
        // "Quit & Save" (save = true) persists the full session first - restored
        // exactly next launch; plain "Quit" writes nothing (default layout next
        // launch, since take_saved_session found no flag). Either way we exit the
        // loop; browser::shutdown_cef() runs after run_app returns (app.rs run()).
        if (self.views_started || self.lock_view_started)
            && let Some(save) = browser::take_pending_quit()
        {
            if save && self.views_started {
                self.save_session();
            }
            crate::vault::wipe_for_exit();
            event_loop.exit();
            return;
        }


        // A favorite-tile drag the page started - the host takes over (CD-12).
        if self.views_started
            && self.drag.is_none()
            && let Some((url, title)) = browser::take_pending_drag()
        {
            self.drag = Some((url, title));
        }

        // Drive the top bar's reveal/hide state machine and slide easing. While the
        // band is engaged this re-pushes the frame every tick (picking up a Tor
        // status change live); the next block covers the band-CLOSED case.
        self.update_band();

        // Refresh the frame when the Tor engine status changes (CD-23, D-0037). The
        // engine reaches READY on a background thread with NO user action, and the
        // frame push (which carries `tor_status` to the per-window anonymity
        // indicator) otherwise only fires on user actions / while the band is engaged
        // - so without this a bootstrapping→ready transition would leave the indicator
        // latched on "Connecting" while Tor is actually usable. `push_frame` dedups on
        // its own signature, so this pushes at most once per real transition and keeps
        // the cached `get_frame` payload current for any (re)created consumer. The MF
        // Tor tab was already correct (it polls `tor_status`); this makes the two agree.
        if self.views_started && crate::tor::status() != self.tor_status_pushed {
            self.push_frame(false);
        }

        // A committed navigation closes whatever internal overlay is open: the
        // command band slides away (CD-12), or the info panel closes as its notes
        // link loads in the active slot (CD-13).
        if browser::take_overlay_close() {
            match self.overlay {
                Overlay::Info => self.close_info(),
                _ => self.disengage(),
            }
        }

        // Keep the command band's internal view sized to the full-width band as
        // the window changes (CD-12: the band spans the top, fixed height).
        if self.overlay == Overlay::Command
            && let Some(r) = self.renderer.as_ref()
        {
            let (w, h) = r.size();
            let (_, _, iw, ih) = self.internal_rect(w, h);
            let want = (iw as u32, ih as u32);
            if want != self.applied_internal {
                self.applied_internal = want;
                browser::set_view_geometry(Role::Internal, want.0, want.1, self.scale);
                browser::notify_resized(Role::Internal);
            }
        }

        // Apply the foreground guard (acts only when the setting changes).
        self.apply_foreground(false);

        // Ease the gear hover glow toward its target.
        self.gear_hover += (self.gear_hover_target - self.gear_hover) * 0.25;
        // Ease the info glyph hover + its "updates available" fill (CD-13).
        self.info_hover += (self.info_hover_target - self.info_hover) * 0.25;
        let info_target = if crate::updates::update_count() > 0 { 1.0 } else { 0.0 };
        self.info_active += (info_target - self.info_active) * 0.15;

        // Advance the CD-11 frame reflow (side zones + slot recenter) one step.
        self.update_frame();

        // Ease each live slot's loading line toward on (loading) / off (done).
        for &id in &self.order {
            let target = if browser::slot_loading(id) { 1.0 } else { 0.0 };
            self.loading[id] += (target - self.loading[id]) * 0.15;
        }

        // In windowed dev mode, reflect the active slot's page title in the OS
        // window title.
        if self.windowed {
            let title = browser::slot_title(self.active_slot);
            if title != self.applied_title
                && let Some(window) = self.window.as_ref()
            {
                let full = if title.is_empty() {
                    "CARVILON CyberDesk".to_string()
                } else {
                    format!("{title} - CARVILON CyberDesk")
                };
                window.set_title(&full);
                self.applied_title = title;
            }
        }

        // Cursor feedback comes from whichever view the cursor is over (CD-09):
        // the hovered slot's / overlay's requested icon, or the default arrow over
        // a gutter / margin / the gear.
        let cursor_icon = match self.mouse_role {
            Some(role) => browser::take_cursor(role),
            None => Some(CursorIcon::Default),
        };
        if let Some(icon) = cursor_icon
            && icon != self.applied_cursor
            && let Some(window) = self.window.as_ref()
        {
            window.set_cursor(icon);
            self.applied_cursor = icon;
        }

        // Upload freshly painted frames into their textures (per slot + overlay).
        if let Some(r) = self.renderer.as_mut() {
            for &id in &self.order {
                browser::with_dirty_frame(Role::Slot(id), |data, w, h| {
                    r.upload_slot(id, data, w, h)
                });
            }
            browser::with_dirty_frame(Role::Internal, |data, w, h| r.upload_panel(data, w, h));
            browser::with_dirty_frame(Role::MfZone, |data, w, h| r.upload_mfzone(data, w, h));
            browser::with_dirty_frame(Role::Hud, |data, w, h| r.upload_hud(data, w, h));
        }

        // Opt-in web-isolation self-test: try to steer the internal view onto the
        // web and confirm the RequestHandler refuses it (logs "[isolation] ...").
        if self.views_started
            && !self.isolation_tested
            && self.start.elapsed().as_secs_f32() > 2.5
            && std::env::var("CYBERDESK_ISOLATION_SELFTEST").is_ok()
        {
            eprintln!("[isolation] self-test: steering the internal view to https://example.com/");
            browser::load_url(Role::Internal, "https://example.com/");
            self.isolation_tested = true;
        }

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

