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
use crate::renderer::{SlotView, SurfaceRenderer};
use crate::settings;
use crate::slots::{self, MAX_SLOTS};
use crate::theme::Theme;

/// Per-frame ease factor for the top bar's slide (CD-08). Exponential approach,
/// matching the gear/loading eases already in this loop; ~180 ms ease-out at the
/// loop's ~60 fps.
const BAR_EASE: f32 = 0.22;

/// Grace period after the cursor leaves the bar's keep region before it hides
/// (hysteresis — no flicker on grazing touches, CD-08).
const BAR_HIDE_HYSTERESIS: Duration = Duration::from_millis(250);

/// Bar content height in logical px, from the shared theme tokens (so the page
/// and the host-side sizing agree): the input row, plus the favorites chip row
/// (an untouched/empty input) or the suggestion list (while typing).
fn bar_body_logical(cmd: &crate::theme::Command, rows: usize, input_empty: bool) -> f32 {
    let body = if input_empty {
        if rows > 0 { cmd.chip_row } else { 0.0 }
    } else if rows > 0 {
        rows as f32 * cmd.row_height + 2.0 * cmd.list_pad
    } else {
        0.0
    };
    cmd.input_height + body
}

/// Cursor hit-test for the bar's reveal hot zone: the free gap band above the
/// surf zone, over the full surf-zone width.
fn hot_zone_contains(cx: f32, cy: f32, zx: f32, zy: f32, zw: f32) -> bool {
    cx >= zx && cx <= zx + zw && cy >= 0.0 && cy < zy
}

