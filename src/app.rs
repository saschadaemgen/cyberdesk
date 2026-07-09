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

use crate::browser::{self, Role};
use crate::renderer::{self, SlotView, SurfaceRenderer};
use crate::session;
use crate::settings;
use crate::slots::{self, MAX_SLOTS};
use crate::theme::Theme;

/// Grace period after the cursor leaves the engaged band region before it
/// disengages (hysteresis — no flicker on grazing touches, CD-08 → CD-12).
const BAR_HIDE_HYSTERESIS: Duration = Duration::from_millis(250);

/// After the band disengages, keep it composited this long so the page's
/// per-ensemble fade-out (CSS ~220 ms) completes before compositing stops (CD-12).
const BAND_FADE_LINGER: Duration = Duration::from_millis(300);

/// Debounce for session-workspace saves (CD-10): a meaningful change arms a save
/// this far in the future, coalescing bursts off the render hot path.
const SESSION_DEBOUNCE: Duration = Duration::from_millis(500);

/// Per-frame ease factor for the CD-11 frame reflow (side zones retreating to
/// rails + the slot recenter). Exponential approach, ~220 ms ease-out at ~60 fps
/// — the same host-side interpolation pattern as the top-bar slide.
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

/// Which internal overlay (if any) is currently shown. `Bar` is the CD-08
/// hover-reveal top bar (the former centered command palette); `Settings` is the
/// gear card. The two are mutually exclusive — one shared internal OSR view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Closed,
    Settings,
    Command,
}

pub fn run(windowed: bool) {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    // Opens and takes ownership of the app-state store (state.db) and loads the
    // persisted toggles; the settings IPC writes through it live.
    settings::init();

    let mut app = Shell {
        windowed,
        window: None,
        renderer: None,
        theme: Theme::load(),
        start: Instant::now(),
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
        order: vec![0],
        active_slot: 0,
        mouse_role: None,
        loading: [0.0; MAX_SLOTS],
        width_units: [1; MAX_SLOTS],
        armed: std::array::from_fn(|_| None),
        overflow: Vec::new(),
        session_dirty: None,
        session_saved_sig: String::new(),
        disp_rects: [None; MAX_SLOTS],
        disp_left: slots::Rect::default(),
        disp_right: slots::Rect::default(),
        disp_side_width: 0.0,
        frame_inited: false,
        applied_title: String::new(),
        applied_topmost: false,
        isolation_tested: false,
        applied_internal: (0, 0),
        engaged_slot: None,
        bar_hide_at: None,
        band_off_at: None,
        frame_sig: String::new(),
        drag: None,
    };
    event_loop.run_app(&mut app).expect("event loop error");

    browser::shutdown_cef();
}

struct Shell {
    windowed: bool,
    window: Option<Arc<Window>>,
    renderer: Option<SurfaceRenderer>,
    theme: Theme,
    start: Instant,
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
    /// Per-slot pre-armed URL (CD-10): a restored lazy slot's page, loaded on the
    /// slot's first interaction (activation / click). `None` once spawned or for
    /// an empty slot. Indexed by slot id.
    armed: [Option<String>; MAX_SLOTS],
    /// Session slots that did not fit the current width at restore (windowed
    /// shrink). Kept out of the display but re-saved, so a wider restart brings
    /// them back (CD-10).
    overflow: Vec<crate::store::SessionSlot>,
    /// When set, a debounced session save is due at this instant (CD-10).
    session_dirty: Option<Instant>,
    /// Signature of the last-saved session, so a save fires only on real change.
    session_saved_sig: String,
    /// Animated frame (CD-11): the on-screen (interpolated) rect per slot id, and
    /// the eased side zones. Rendering AND input read these — one per-frame
    /// geometry, so the reflow animation can never desync. `disp_rect[id]` is
    /// `None` until a slot's first frame (it then grows from a collapsed sliver).
    disp_rects: [Option<slots::Rect>; MAX_SLOTS],
    disp_left: slots::Rect,
    disp_right: slots::Rect,
    disp_side_width: f32,
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
    /// (target rects + engaged slot) — not per frame (the CD-11 IPC cadence).
    frame_sig: String,
    /// An in-progress favorite-tile drag `(url, title)` (CD-12): the host owns it
    /// (ghost + drop zones) and slot views receive no mouse until it ends.
    drag: Option<(String, String)>,
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

