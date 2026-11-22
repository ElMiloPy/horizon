//! Touch controls for Android (adapted from Starling,
//! https://github.com/starling-client/starling):
//! - Off the playing screen (menus, options, inventories, etc.): [`TouchPointer`] turns touch
//!   gestures into mouse pointer clicks, drags, and scroll-wheel events.
//! - On the playing screen: delegates to `horizon_client_render::touch::TouchControls` (all 4
//!   Starling control schemes with MCPE double-bevel pixel-art icons, saved in `touch.json`).
//! - In-game [`TouchSettingsOverlay`] lets players switch schemes and tune scale, opacity,
//!   sensitivity, left-handed layout, and dynamic joystick without leaving the world.

use horizon_client::hud::{HudAtlas, HudBatch, rgba};
use horizon_client_render::touch::{Mode, Settings, TouchControls, layout};
use std::path::PathBuf;
use winit::{
    dpi::PhysicalPosition,
    event::{DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent},
};

/// Menu scroll/tap slop in physical pixels.
pub const SLOP_PX: f32 = 14.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MenuGesture {
    #[default]
    Undecided,
    Scroll,
    Drag,
}

/// Off-world touch pointer: translates touch interactions on UI screens into mouse clicks/scrolls.
#[derive(Default)]
pub struct TouchPointer {
    finger: Option<u64>,
    start: [f32; 2],
    last: [f32; 2],
    gesture: MenuGesture,
    scroll_scale: f32,
}

impl TouchPointer {
    pub fn owns(&self, finger: u64) -> bool {
        self.finger == Some(finger)
    }

    pub fn event(&mut self, touch: &Touch, slop: f32) -> Vec<WindowEvent> {
        let p = [touch.location.x as f32, touch.location.y as f32];
        let device_id = DeviceId::dummy();
        let mut out = Vec::new();

        match touch.phase {
            TouchPhase::Started if self.finger.is_none() => {
                self.finger = Some(touch.id);
                self.start = p;
                self.last = p;
                self.gesture = MenuGesture::Undecided;
                self.scroll_scale = 0.5;
                out.push(WindowEvent::CursorMoved {
                    device_id,
                    position: PhysicalPosition::new(p[0] as f64, p[1] as f64),
                });
            }
            TouchPhase::Moved if self.owns(touch.id) => {
                if self.gesture == MenuGesture::Undecided {
                    let dx = p[0] - self.start[0];
                    let dy = p[1] - self.start[1];
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist > slop {
                        if dy.abs() > dx.abs() {
                            self.gesture = MenuGesture::Scroll;
                        } else {
                            self.gesture = MenuGesture::Drag;
                            out.push(WindowEvent::MouseInput {
                                device_id,
                                state: ElementState::Pressed,
                                button: MouseButton::Left,
                            });
                        }
                    }
                }

                match self.gesture {
                    MenuGesture::Scroll => {
                        let step = (p[1] - self.last[1]) * self.scroll_scale;
                        out.push(WindowEvent::MouseWheel {
                            device_id,
                            delta: MouseScrollDelta::LineDelta(0.0, step),
                            phase: TouchPhase::Moved,
                        });
                    }
                    MenuGesture::Drag => {
                        out.push(WindowEvent::CursorMoved {
                            device_id,
                            position: PhysicalPosition::new(p[0] as f64, p[1] as f64),
                        });
                    }
                    MenuGesture::Undecided => {}
                }
                self.last = p;
            }
            TouchPhase::Ended | TouchPhase::Cancelled if self.owns(touch.id) => {
                self.finger = None;
                match self.gesture {
                    MenuGesture::Undecided if touch.phase == TouchPhase::Ended => {
                        out.push(WindowEvent::CursorMoved {
                            device_id,
                            position: PhysicalPosition::new(p[0] as f64, p[1] as f64),
                        });
                        out.push(WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Pressed,
                            button: MouseButton::Left,
                        });
                        out.push(WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Released,
                            button: MouseButton::Left,
                        });
                    }
                    MenuGesture::Drag => {
                        out.push(WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Released,
                            button: MouseButton::Left,
                        });
                    }
                    _ => {}
                }
                self.gesture = MenuGesture::Undecided;
            }
            _ => {}
        }
        out
    }
}

