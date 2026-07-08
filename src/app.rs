//! winit application: window, render loop, surf-zone + settings geometry, input
//! forwarding into CEF (OSR), the gear button, cursor feedback, ESC handling.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

use crate::browser::{self, Role};
use crate::renderer::SurfaceRenderer;
use crate::settings;
use crate::theme::Theme;

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

/// Command-bar rectangle in device pixels: a centered strip in the upper third.
fn command_rect(width: u32, height: u32, scale: f32) -> (f32, f32, f32, f32) {
    let (w, h) = (width as f32, height as f32);
    let pw = (w * 0.5).clamp(560.0, 960.0).min(w);
    let ph = (76.0 * scale).min(h);
    let px = ((w - pw) * 0.5).round();
    let py = (h * 0.20).round();
    (px, py, pw.round(), ph.round())
}

/// Which internal overlay (if any) is currently shown.
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
        isolation_tested: false,
    };
    event_loop.run_app(&mut app).expect("event loop error");

    browser::shutdown_cef();
}

struct Shell {
    windowed: bool,
    window: Option<Arc<Window>>,
    renderer: Option<SurfaceRenderer>,
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
    isolation_tested: bool,
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

    /// The internal view's rectangle (device px) for the current overlay.
    fn internal_rect(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        match self.overlay {
            Overlay::Command => command_rect(w, h, self.scale),
            _ => panel_rect(w, h),
        }
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
        self.overlay = next;
        // Match the internal OSR view's size to the new overlay before it paints.
        if let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            let (_, _, iw, ih) = self.internal_rect(w, h);
            browser::set_view_geometry(Role::Internal, iw as u32, ih as u32, self.scale);
            browser::notify_resized(Role::Internal);
        }
        match next {
            Overlay::Settings => {
                browser::show_internal_settings();
                browser::set_focus(Role::Surf, false);
                browser::set_focus(Role::Internal, true);
            }
            Overlay::Command => {
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
        let renderer = SurfaceRenderer::new(window.clone(), Theme::load());
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

            WindowEvent::Focused(focused) => browser::set_focus(self.active_role(), focused),

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
                // Ctrl+L opens the command bar (from any state).
                if event.state == ElementState::Pressed
                    && self.mods.control_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyL)
                {
                    self.set_overlay(Overlay::Command);
                    return;
                }
                // ESC chain: command bar / settings first, then quit the shell.
                if vk == 0x1B && event.state == ElementState::Pressed {
                    if self.overlay != Overlay::Closed {
                        self.set_overlay(Overlay::Closed);
                    } else {
                        event_loop.exit();
                    }
                    return;
                }
                // Surf navigation shortcuts (only while the surf view is active).
                if event.state == ElementState::Pressed && self.active_role() == Role::Surf {
                    if let PhysicalKey::Code(code) = event.physical_key {
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
                let open = self.overlay != Overlay::Closed;
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
                        settings::deep_field(),
                        open,
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
        if !self.views_started && browser::context_ready() {
            if let Some(window) = self.window.clone() {
                self.push_geometry();
                let hwnd = window_hwnd(&window);
                browser::create_browser(Role::Surf, hwnd);
                browser::create_browser(Role::Internal, hwnd);
                self.views_started = true;
            }
        }

        // The command bar's navigate request closes the overlay (set from the
        // IPC thread).
        if browser::take_overlay_close() {
            self.set_overlay(Overlay::Closed);
        }

        // Ease the gear hover glow toward its target.
        self.gear_hover += (self.gear_hover_target - self.gear_hover) * 0.25;

        // Ease the loading line toward on (loading) / off (done).
        let load_target = if browser::surf_loading() { 1.0 } else { 0.0 };
        self.loading_intensity += (load_target - self.loading_intensity) * 0.15;

        // In windowed dev mode, reflect the page title in the OS window title.
        if self.windowed {
            let title = browser::surf_title();
            if title != self.applied_title {
                if let Some(window) = self.window.as_ref() {
                    let full = if title.is_empty() {
                        "CARVILON CyberDesk".to_string()
                    } else {
                        format!("{title} — CARVILON CyberDesk")
                    };
                    window.set_title(&full);
                    self.applied_title = title;
                }
            }
        }

        // Apply a pending cursor request from the active view.
        if let Some(icon) = browser::take_cursor(self.active_role()) {
            if icon != self.applied_cursor {
                if let Some(window) = self.window.as_ref() {
                    window.set_cursor(icon);
                    self.applied_cursor = icon;
                }
            }
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