/// Cursor hit-test for the bar's keep-open region: the hot zone unioned with the
/// visible bar rect, expanded by `margin`. `bottom` is the visible bar bottom.
fn keep_region_contains(cx: f32, cy: f32, zx: f32, zw: f32, bottom: f32, margin: f32) -> bool {
    cx >= zx - margin && cx <= zx + zw + margin && cy >= 0.0 && cy <= bottom + margin
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
    Bar,
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
        applied_title: String::new(),
        applied_topmost: false,
        isolation_tested: false,
        applied_internal: (0, 0),
        bar_progress: 0.0,
        bar_target: 0.0,
        bar_hide_at: None,
        bar_engaged: false,
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
    applied_title: String,
    applied_topmost: bool,
    isolation_tested: bool,
    /// Internal-view size (device px) currently applied, so the bar's resize in
    /// `about_to_wait` fires only when its body (chips / suggestions) changes.
    applied_internal: (u32, u32),
    /// Top bar slide progress (0 = hidden above the top edge, 1 = fully down) and
    /// its target; the composite clips the bar to `progress * height` (CD-08).
    bar_progress: f32,
    bar_target: f32,
    /// Armed when the cursor leaves the bar's keep region; the bar hides when it
    /// fires (hysteresis). Cleared if the cursor returns first.
    bar_hide_at: Option<Instant>,
    /// Whether the cursor has entered the bar since it was revealed. A keyboard
    /// (Ctrl+L) reveal only becomes subject to the mouse-out hysteresis once the
    /// user has actually moved into the bar — otherwise it hides before they can
    /// type; a hover reveal engages on its first frame.
    bar_engaged: bool,
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

    /// The current slot rectangles (device px, one per live column in display
    /// order) for a given surface size.
    fn slot_rects_wh(&self, w: u32, h: u32) -> Vec<slots::Rect> {
        slots::slot_rects(w, h, self.order.len(), self.scale, &self.theme.slots)
    }

    /// The display position (index into `order`) of the active slot.
    fn active_position(&self) -> usize {
        self.order
            .iter()
            .position(|&id| id == self.active_slot)
            .unwrap_or(0)
    }

    /// The active slot's rectangle for a given surface size.
    fn active_rect_wh(&self, w: u32, h: u32) -> slots::Rect {
        let rects = self.slot_rects_wh(w, h);
        rects[self.active_position().min(rects.len().saturating_sub(1))]
    }

    /// The slot id + rect whose rectangle contains the cursor, if any (Stage C
    /// mouse routing). Slots never overlap, so the first hit is the one.
    fn slot_at_cursor(&self) -> Option<(usize, slots::Rect)> {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let rects = self.slot_rects_wh(w, h);
        let (cx, cy) = (self.cursor_phys.x as f32, self.cursor_phys.y as f32);
        self.order
            .iter()
            .enumerate()
            .find(|&(p, _)| rects[p].contains(cx, cy))
            .map(|(p, &id)| (id, rects[p]))
    }

    /// Top-left origin (device px) of a role's rectangle (a slot's rect, or the
    /// internal overlay's).
    fn origin_of_role(&self, role: Role) -> (f32, f32) {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        match role {
            Role::Internal => {
                let (x, y, _, _) = self.internal_rect(w, h);
                (x, y)
            }
            Role::Slot(id) => self
                .order
                .iter()
                .position(|&i| i == id)
                .map(|p| {
                    let r = self.slot_rects_wh(w, h)[p];
                    (r.x, r.y)
                })
                .unwrap_or((0.0, 0.0)),
        }
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
            Overlay::Bar => {
                // Over the visible (slid-down) bar strip -> the internal view;
                // elsewhere the slot under the cursor.
                let (bx, by, bw, bh) = self.bar_rect(w, h);
                let visible_bottom = by + bh * self.bar_progress;
                if cx >= bx && cx <= bx + bw && cy >= by && cy <= visible_bottom {
                    Some((Role::Internal, (bx, by)))
                } else {
                    self.slot_at_cursor().map(|(id, r)| (Role::Slot(id), (r.x, r.y)))
                }
            }
            Overlay::Closed => self.slot_at_cursor().map(|(id, r)| (Role::Slot(id), (r.x, r.y))),
        }
    }

    /// How many slots fit the current surface width (1..=MAX_SLOTS).
    fn capacity(&self) -> usize {
        let (w, _) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        slots::max_slots(w, self.scale, &self.theme.slots)
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
        if self.order.len() >= self.capacity() {
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
        self.active_slot = free;
        browser::set_active_slot(free);
        // Recentre the group and re-size every view for the new column count.
        self.push_geometry();
        self.notify_all_resized();
        // Reveal the bar focused + empty (the lazy slot's URL is empty), ready to
        // type the first address (which spawns the browser via `navigate`).
        self.reveal_bar(true);
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
        let new_pos = slots::neighbor_position(pos, self.order.len());
        self.active_slot = self.order[new_pos];
        browser::set_active_slot(self.active_slot);
        if self.overlay == Overlay::Closed {
            browser::set_focus(Role::Slot(self.active_slot), true);
        }
        self.push_geometry();
        self.notify_all_resized();
    }

    /// The internal view's rectangle (device px) for the current overlay: the
    /// full top bar for `Bar`, the centered card for `Settings`.
    fn internal_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        match self.overlay {
            Overlay::Bar => self.bar_rect(w, h),
            _ => panel_rect(w, h),
        }
    }

    /// The top bar rectangle (device px): the active slot's width, anchored to
    /// the top edge, its height the input row plus the current body (favorites
    /// chips or the suggestion list). The bar belongs to the active slot and
    /// drives it (CD-09). The page renders at this full size; the composite clips
    /// it to `bar_progress` during the slide.
    fn bar_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        let r = self.active_rect_wh(w, h);
        let bh = (self.bar_content_logical() * self.scale)
            .round()
            .min(h as f32);
        (r.x, 0.0, r.w, bh)
    }

    /// Bar content height in logical px (see [`bar_body_logical`]).
    fn bar_content_logical(&self) -> f32 {
        bar_body_logical(
            &self.theme.command,
            browser::command_rows(),
            browser::bar_input_empty(),
        )
    }

    /// The reveal hot zone: the free gap band above the active slot, over the
    /// active slot's width. Entering it (from `Closed`) slides the bar down.
    fn in_bar_hot_zone(&self) -> bool {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let r = self.active_rect_wh(w, h);
        hot_zone_contains(self.cursor_phys.x as f32, self.cursor_phys.y as f32, r.x, r.y, r.w)
    }

    /// The keep-open region: the hot zone unioned with the visible bar rect, plus
    /// a small margin so a graze along the edge does not flicker. Leaving it (and
    /// not typing) arms the hysteresis hide.
    fn in_bar_keep_region(&self) -> bool {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let r = self.active_rect_wh(w, h);
        let (_, _, _, bh) = self.bar_rect(w, h);
        let bottom = (bh * self.bar_progress).max(r.y);
        keep_region_contains(
            self.cursor_phys.x as f32,
            self.cursor_phys.y as f32,
            r.x,
            r.w,
            bottom,
            6.0 * self.scale,
        )
    }

    /// Reveal the bar (hover-to-top or Ctrl+L). `autofocus` selects the input on
    /// open (Ctrl+L) versus leaving it unfocused (hover). A fresh reveal starts
    /// the slide from fully hidden.
    fn reveal_bar(&mut self, autofocus: bool) {
        browser::set_bar_autofocus(autofocus);
        if self.overlay != Overlay::Bar {
            self.bar_progress = 0.0;
        }
        self.bar_target = 1.0;
        self.bar_hide_at = None;
        // The mouse-out hysteresis waits until the cursor enters the bar; a hover
        // reveal engages on the next frame (cursor is already in the hot zone).
        self.bar_engaged = false;
        self.set_overlay(Overlay::Bar);
    }

    /// Start hiding the bar (slide up); it finalises to `Closed` in `update_bar`.
    fn hide_bar(&mut self) {
        self.bar_target = 0.0;
        self.bar_hide_at = None;
    }

    /// Drive the bar state machine once per frame: hover reveal, hysteresis hide
    /// (with the typing exception), the slide easing, and the hide finalisation.
    fn update_bar(&mut self) {
        match self.overlay {
            Overlay::Closed => {
                if self.in_bar_hot_zone() {
                    self.reveal_bar(false);
                }
            }
            Overlay::Bar => {
                let in_keep = self.in_bar_keep_region();
                if in_keep {
                    self.bar_engaged = true;
                }
                // A cursor over the bar, or a focused/typing input, keeps it open;
                // the hysteresis only applies once the cursor has engaged the bar
                // (so a Ctrl+L reveal is not hidden before the user can type).
                let keep = in_keep || browser::bar_typing();
                if keep {
                    self.bar_hide_at = None;
                } else if self.bar_target > 0.5 && self.bar_engaged {
                    let now = Instant::now();
                    match self.bar_hide_at {
                        None => self.bar_hide_at = Some(now + BAR_HIDE_HYSTERESIS),
                        Some(deadline) if now >= deadline => {
                            self.bar_hide_at = None;
                            self.hide_bar();
                        }
                        _ => {}
                    }
                }
            }
            Overlay::Settings => {}
        }

        // Ease the slide toward its target and finalise a completed hide.
        self.bar_progress += (self.bar_target - self.bar_progress) * BAR_EASE;
        if self.bar_target < 0.5 && self.bar_progress < 0.01 {
            self.bar_progress = 0.0;
            if self.overlay == Overlay::Bar {
                self.overlay = Overlay::Closed;
                browser::set_focus(Role::Internal, false);
                browser::set_focus(Role::Slot(self.active_slot), true);
            }
        } else if self.bar_target > 0.5 && self.bar_progress > 0.99 {
            self.bar_progress = 1.0;
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

    /// Switch the overlay state machine: resize/navigate the internal view and
    /// move keyboard focus accordingly. Closed <-> Settings <-> Command.
    fn set_overlay(&mut self, next: Overlay) {
        // Switching to Settings or Closed drops any bar slide state.
        if next != Overlay::Bar {
            self.bar_target = 0.0;
            self.bar_progress = 0.0;
            self.bar_hide_at = None;
        }
        self.overlay = next;
        // Pre-size the bar to its opening body (favorites chips) so it appears at
        // the right height instead of resizing a frame later.
        if next == Overlay::Bar {
            browser::prime_command_rows();
        }
        // Match the internal OSR view's size to the new overlay before it paints.
        if let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            let (_, _, iw, ih) = self.internal_rect(w, h);
            browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
            browser::notify_resized(Role::Internal);
            self.applied_internal = (iw as u32, ih as u32);
        }
        match next {
            Overlay::Settings => {
                browser::show_internal_settings();
                browser::set_focus(Role::Slot(self.active_slot), false);
                browser::set_focus(Role::Internal, true);
            }
            Overlay::Bar => {
                browser::show_internal_command();
                browser::set_focus(Role::Slot(self.active_slot), false);
                browser::set_focus(Role::Internal, true);
            }
            Overlay::Closed => {
                browser::set_focus(Role::Internal, false);
                browser::set_focus(Role::Slot(self.active_slot), true);
            }
        }
    }

    fn toggle_settings(&mut self) {
        let next = if self.overlay == Overlay::Settings {
            Overlay::Closed
        } else {
            Overlay::Settings
        };
        self.set_overlay(next);
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

    /// Re-clamp the live slot count to what the current width allows (called on
    /// resize / DPI change): close the excess columns from the right (clean
    /// browser shutdown; CD-10 will preserve their URLs). Keeps `active_slot`
    /// valid, promoting a neighbor if the active column was closed.
    fn reflow_slots(&mut self) {
        let cap = self.capacity().max(1);
        while self.order.len() > cap {
            let id = self.order.pop().expect("order is non-empty");
            browser::close_slot(id);
            if let Some(r) = self.renderer.as_mut() {
                r.clear_slot(id);
            }
            self.loading[id] = 0.0;
        }
        if !self.order.contains(&self.active_slot) {
            self.active_slot = *self.order.last().expect("order is non-empty");
            browser::set_active_slot(self.active_slot);
            if self.overlay == Overlay::Closed {
                browser::set_focus(Role::Slot(self.active_slot), true);
            }
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
            attributes.with_inner_size(LogicalSize::new(1600.0, 900.0))
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
                        if self.overlay == Overlay::Bar {
                            self.hide_bar();
                        }
                    }
                    let (x, y) = self.view_coords(origin);
                    browser::send_mouse_button(role, x, y, self.mouse_mods(), button, down, 1);
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
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
                // Ctrl+L reveals the top bar (from any state) with the input
                // focused + selected.
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyL)
                {
                    self.reveal_bar(true);
                    return;
                }
                // Ctrl+D toggles the current surf page's favorite. Handled host-
                // side only while the surf view is active; when the command bar
                // is open the page owns the shortcut and updates its star live.
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyD)
                    && self.overlay == Overlay::Closed
                {
                    browser::toggle_current_favorite();
                    return;
                }
                // Slot management (CD-09), intercepted host-side before the page
                // sees the key: Ctrl+T add, Ctrl+W close, Ctrl+Tab / Ctrl+Shift+Tab
                // cycle, Ctrl+1..4 focus by position.
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && let PhysicalKey::Code(code) = event.physical_key
                {
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
                            self.cycle_active(!self.mods.shift_key());
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
                        Overlay::Bar => self.hide_bar(),
                        Overlay::Settings => self.set_overlay(Overlay::Closed),
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
                let is_bar = self.overlay == Overlay::Bar;
                // The overlay is composited while settings is open, or while the
                // bar has any of itself showing (during the slide up/down).
                let open = match self.overlay {
                    Overlay::Settings => true,
                    Overlay::Bar => self.bar_progress > 0.001,
                    Overlay::Closed => false,
                };
                let bar_progress = self.bar_progress;
                let size = self.renderer.as_ref().map(|r| r.size());
                if let Some((w, h)) = size {
                    let internal = self.internal_rect(w, h);
                    let rects = self.slot_rects_wh(w, h);
                    let slot_views: Vec<SlotView> = self
                        .order
                        .iter()
                        .enumerate()
                        .map(|(p, &id)| SlotView {
                            rect: (rects[p].x, rects[p].y, rects[p].w, rects[p].h),
                            loading: self.loading[id],
                            active: id == self.active_slot,
                            index: id,
                        })
                        .collect();
                    if let Some(r) = self.renderer.as_mut() {
                        r.render(
                            time,
                            &slot_views,
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
        // Create the eager slot 0 + the internal overlay view once the CEF
        // context is initialised. Slots 1..N are lazy (spawned on first navigate).
        if !self.views_started
            && browser::context_ready()
            && let Some(window) = self.window.clone()
        {
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::set_active_slot(0);
            browser::create_browser(Role::Slot(0), hwnd);
            browser::create_browser(Role::Internal, hwnd);
            self.views_started = true;
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

        // Drive the top bar's reveal/hide state machine and slide easing.
        self.update_bar();

        // A committed navigation from the bar slides it away.
        if browser::take_overlay_close() {
            self.hide_bar();
        }

        // Resize the bar's internal view when its body (favorites chips or the
        // suggestion list) changes height, so the composite and the page stay in
        // lockstep as it grows and shrinks.
        if self.overlay == Overlay::Bar
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

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Command;

    fn cmd() -> Command {
        // The token values the "cyber" theme ships (theme.toml [command]).
        Command {
            input_height: 76.0,
            row_height: 46.0,
            list_pad: 8.0,
            max_results: 6,
            chip_row: 54.0,
        }
    }

    #[test]
    fn bar_height_untouched_input_is_input_plus_one_chip_row() {
        let c = cmd();
        // Favorites present -> a single chip row regardless of how many chips.
        assert_eq!(bar_body_logical(&c, 3, true), 76.0 + 54.0);
        assert_eq!(bar_body_logical(&c, 1, true), 76.0 + 54.0);
        // No favorites -> input row only, no chip band.
        assert_eq!(bar_body_logical(&c, 0, true), 76.0);
    }

    #[test]
    fn bar_height_typing_is_input_plus_suggestion_rows() {
        let c = cmd();
        // N suggestion rows plus the list's top/bottom padding.
        assert_eq!(bar_body_logical(&c, 3, false), 76.0 + 3.0 * 46.0 + 2.0 * 8.0);
        // Empty result set while typing -> input row only.
        assert_eq!(bar_body_logical(&c, 0, false), 76.0);
    }

    #[test]
    fn hot_zone_is_the_gap_over_the_surf_width() {
        // Surf zone at a 1600x900 window: zx=320, zy=135, zw=960.
        let (zx, zy, zw) = (320.0, 135.0, 960.0);
        assert!(hot_zone_contains(800.0, 10.0, zx, zy, zw)); // top-centre gap
        assert!(hot_zone_contains(zx, 0.0, zx, zy, zw)); // left edge, very top
        assert!(!hot_zone_contains(800.0, 200.0, zx, zy, zw)); // below the gap
        assert!(!hot_zone_contains(100.0, 10.0, zx, zy, zw)); // left of the surf width
        assert!(!hot_zone_contains(1550.0, 10.0, zx, zy, zw)); // right of it (gear side)
    }

    #[test]
    fn keep_region_covers_bar_plus_margin_not_below() {
        let (zx, zw, bottom, m) = (320.0, 960.0, 260.0, 6.0);
        assert!(keep_region_contains(800.0, 100.0, zx, zw, bottom, m)); // inside the bar
        assert!(keep_region_contains(800.0, bottom + 5.0, zx, zw, bottom, m)); // within margin
        assert!(!keep_region_contains(800.0, bottom + 20.0, zx, zw, bottom, m)); // well below
        assert!(!keep_region_contains(zx - 20.0, 100.0, zx, zw, bottom, m)); // left of the bar
    }
}