/// Default settings for first-time touch users: Joystick & Tap-to-Interact with
/// joystick sprinting, matching Starling/MCPE defaults while allowing one-tap
/// switching to all 4 schemes via the Settings gear button.
pub fn default_touch_settings() -> Settings {
    let mut s = Settings::default();
    s.mode = Mode::Tap;
    s.sprint_with_joystick = true;
    s
}

/// Path where touch settings are persisted (`<HORIZON_HOME>/touch.json`).
pub fn touch_settings_path() -> PathBuf {
    horizon_client::logging::player_dir().join("touch.json")
}

/// Loads saved touch settings or falls back to [`default_touch_settings`].
pub fn load_touch_controls() -> TouchControls {
    let path = touch_settings_path();
    let settings = if path.is_file() {
        Settings::load(&path)
    } else {
        default_touch_settings()
    };
    let mut tc = TouchControls::default();
    tc.settings = settings;
    tc
}

/// Saves touch settings to `<HORIZON_HOME>/touch.json`.
pub fn save_touch_settings(settings: &Settings) {
    let path = touch_settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = settings.save(&path) {
        log::warn!("Could not save touch settings ({}): {e}", path.display());
    }
}

/// Tracks a finger pressing the Esc icon button while an inventory/container screen is open.
#[derive(Default)]
pub struct EscButtonState {
    pub finger: Option<u64>,
}

impl EscButtonState {
    /// Handles a touch event against `esc_rect` (normalized `[x, y, w, h]`).
    /// Returns `(consumed, triggered)`.
    pub fn handle(
        &mut self,
        touch: &Touch,
        surface_w: f32,
        surface_h: f32,
        esc_rect: [f32; 4],
    ) -> (bool, bool) {
        let point = [
            touch.location.x as f32 / surface_w.max(1.0),
            touch.location.y as f32 / surface_h.max(1.0),
        ];
        // Slight padding around the visual button so quick taps register comfortably.
        let pad_x = esc_rect[2] * 0.2;
        let pad_y = esc_rect[3] * 0.2;
        let padded = [
            esc_rect[0] - pad_x,
            esc_rect[1] - pad_y,
            esc_rect[2] + pad_x * 2.0,
            esc_rect[3] + pad_y * 2.0,
        ];
        let inside = layout::contains(padded, point);
        match touch.phase {
            TouchPhase::Started => {
                if inside {
                    self.finger = Some(touch.id);
                    (true, false)
                } else {
                    (false, false)
                }
            }
            TouchPhase::Moved => (self.finger == Some(touch.id), false),
            TouchPhase::Ended => {
                if self.finger == Some(touch.id) {
                    self.finger = None;
                    (true, inside)
                } else {
                    (false, false)
                }
            }
            TouchPhase::Cancelled => {
                if self.finger == Some(touch.id) {
                    self.finger = None;
                    (true, false)
                } else {
                    (false, false)
                }
            }
        }
    }

    pub fn is_pressed(&self) -> bool {
        self.finger.is_some()
    }
}

/// Action returned by clicking inside [`TouchSettingsOverlay`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchSettingsAction {
    None,
    Changed,
    Close,
}

#[derive(Clone, Copy)]
enum SettingsBtn {
    Scheme(usize),
    LeftHanded,
    DynamicJoystick,
    SwapJumpSneak,
    SprintJoystick,
    ScaleDown,
    ScaleUp,
    SensDown,
    SensUp,
    OpacityDown,
    OpacityUp,
    Reset,
    Done,
}

/// In-game Touch Settings panel (adapted from Starling's touch settings screen).
#[derive(Default)]
pub struct TouchSettingsOverlay {
    buttons: Vec<([f32; 4], SettingsBtn)>,
}

impl TouchSettingsOverlay {
    fn current_opacity(settings: &Settings) -> f32 {
        let row = &settings.layouts[settings.mode.index()];
        row.first().map_or(0.65, |p| p.opacity)
    }

    fn set_all_opacity(settings: &mut Settings, opacity: f32) {
        let v = opacity.clamp(0.2, 1.0);
        for row in &mut settings.layouts {
            for p in row.iter_mut() {
                p.opacity = v;
            }
        }
    }

