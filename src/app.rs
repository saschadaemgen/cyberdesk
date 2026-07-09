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
use crate::renderer::SurfaceRenderer;
use crate::settings;
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

/// Surf-zone rectangle in device pixels: 60% width, 70% height, centered.
fn zone_rect(width: u32, height: u32) -> (f32, f32, f32, f32) {
    let (w, h) = (width as f32, height as f32);
    let zw = (w * 0.60).round();
    let zh = (h * 0.70).round();
    let zx = ((w - zw) * 0.5).round();
    let zy = ((h - zh) * 0.5).round();
    (zx, zy, zw, zh)
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
        loading_intensity: 0.0,
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
    loading_intensity: f32,
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
    /// The view input is currently routed to (internal when an overlay is open,
    /// else the surf view).
    fn active_role(&self) -> Role {
        if self.overlay == Overlay::Closed {
            Role::Surf
        } else {
            Role::Internal
        }
    }

    /// The internal view's rectangle (device px) for the current overlay: the
    /// full top bar for `Bar`, the centered card for `Settings`.
    fn internal_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        match self.overlay {
            Overlay::Bar => self.bar_rect(w, h),
            _ => panel_rect(w, h),
        }
    }

    /// The top bar rectangle (device px): full surf-zone width, anchored to the
    /// top edge, its height the input row plus the current body (favorites chips
    /// or the suggestion list). The page renders at this full size; the composite
    /// clips it to `bar_progress` during the slide.
    fn bar_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        let (zx, _, zw, _) = zone_rect(w, h);
        let bh = (self.bar_content_logical() * self.scale)
            .round()
            .min(h as f32);
        (zx, 0.0, zw, bh)
    }

    /// Bar content height in logical px (see [`bar_body_logical`]).
    fn bar_content_logical(&self) -> f32 {
        bar_body_logical(
            &self.theme.command,
            browser::command_rows(),
            browser::bar_input_empty(),
        )
    }

    /// The reveal hot zone: the free gap band above the surf zone, over the full
    /// surf-zone width. Entering it (from `Closed`) slides the bar down.
    fn in_bar_hot_zone(&self) -> bool {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (zx, zy, zw, _) = zone_rect(w, h);
        hot_zone_contains(self.cursor_phys.x as f32, self.cursor_phys.y as f32, zx, zy, zw)
    }

    /// The keep-open region: the hot zone unioned with the visible bar rect, plus
    /// a small margin so a graze along the edge does not flicker. Leaving it (and
    /// not typing) arms the hysteresis hide.
    fn in_bar_keep_region(&self) -> bool {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (zx, zy, zw, _) = zone_rect(w, h);
        let (_, _, _, bh) = self.bar_rect(w, h);
        let bottom = (bh * self.bar_progress).max(zy);
        keep_region_contains(
            self.cursor_phys.x as f32,
            self.cursor_phys.y as f32,
            zx,
            zw,
            bottom,
            6.0 * self.scale,
        )
    }

    /// Top-left origin (device px) of the active view's rectangle.
    fn active_origin(&self) -> (f32, f32) {
        let (w, h) = self.renderer.as_ref().map(|r| r.size()).unwrap_or((1, 1));
        let (x, y, _, _) = if self.overlay == Overlay::Closed {
            zone_rect(w, h)
        } else {
            self.internal_rect(w, h)
        };
        (x, y)
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
                browser::set_focus(Role::Surf, true);
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
                browser::set_focus(Role::Surf, false);
                browser::set_focus(Role::Internal, true);
            }
            Overlay::Bar => {
                browser::show_internal_command();
                browser::set_focus(Role::Surf, false);
                browser::set_focus(Role::Internal, true);
            }
            Overlay::Closed => {
                browser::set_focus(Role::Internal, false);
                browser::set_focus(Role::Surf, true);
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
            let (_, _, zw, zh) = zone_rect(w, h);
            browser::set_view_geometry(Role::Surf, zw as u32, zh as u32, self.scale);
            let (_, _, iw, ih) = self.internal_rect(w, h);
            browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
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
                self.push_geometry();
                browser::notify_resized(Role::Surf);
                browser::notify_resized(Role::Internal);
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.push_geometry();
                browser::notify_resized(Role::Surf);
                browser::notify_resized(Role::Internal);
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
                if !over_gear {
                    let (x, y) = self.view_coords(self.active_origin());
                    browser::send_mouse_move(self.active_role(), x, y, self.mouse_mods(), false);
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
                // Mouse buttons 4/5 are Back/Forward on the surf view.
                if down && self.active_role() == Role::Surf {
                    match button {
                        MouseButton::Back => {
                            browser::go_back(Role::Surf);
                            return;
                        }
                        MouseButton::Forward => {
                            browser::go_forward(Role::Surf);
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
                let (x, y) = self.view_coords(self.active_origin());
                browser::send_mouse_button(self.active_role(), x, y, self.mouse_mods(), button, down, 1);
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (x, y) = self.view_coords(self.active_origin());
                browser::send_mouse_wheel(self.active_role(), x, y, self.mouse_mods(), dx as i32, dy as i32);
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
                // Surf navigation shortcuts (only while the surf view is active).
                if event.state == ElementState::Pressed
                    && self.active_role() == Role::Surf
                    && let PhysicalKey::Code(code) = event.physical_key
                {
                    let (ctrl, alt, shift) =
                        (self.mods.control_key(), self.mods.alt_key(), self.mods.shift_key());
                    match code {
                        KeyCode::F5 => {
                            browser::reload(Role::Surf);
                            return;
                        }
                        KeyCode::KeyR if ctrl => {
                            if shift {
                                browser::reload_ignore_cache(Role::Surf);
                            } else {
                                browser::reload(Role::Surf);
                            }
                            return;
                        }
                        KeyCode::ArrowLeft if alt => {
                            browser::go_back(Role::Surf);
                            return;
                        }
                        KeyCode::ArrowRight if alt => {
                            browser::go_forward(Role::Surf);
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
                let (scale, hover, load) = (self.scale, self.gear_hover, self.loading_intensity);
                let is_bar = self.overlay == Overlay::Bar;
                // The overlay is composited while settings is open, or while the
                // bar has any of itself showing (during the slide up/down).
                let open = match self.overlay {
                    Overlay::Settings => true,
                    Overlay::Bar => self.bar_progress > 0.001,
                    Overlay::Closed => false,
                };
                let bar_progress = self.bar_progress;
                let internal = self.renderer.as_ref().map(|r| {
                    let (w, h) = r.size();
                    self.internal_rect(w, h)
                });
                if let (Some(r), Some(internal)) = (self.renderer.as_mut(), internal) {
                    let (w, h) = r.size();
                    r.render(
                        time,
                        zone_rect(w, h),
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
                        load,
                    );
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Create both OSR views once the CEF context is initialised.
        if !self.views_started
            && browser::context_ready()
            && let Some(window) = self.window.clone()
        {
            self.push_geometry();
            let hwnd = window_hwnd(&window);
            browser::create_browser(Role::Surf, hwnd);
            browser::create_browser(Role::Internal, hwnd);
            self.views_started = true;
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

        // Ease the loading line toward on (loading) / off (done).
        let load_target = if browser::surf_loading() { 1.0 } else { 0.0 };
        self.loading_intensity += (load_target - self.loading_intensity) * 0.15;

        // In windowed dev mode, reflect the page title in the OS window title.
        if self.windowed {
            let title = browser::surf_title();
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

        // Apply a pending cursor request from the active view.
        if let Some(icon) = browser::take_cursor(self.active_role())
            && icon != self.applied_cursor
            && let Some(window) = self.window.as_ref()
        {
            window.set_cursor(icon);
            self.applied_cursor = icon;
        }

        // Upload freshly painted frames into their textures.
        if let Some(r) = self.renderer.as_mut() {
            browser::with_dirty_frame(Role::Surf, |data, w, h| r.upload_page(data, w, h));
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
