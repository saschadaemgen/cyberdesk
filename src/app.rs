//! winit application: window, render loop, surf-zone geometry, input forwarding
//! into CEF (OSR), cursor feedback, and clean ESC exit.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

use crate::browser;
use crate::renderer::SurfaceRenderer;

/// Rounded-corner radius for the page (device pixels). Stage B: sharp (0);
/// Stage C introduces the rounded-corner effect.
const PAGE_CORNER_RADIUS: f32 = 0.0;

/// Surf-zone rectangle in device pixels: 60% width, 70% height, centered.
fn zone_rect(width: u32, height: u32) -> (f32, f32, f32, f32) {
    let (w, h) = (width as f32, height as f32);
    let zw = (w * 0.60).round();
    let zh = (h * 0.70).round();
    let zx = ((w - zw) * 0.5).round();
    let zy = ((h - zh) * 0.5).round();
    (zx, zy, zw, zh)
}

pub fn run(windowed: bool) {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = Shell {
        windowed,
        window: None,
        renderer: None,
        start: Instant::now(),
        cef_inited: false,
        browser_started: false,
        scale: 1.0,
        cursor_phys: PhysicalPosition::new(0.0, 0.0),
        key_mods: 0,
        button_flags: 0,
        applied_cursor: CursorIcon::Default,
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
    browser_started: bool,
    scale: f32,
    cursor_phys: PhysicalPosition<f64>,
    key_mods: u32,
    button_flags: u32,
    applied_cursor: CursorIcon,
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
    /// Cursor position translated into surf-zone view coordinates (DIP).
    fn view_coords(&self) -> (i32, i32) {
        let (zx, zy) = match self.renderer.as_ref() {
            Some(r) => {
                let (w, h) = r.size();
                let (zx, zy, _, _) = zone_rect(w, h);
                (zx, zy)
            }
            None => (0.0, 0.0),
        };
        let vx = ((self.cursor_phys.x - zx as f64) / self.scale as f64) as i32;
        let vy = ((self.cursor_phys.y - zy as f64) / self.scale as f64) as i32;
        (vx, vy)
    }

    fn mouse_mods(&self) -> u32 {
        self.key_mods | self.button_flags
    }

    fn push_geometry(&mut self) {
        if let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            let (_, _, zw, zh) = zone_rect(w, h);
            browser::set_view_geometry(zw as u32, zh as u32, self.scale);
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
        let renderer = SurfaceRenderer::new(window.clone());
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

            WindowEvent::Focused(focused) => browser::set_focus(focused),

            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
                self.push_geometry();
                browser::notify_resized();
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.push_geometry();
                browser::notify_resized();
            }

            WindowEvent::ModifiersChanged(mods) => {
                let s = mods.state();
                self.key_mods = browser::modifier_flags(
                    s.shift_key(),
                    s.control_key(),
                    s.alt_key(),
                );
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_phys = position;
                let (x, y) = self.view_coords();
                browser::send_mouse_move(x, y, self.mouse_mods(), false);
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let flag = match button {
                    MouseButton::Left => browser::EVENTFLAG_LEFT_MOUSE_BUTTON,
                    MouseButton::Middle => browser::EVENTFLAG_MIDDLE_MOUSE_BUTTON,
                    MouseButton::Right => browser::EVENTFLAG_RIGHT_MOUSE_BUTTON,
                    _ => 0,
                };
                let down = state == ElementState::Pressed;
                if down {
                    self.button_flags |= flag;
                } else {
                    self.button_flags &= !flag;
                }
                let (x, y) = self.view_coords();
                browser::send_mouse_button(x, y, self.mouse_mods(), button, down, 1);
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (x, y) = self.view_coords();
                browser::send_mouse_wheel(x, y, self.mouse_mods(), dx as i32, dy as i32);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let vk = match event.physical_key {
                    PhysicalKey::Code(code) => keycode_to_vk(code),
                    _ => 0,
                };
                // ESC quits the shell — never forwarded to the page.
                if vk == 0x1B && event.state == ElementState::Pressed {
                    event_loop.exit();
                    return;
                }
                match event.state {
                    ElementState::Pressed => {
                        browser::send_key_down(vk, self.key_mods);
                        if let Some(text) = event.text.as_ref() {
                            for ch in text.encode_utf16() {
                                browser::send_char(ch, self.key_mods);
                            }
                        }
                    }
                    ElementState::Released => browser::send_key_up(vk, self.key_mods),
                }
            }

            WindowEvent::RedrawRequested => {
                let time = self.start.elapsed().as_secs_f32();
                if let Some(r) = self.renderer.as_mut() {
                    let (w, h) = r.size();
                    let zone = zone_rect(w, h);
                    r.render(time, zone, PAGE_CORNER_RADIUS);
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Create the OSR browser once the CEF context is initialised.
        if !self.browser_started && browser::context_ready() {
            if let Some(window) = self.window.clone() {
                self.push_geometry();
                browser::create_browser(window_hwnd(&window));
                self.browser_started = true;
            }
        }

        // Apply a pending cursor request from the page.
        if let Some(icon) = browser::take_cursor() {
            if icon != self.applied_cursor {
                if let Some(window) = self.window.as_ref() {
                    window.set_cursor(icon);
                    self.applied_cursor = icon;
                }
            }
        }

        // Upload a freshly painted CEF frame into the page texture.
        if let Some(r) = self.renderer.as_mut() {
            browser::with_dirty_frame(|data, w, h| r.upload_page(data, w, h));
        }

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}