    /// Draws the Touch Settings overlay in GUI pixels and updates hit rectangles.
    pub fn draw(
        &mut self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        settings: &Settings,
        cursor: (f32, f32),
    ) {
        self.buttons.clear();
        let (gw, gh) = (batch.width, batch.height);

        // Backdrop scrim.
        batch.fill(atlas, 0.0, 0.0, gw, gh, rgba(0, 0, 0, 165));

        let panel_w = (gw - 24.0).clamp(240.0, 340.0);
        let panel_h = (gh - 20.0).clamp(185.0, 232.0);
        let px = ((gw - panel_w) * 0.5).floor();
        let py = ((gh - panel_h) * 0.5).floor();

        draw_panel(batch, atlas, px, py, panel_w, panel_h);

        let title = "Touch Controls";
        let tw = HudBatch::text_width(atlas, title);
        batch.text(
            atlas,
            title,
            px + (panel_w - tw) * 0.5,
            py + 8.0,
            rgba(255, 255, 255, 255),
            true,
        );

        let pad = 10.0;
        let inner_w = panel_w - pad * 2.0;
        let mut y = py + 22.0;

        // 1. Control Schemes (2x2 grid).
        batch.text(atlas, "Control Scheme", px + pad, y, rgba(190, 195, 210, 255), true);
        y += 11.0;
        let gap = 4.0;
        let col_w = (inner_w - gap) * 0.5;
        let btn_h = 18.0;
        for (idx, (mode, split, label)) in Mode::SCHEMES.iter().enumerate() {
            let col = (idx % 2) as f32;
            let row = (idx / 2) as f32;
            let bx = px + pad + col * (col_w + gap);
            let by = y + row * (btn_h + gap);
            let active = settings.mode == *mode && settings.split_controls == *split;
            let short = match idx {
                0 => "Joystick + Crosshair",
                1 => "Joystick + Tap",
                2 => "D-Pad + Tap",
                3 => "D-Pad + Crosshair",
                _ => label,
            };
            let rect = [bx, by, col_w, btn_h];
            draw_button(batch, atlas, rect, short, active, hit(rect, cursor));
            self.buttons.push((rect, SettingsBtn::Scheme(idx)));
        }
        y += (btn_h + gap) * 2.0 + 4.0;

        // 2. Toggles (2x2 grid).
        let toggles = [
            (
                if settings.left_handed { "Left-Handed: ON" } else { "Left-Handed: OFF" },
                settings.left_handed,
                SettingsBtn::LeftHanded,
            ),
            (
                if settings.dynamic_joystick { "Dynamic Stick: ON" } else { "Dynamic Stick: OFF" },
                settings.dynamic_joystick,
                SettingsBtn::DynamicJoystick,
            ),
            (
                if settings.swap_jump_sneak { "Swap Jump/Sneak: ON" } else { "Swap Jump/Sneak: OFF" },
                settings.swap_jump_sneak,
                SettingsBtn::SwapJumpSneak,
            ),
            (
                if settings.sprint_with_joystick { "Stick Sprint: ON" } else { "Stick Sprint: OFF" },
                settings.sprint_with_joystick,
                SettingsBtn::SprintJoystick,
            ),
        ];
        for (idx, (label, active, kind)) in toggles.into_iter().enumerate() {
            let col = (idx % 2) as f32;
            let row = (idx / 2) as f32;
            let bx = px + pad + col * (col_w + gap);
            let by = y + row * (btn_h + gap);
            let rect = [bx, by, col_w, btn_h];
            draw_button(batch, atlas, rect, label, active, hit(rect, cursor));
            self.buttons.push((rect, kind));
        }
        y += (btn_h + gap) * 2.0 + 4.0;

        // 3. Steppers: Scale, Sensitivity, Opacity.
        let steppers = [
            (
                format!("Button Size: {}%", (settings.button_scale * 100.0).round() as i32),
                SettingsBtn::ScaleDown,
                SettingsBtn::ScaleUp,
            ),
            (
                format!("Sensitivity: {}%", (settings.sensitivity * 100.0).round() as i32),
                SettingsBtn::SensDown,
                SettingsBtn::SensUp,
            ),
            (
                format!(
                    "Opacity: {}%",
                    (Self::current_opacity(settings) * 100.0).round() as i32
                ),
                SettingsBtn::OpacityDown,
                SettingsBtn::OpacityUp,
            ),
        ];
        let step_h = 16.0;
        let step_w = 24.0;
        for (label, down_btn, up_btn) in steppers {
            batch.text(atlas, &label, px + pad, y + 4.0, rgba(235, 235, 240, 255), true);
            let minus = [px + panel_w - pad - step_w * 2.0 - 4.0, y, step_w, step_h];
            let plus = [px + panel_w - pad - step_w, y, step_w, step_h];
            draw_button(batch, atlas, minus, "-", false, hit(minus, cursor));
            draw_button(batch, atlas, plus, "+", false, hit(plus, cursor));
            self.buttons.push((minus, down_btn));
            self.buttons.push((plus, up_btn));
            y += step_h + 3.0;
        }

        // 4. Footer: Reset & Done.
        let foot_y = (py + panel_h - 26.0).max(y + 2.0);
        let reset_rect = [px + pad, foot_y, col_w, 18.0];
        let done_rect = [px + pad + col_w + gap, foot_y, col_w, 18.0];
        draw_button(batch, atlas, reset_rect, "Reset Defaults", false, hit(reset_rect, cursor));
        draw_button(batch, atlas, done_rect, "Done", true, hit(done_rect, cursor));
        self.buttons.push((reset_rect, SettingsBtn::Reset));
        self.buttons.push((done_rect, SettingsBtn::Done));
    }