    /// The current slot rectangles (device px, one per live column in display
    /// order) for a given surface size, honoring each slot's width units.
    fn slot_rects_wh(&self, w: u32, h: u32) -> Vec<slots::Rect> {
        slots::slot_rects_units(w, h, &self.units_in_order(), self.scale, &self.theme.slots)
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
        }
    }

    /// Advance the animated frame one step toward the target [`slots::frame_layout`]
    /// for the current slots (CD-11). Called once per frame; the eased result is
    /// what both rendering and input read.
    fn update_frame(&mut self) {
        let Some((w, h)) = self.renderer.as_ref().map(|r| r.size()) else {
            return;
        };
        let units = self.units_in_order();
        let target = slots::frame_layout(w, h, &units, self.scale, &self.theme.slots);
        let g = self.theme.slots.gutter * self.scale;

        if !self.frame_inited {
            for (p, &id) in self.order.iter().enumerate() {
                self.disp_rects[id] = Some(target.slots[p]);
            }
            self.disp_left = target.left;
            self.disp_right = target.right;
            self.disp_side_width = target.side_width;
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

        // Ease the side width; derive the side rects from it and the animated
        // group bounds so the zones glide with the columns.
        self.disp_side_width += (target.side_width - self.disp_side_width) * FRAME_EASE;
        let first = self.order[0];
        let last = *self.order.last().expect("order is non-empty");
        let gl = self.disp_rects[first].map(|r| r.x).unwrap_or(target.left.x);
        let gr = self.disp_rects[last]
            .map(|r| r.x + r.w)
            .unwrap_or(target.right.x);
        self.disp_left = slots::Rect {
            x: gl - g - self.disp_side_width,
            y: target.left.y,
            w: self.disp_side_width,
            h: target.left.h,
        };
        self.disp_right = slots::Rect {
            x: gr + g,
            y: target.right.y,
            w: self.disp_side_width,
            h: target.right.h,
        };
    }

    /// The view the mouse currently routes to and its origin: the internal
    /// overlay when the cursor is over its visible rect, otherwise the slot under
    /// the cursor. `None` over a gutter / margin (no page there).
    fn mouse_target(&self) -> Option<(Role, (f32, f32))> {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        match self.overlay {
            Overlay::Settings => {
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
                // elsewhere the slot under the cursor (so another column's gap can
                // engage it, and slots stay usable). CD-12.
                let over_ensemble = self.engaged_band_rect().map(|r| self.point_in(r)).unwrap_or(false);
                if over_ensemble || self.point_in(self.launcher_rect()) {
                    Some((Role::Internal, (0.0, 0.0)))
                } else {
                    self.slot_at_cursor().map(|(id, r)| (Role::Slot(id), (r.x, r.y)))
                }
            }
            Overlay::Closed => self.slot_at_cursor().map(|(id, r)| (Role::Slot(id), (r.x, r.y))),
        }
    }

    /// The total slot-unit budget the frame can hold at the current width — the
    /// rail-state center budget (CD-11), so slots are capped against the maximum
    /// the frame will ever fit.
    fn capacity(&self) -> usize {
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        slots::frame_capacity(w, self.scale, &self.theme.slots)
    }

    /// Make slot id `id` the active slot: move CEF keyboard focus (only while no
    /// overlay is open — otherwise the internal view holds focus) and update the
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
        // First interaction with a restored lazy slot spawns its browser (CD-10).
        self.spawn_if_armed(id);
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

    /// Add a slot right of the active one (Ctrl+T). No-op at capacity / MAX_SLOTS.
    /// The new slot is lazy (placeholder, no browser); it becomes active and the
    /// top bar reveals focused + empty so the user can type its first address.
    fn add_slot(&mut self) {
        // A new slot is one unit; it must fit both the count and the unit budget.
        if self.order.len() >= MAX_SLOTS || self.total_units() + 1 > self.capacity() as u32 {
            return;
        }
        let Some(free) = slots::free_id(&self.order) else {
            return;
        };
        // Drop keyboard focus from the outgoing active slot's browser before the
        // new (lazy, browser-less) slot takes over; the bar then holds focus.
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), false);
        }
        let pos = slots::insert_position(&self.order, self.active_slot);
        self.order.insert(pos, free);
        self.loading[free] = 0.0;
        self.width_units[free] = 1;
        self.armed[free] = None;
        self.active_slot = free;
        browser::set_active_slot(free);
        // Recentre the group and re-size every view for the new column count.
        self.push_geometry();
        self.notify_all_resized();
        // Reveal the bar focused + empty (the lazy slot's URL is empty), ready to
        // type the first address (which spawns the browser via `navigate`).
        self.reveal_active_capsule();
    }

    /// Open `url` in a new slot beside the source slot — a user-gesture popup or
    /// a Ctrl-/middle-click on a link (D-0018). The new slot is one unit, spawns
    /// immediately with the URL, and becomes active. If the grid has no room, fall
    /// back to the CD-04 behavior: navigate the source slot in place.
    fn open_in_new_slot(&mut self, source_id: usize, url: String) {
        let has_room = self.order.len() < MAX_SLOTS && self.total_units() < self.capacity() as u32;
        let Some(free) = slots::free_id(&self.order).filter(|_| has_room) else {
            browser::load_url(Role::Slot(source_id), &url);
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
        self.armed[free] = None;
        self.loading[free] = 0.0;
        self.active_slot = free;
        browser::set_active_slot(free);
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
        self.armed[free] = None;
        self.loading[free] = 0.0;
        self.disp_rects[free] = None;
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
    /// between each pair, and after the last slot — paired with the display
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
        self.order.len() < MAX_SLOTS && self.total_units() < self.capacity() as u32
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
                });
            }
        } else if let Some((_, r)) = self.slot_at_cursor() {
            // Full grid: dropping over a slot navigates it — hint by glowing it.
            out.push(renderer::DragQuad {
                rect: (r.x, r.y, r.w, r.h),
                color: [b[0], b[1], b[2], 0.16],
                radius: self.theme.page.corner_radius,
                glow: 10.0 * self.scale,
            });
        }
        // The ghost: a glowing brand circle at the cursor.
        let gs = 40.0 * self.scale;
        out.push(renderer::DragQuad {
            rect: (cx - gs * 0.5, cy - gs * 0.5, gs, gs),
            color: [b[0], b[1], b[2], 0.85],
            radius: gs * 0.5,
            glow: 13.0 * self.scale,
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

    /// Cancel an in-progress drag (ESC) — no drop.
    fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// Close the active slot (Ctrl+W). The last slot cannot be closed. The
    /// browser shuts down cleanly, the group recenters, and the nearest neighbor
    /// becomes active.
    fn close_active_slot(&mut self) {
        if self.order.len() <= 1 {
            return;
        }
        let pos = self.active_position();
        let id = self.order.remove(pos);
        browser::close_slot(id);
        if let Some(r) = self.renderer.as_mut() {
            r.clear_slot(id);
        }
        self.loading[id] = 0.0;
        self.armed[id] = None;
        self.width_units[id] = 1;
        self.disp_rects[id] = None;
        let new_pos = slots::neighbor_position(pos, self.order.len());
        self.active_slot = self.order[new_pos];
        browser::set_active_slot(self.active_slot);
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), true);
        }
        self.push_geometry();
        self.notify_all_resized();
    }

    /// Swap the active slot with its neighbor (Ctrl+Shift+Left/Right). A pure
    /// order operation — the active slot keeps its id (and its browser/texture),
    /// only its display position changes; no browser moves and no view resizes
    /// (widths are unchanged), so the compositor picks up the new positions next
    /// frame. A hard swap (no slide animation) — see D-0019. No-op at the edge.
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
    /// halving always works. Only the toggled slot's view resizes (its page
    /// reflows to the new width); the others merely recenter (CD-10).
    fn toggle_active_width(&mut self) {
        let id = self.active_slot;
        if self.width_units[id] == 2 {
            self.width_units[id] = 1;
        } else if self.total_units() < self.capacity() as u32 {
            self.width_units[id] = 2;
        } else {
            return; // doubling would overflow — no-op
        }
        self.push_geometry();
        browser::notify_resized(Role::Slot(id));
    }

    /// The internal view's rectangle (device px) for the current overlay: the
    /// full-width transparent command band for `Command` (CD-12), the centered
    /// card for `Settings`.
    fn internal_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        match self.overlay {
            Overlay::Command => (0.0, 0.0, w as f32, self.band_height()),
            _ => panel_rect(w, h),
        }
    }

    /// The command band height in device px (a fixed token band; the ensembles
    /// float within it).
    fn band_height(&self) -> f32 {
        (self.theme.command.band_height * self.scale).round()
    }

    /// The slot whose floating-ensemble band segment the cursor is over — the top
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
    /// origin). Pushed on change only — the page glides via CSS (CD-11 cadence).
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
        let mut sig = format!("{:?}", self.engaged_slot);
        for (p, &id) in self.order.iter().enumerate() {
            let _ = write!(
                sig,
                ";{}:{},{}",
                id,
                (rects[p].x as f64 / scale).round(),
                (rects[p].w as f64 / scale).round()
            );
        }
        if !autofocus && sig == self.frame_sig {
            return; // nothing changed — no IPC (the CD-11 on-change cadence)
        }
        self.frame_sig = sig;
        // Build + push only on a real change.
        let slots: Vec<serde_json::Value> = self
            .order
            .iter()
            .enumerate()
            .map(|(p, &id)| {
                serde_json::json!({
                    "id": id,
                    "x": (rects[p].x as f64 / scale).round(),
                    "w": (rects[p].w as f64 / scale).round(),
                })
            })
            .collect();
        let payload = serde_json::json!({
            "slots": slots,
            "engaged": self.engaged_slot,
            "autofocus": autofocus,
        })
        .to_string();
        browser::set_frame_state(&payload);
    }

    /// Ctrl+L: reveal + focus the keyboard-active slot's own capsule.
    fn reveal_active_capsule(&mut self) {
        self.engage(self.active_slot, true);
    }

    /// Drive the floating command band once per frame: engage on band hover,
    /// hysteresis disengage (typing exception), and the compositing linger.
    fn update_band(&mut self) {
        // During a favorite drag the host owns the mouse — don't engage/disengage.
        if self.drag.is_some() {
            return;
        }
        match self.overlay {
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
            Overlay::Settings => {}
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
        }
    }

    /// Notify CEF that every live slot view (and the internal view) was resized.
    fn notify_all_resized(&self) {
        for &id in &self.order {
            browser::notify_resized(Role::Slot(id));
        }
        browser::notify_resized(Role::Internal);
    }

    /// Total width in units of the live slots (CD-10).
    fn total_units(&self) -> u32 {
        self.order.iter().map(|&id| self.width_units[id]).sum()
    }

    /// Re-clamp the live slots to what the current width allows (called on resize
    /// / DPI change): close excess columns from the right and preserve each in the
    /// session overflow, so a wider restart brings them back (CD-10). Keeps
    /// `active_slot` valid, promoting a neighbor if the active column was closed.
    fn reflow_slots(&mut self) {
        let cap = self.capacity().max(1) as u32;
        while self.total_units() > cap && self.order.len() > 1 {
            let id = self.order.pop().expect("order is non-empty");
            self.overflow.insert(
                0,
                crate::store::SessionSlot {
                    url: self.slot_persist_url(id),
                    width_units: self.width_units[id],
                    active: false,
                },
            );
            browser::close_slot(id);
            if let Some(r) = self.renderer.as_mut() {
                r.clear_slot(id);
            }
            self.loading[id] = 0.0;
            self.armed[id] = None;
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

    // --- Session workspace (CD-10) ------------------------------------------

    /// The scheme color of a restored-pending slot (no browser yet, armed URL) —
    /// its placeholder shows a small dot in this color (CD-10). `None` if the slot
    /// is live or genuinely empty.
    fn slot_pending_color(&self, id: usize) -> Option<[f32; 3]> {
        if browser::slot_has_browser(id) {
            return None;
        }
        let url = self.armed[id].as_deref()?;
        let c = &self.theme.colors;
        let hex = if url.starts_with("https://") {
            &c.accent
        } else if url.starts_with("http://") {
            &c.warn
        } else {
            &c.text_dim
        };
        Some(crate::theme::hex3(hex))
    }

    /// The URL slot `id` persists into the session: its live page (once loaded),
    /// else its pre-armed restored URL, filtered to real web pages ("" otherwise).
    fn slot_persist_url(&self, id: usize) -> String {
        let live = if browser::slot_has_browser(id) {
            browser::slot_url(id)
        } else {
            String::new()
        };
        let url = if live.is_empty() {
            self.armed[id].clone().unwrap_or_default()
        } else {
            live
        };
        session::persist_url(&url)
    }

    /// A compact signature of the current session state, so a save fires only on a
    /// real change (order, per-slot url/width, the active slot, and overflow).
    fn session_signature(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        for &id in &self.order {
            let _ = write!(
                s,
                "{}|{}|{};",
                self.slot_persist_url(id),
                self.width_units[id],
                (id == self.active_slot) as u32
            );
        }
        s.push('#');
        for o in &self.overflow {
            let _ = write!(s, "{}|{}|{};", o.url, o.width_units, o.active as u32);
        }
        s
    }

    /// Persist the current session now (displayed slots in order, then overflow).
    fn save_session_now(&mut self) {
        let mut slots: Vec<crate::store::SessionSlot> = self
            .order
            .iter()
            .map(|&id| crate::store::SessionSlot {
                url: self.slot_persist_url(id),
                width_units: self.width_units[id],
                active: id == self.active_slot,
            })
            .collect();
        slots.extend(self.overflow.iter().cloned());
        session::save(&slots);
        self.session_saved_sig = self.session_signature();
        self.session_dirty = None;
    }

    /// Drive the debounced session save: arm on a detected change, fire when due.
    fn maybe_save_session(&mut self) {
        if !self.views_started {
            return;
        }
        if self.session_dirty.is_none() && self.session_signature() != self.session_saved_sig {
            self.session_dirty = Some(Instant::now() + SESSION_DEBOUNCE);
        }
        if let Some(deadline) = self.session_dirty
            && Instant::now() >= deadline
        {
            self.save_session_now();
        }
    }

    /// If slot `id` is a restored lazy slot (no browser but a pre-armed URL),
    /// spawn its browser now with that URL — its "first interaction" (CD-10).
    fn spawn_if_armed(&mut self, id: usize) {
        if browser::slot_has_browser(id) {
            return;
        }
        let Some(url) = self.armed[id].clone() else {
            return;
        };
        if let Some(window) = self.window.clone() {
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser_url(Role::Slot(id), hwnd, &url);
        }
    }

    /// Restore the saved slot workspace (called once at startup). Builds the slot
    /// order / widths / armed URLs / active from the session, fitting from the
    /// left by width units (the rest kept in overflow), then spawns the active
    /// slot immediately and leaves the rest lazy. A fresh / no session falls back
    /// to the CD-09 default: one slot on the home page.
    fn restore_session(&mut self, window: &Window) {
        let hwnd = window_hwnd(window);
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let cap = slots::max_slots(w, self.scale, &self.theme.slots) as u32;
        let saved = session::load();

        if saved.is_empty() {
            // Fresh install / no session: the CD-09 default — one home-page slot.
            self.order = vec![0];
            self.active_slot = 0;
            browser::set_active_slot(0);
            self.push_geometry();
            browser::create_browser(Role::Internal, hwnd);
            browser::create_browser(Role::Slot(0), hwnd);
            return;
        }

        // Fit saved slots from the left by width units (pure, unit-tested); the
        // shell assigns fresh contiguous ids (id = display index at startup).
        let plan = session::plan_restore(&saved, cap, MAX_SLOTS);
        self.order = (0..plan.slots.len()).collect();
        for (id, ps) in plan.slots.iter().enumerate() {
            self.width_units[id] = ps.width_units;
            self.armed[id] = ps.url.clone();
        }
        self.overflow = plan.overflow;
        let active_id = plan.active;
        self.active_slot = active_id;
        browser::set_active_slot(active_id);
        self.push_geometry();
        browser::create_browser(Role::Internal, hwnd);
        // The active slot spawns immediately if it carries a page; otherwise it is
        // an empty active placeholder.
        if let Some(url) = self.armed[active_id].clone() {
            browser::create_browser_url(Role::Slot(active_id), hwnd, &url);
        }
    }

    /// Foreground guard (tier 1): in fullscreen, keep the shell always-on-top
    /// while the "stay_foreground" setting is on. Dev (`--windowed`) mode is
    /// never topmost. `force` re-asserts the level even if unchanged — used by
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
            // 1600x900) — e.g. to exercise multi-slot layouts on a non-ultrawide.
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
        self.window = Some(window);
        self.renderer = Some(renderer);

        if !self.cef_inited {
            browser::init_cef();
            self.cef_inited = true;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

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
                self.gear_hover_target = if over_gear { 1.0 } else { 0.0 };
                // Route the move to the view under the cursor (a slot, or the
                // overlay). When the cursor crosses from one view to another, send
                // a mouse-leave to the one it left so its hover states clear.
                let target = if over_gear { None } else { self.mouse_target() };
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
                // forwarded to any page.
                if button == MouseButton::Left && down && self.gear_hit() {
                    self.toggle_settings();
                    return;
                }
                let target = self.mouse_target();
                // Mouse buttons 4/5 are Back/Forward on the slot under the cursor
                // (only when a slot is the actual target — not over an overlay).
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
                // ESC chain (CD-08): bar visible -> hide the bar; else settings
                // open -> close settings; else quit the shell.
                if vk == 0x1B && event.state == ElementState::Pressed {
                    match self.overlay {
                        Overlay::Command => self.disengage(),
                        Overlay::Settings => self.close_settings(),
                        Overlay::Closed => event_loop.exit(),
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
                    // Rects come from the ANIMATED frame (CD-11) — the same
                    // geometry input routing reads, so the reflow can never desync.
                    let disp = self.disp_slots();
                    let slot_views: Vec<SlotView> = self
                        .order
                        .iter()
                        .enumerate()
                        .map(|(p, &id)| SlotView {
                            rect: (disp[p].x, disp[p].y, disp[p].w, disp[p].h),
                            loading: self.loading[id],
                            active: id == self.active_slot,
                            index: id,
                            pending: self.slot_pending_color(id),
                        })
                        .collect();
                    let sides = [
                        (self.disp_left.x, self.disp_left.y, self.disp_left.w, self.disp_left.h),
                        (self.disp_right.x, self.disp_right.y, self.disp_right.w, self.disp_right.h),
                    ];
                    let drag = self.drag_quads();
                    if let Some(r) = self.renderer.as_mut() {
                        r.render(
                            time,
                            &slot_views,
                            &sides,
                            &drag,
                            internal,
                            gear_geom(w, scale),
                            settings::feather_edges(),
                            settings::animated_background(),
                            settings::glow_intensity(),
                            scale,
                            open,
                            is_bar,
                            bar_progress,
                            hover,
                        );
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Restore the saved slot workspace once the CEF context is initialised
        // (CD-10): the active slot spawns immediately, the rest stay lazy with
        // their URL pre-armed. A fresh install falls back to the CD-09 default
        // (one slot, home page).
        if !self.views_started
            && browser::context_ready()
            && let Some(window) = self.window.clone()
        {
            self.restore_session(&window);
            self.views_started = true;
            self.session_saved_sig = self.session_signature();
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

        // A favorite-tile drag the page started — the host takes over (CD-12).
        if self.views_started
            && self.drag.is_none()
            && let Some((url, title)) = browser::take_pending_drag()
        {
            self.drag = Some((url, title));
        }

        // Drive the top bar's reveal/hide state machine and slide easing.
        self.update_band();

        // A committed navigation from the bar slides it away.
        if browser::take_overlay_close() {
            self.disengage();
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
                    format!("{title} — CARVILON CyberDesk")
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

        // Persist the slot workspace when it changes (debounced, off the hot path).
        self.maybe_save_session();

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