    /// Handles a click in GUI coordinates `(x, y)`.
    pub fn click(&self, x: f32, y: f32, settings: &mut Settings) -> TouchSettingsAction {
        let Some((_, btn)) = self.buttons.iter().find(|(r, _)| hit(*r, (x, y))) else {
            return TouchSettingsAction::None;
        };
        match *btn {
            SettingsBtn::Scheme(idx) => {
                if let Some((mode, split, _)) = Mode::SCHEMES.get(idx) {
                    settings.mode = *mode;
                    settings.split_controls = *split;
                    settings.sanitize();
                }
                TouchSettingsAction::Changed
            }
            SettingsBtn::LeftHanded => {
                settings.left_handed = !settings.left_handed;
                TouchSettingsAction::Changed
            }
            SettingsBtn::DynamicJoystick => {
                settings.dynamic_joystick = !settings.dynamic_joystick;
                TouchSettingsAction::Changed
            }
            SettingsBtn::SwapJumpSneak => {
                settings.swap_jump_sneak = !settings.swap_jump_sneak;
                TouchSettingsAction::Changed
            }
            SettingsBtn::SprintJoystick => {
                settings.sprint_with_joystick = !settings.sprint_with_joystick;
                TouchSettingsAction::Changed
            }
            SettingsBtn::ScaleDown => {
                settings.button_scale = (settings.button_scale - 0.1).clamp(0.5, 2.0);
                TouchSettingsAction::Changed
            }
            SettingsBtn::ScaleUp => {
                settings.button_scale = (settings.button_scale + 0.1).clamp(0.5, 2.0);
                TouchSettingsAction::Changed
            }
            SettingsBtn::SensDown => {
                settings.sensitivity = (settings.sensitivity - 0.1).clamp(0.2, 3.0);
                TouchSettingsAction::Changed
            }
            SettingsBtn::SensUp => {
                settings.sensitivity = (settings.sensitivity + 0.1).clamp(0.2, 3.0);
                TouchSettingsAction::Changed
            }
            SettingsBtn::OpacityDown => {
                let cur = Self::current_opacity(settings);
                Self::set_all_opacity(settings, cur - 0.1);
                TouchSettingsAction::Changed
            }
            SettingsBtn::OpacityUp => {
                let cur = Self::current_opacity(settings);
                Self::set_all_opacity(settings, cur + 0.1);
                TouchSettingsAction::Changed
            }
            SettingsBtn::Reset => {
                *settings = default_touch_settings();
                TouchSettingsAction::Changed
            }
            SettingsBtn::Done => TouchSettingsAction::Close,
        }
    }
}

fn hit([x, y, w, h]: [f32; 4], (cx, cy): (f32, f32)) -> bool {
    cx >= x && cx <= x + w && cy >= y && cy <= y + h
}

fn draw_panel(b: &mut HudBatch, a: &HudAtlas, x: f32, y: f32, w: f32, h: f32) {
    b.fill(a, x, y, w, h, rgba(16, 18, 24, 240));
    let border = rgba(120, 130, 150, 230);
    b.fill(a, x, y, w, 1.0, border);
    b.fill(a, x, y + h - 1.0, w, 1.0, border);
    b.fill(a, x, y, 1.0, h, border);
    b.fill(a, x + w - 1.0, y, 1.0, h, border);
}

fn draw_button(
    b: &mut HudBatch,
    a: &HudAtlas,
    [x, y, w, h]: [f32; 4],
    label: &str,
    active: bool,
    hovered: bool,
) {
    let bg = if active {
        rgba(64, 128, 84, 240)
    } else if hovered {
        rgba(84, 92, 112, 235)
    } else {
        rgba(52, 56, 68, 230)
    };
    let hi = if active {
        rgba(128, 210, 148, 235)
    } else {
        rgba(140, 146, 162, 200)
    };
    let lo = rgba(20, 22, 28, 240);
    b.fill(a, x, y, w, h, lo);
    b.fill(a, x + 1.0, y + 1.0, (w - 2.0).max(1.0), (h - 2.0).max(1.0), bg);
    b.fill(a, x + 1.0, y + 1.0, (w - 2.0).max(1.0), 1.0, hi);
    b.fill(a, x + 1.0, y + 1.0, 1.0, (h - 2.0).max(1.0), hi);

    let tw = HudBatch::text_width(a, label);
    let start = b.vertices.len();
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    b.text(a, label, cx - tw * 0.5, cy - 4.0, rgba(248, 248, 252, 255), true);
    let scale = ((w - 6.0).max(1.0) / tw.max(1.0)).min(1.0);
    if scale < 1.0 {
        b.scale_since(start, cx, cy, scale, scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_touch(id: u64, phase: TouchPhase, x: f64, y: f64) -> Touch {
        Touch {
            device_id: DeviceId::dummy(),
            phase,
            location: PhysicalPosition::new(x, y),
            force: None,
            id,
        }
    }

    #[test]
    fn touch_pointer_tap_produces_click() {
        let mut ptr = TouchPointer::default();
        let evs1 = ptr.event(&mk_touch(1, TouchPhase::Started, 100.0, 100.0), SLOP_PX);
        assert!(matches!(evs1.as_slice(), [WindowEvent::CursorMoved { .. }]));
        let evs2 = ptr.event(&mk_touch(1, TouchPhase::Ended, 102.0, 101.0), SLOP_PX);
        assert_eq!(evs2.len(), 3);
        assert!(matches!(
            evs2[1],
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            }
        ));
        assert!(matches!(
            evs2[2],
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            }
        ));
    }

    #[test]
    fn touch_pointer_vertical_drag_produces_wheel() {
        let mut ptr = TouchPointer::default();
        let _ = ptr.event(&mk_touch(1, TouchPhase::Started, 100.0, 100.0), SLOP_PX);
        let evs = ptr.event(&mk_touch(1, TouchPhase::Moved, 100.0, 130.0), SLOP_PX);
        assert!(matches!(evs.as_slice(), [WindowEvent::MouseWheel { .. }]));
    }

    #[test]
    fn esc_button_triggers_on_release_inside_rect() {
        let mut esc = EscButtonState::default();
        let rect = [0.45, 0.0, 0.1, 0.12];
        assert_eq!(
            esc.handle(&mk_touch(1, TouchPhase::Started, 500.0, 50.0), 1000.0, 1000.0, rect),
            (true, false)
        );
        assert!(esc.is_pressed());
        assert_eq!(
            esc.handle(&mk_touch(1, TouchPhase::Ended, 500.0, 50.0), 1000.0, 1000.0, rect),
            (true, true)
        );
        assert!(!esc.is_pressed());
    }

    #[test]
    fn touch_settings_overlay_switches_scheme() {
        let mut settings = default_touch_settings();
        let mut overlay = TouchSettingsOverlay::default();
        let atlas = HudAtlas::default();
        let mut batch = HudBatch::new([1280, 720], Default::default());
        overlay.draw(&mut batch, &atlas, &settings, (-1.0, -1.0));
        let (rect, _) = overlay
            .buttons
            .iter()
            .find(|(_, b)| matches!(b, SettingsBtn::Scheme(3)))
            .copied()
            .expect("D-Pad + Crosshair scheme button exists");
        let res = overlay.click(rect[0] + 4.0, rect[1] + 4.0, &mut settings);
        assert_eq!(res, TouchSettingsAction::Changed);
        assert_eq!(settings.mode, Mode::Dpad);
        assert!(settings.split_controls);
    }
}
