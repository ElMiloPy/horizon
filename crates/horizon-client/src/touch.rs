//! Touch controls for Android (Bedrock/Starling layout):
//! - Off the playing screen (menus, options, inventories, etc.): touches become mouse pointer events.
//! - On the playing screen:
//!   - Floating stick on the bottom-left controls movement and sprinting.
//!   - Action buttons (Jump, Sneak, Pause, Chat, Bag) on clean, translucent Bedrock-style discs.
//!   - World touches:
//!     - Tap (< 300ms, still): Use / Interact / Place block.
//!     - Hold (>= 300ms, still): Mine / Break block.
//!     - Drag: Smooth camera look.
//!   - Hotbar: Tapping hotbar slots selects the active slot.

use std::time::{Duration, Instant};
use winit::{
    dpi::PhysicalPosition,
    event::{DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent},
};
use horizon_client::hud::{HudAtlas, HudBatch};

/// Menu scroll/tap slop in physical pixels.
pub const SLOP_PX: f32 = 14.0;
/// Holding still this long starts breaking blocks in view.
pub const HOLD: Duration = Duration::from_millis(300);

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TouchButton {
    Jump,
    Sneak,
    Pause,
    Chat,
    Inventory,
}

pub enum TouchAction {
    Stick([f32; 2]),
    Sprint(bool),
    Look(f64, f64),
    ButtonDown(TouchButton),
    ButtonUp(TouchButton),
    AttackStart,
    AttackStop,
    Use([f32; 2]),
    SelectSlot(usize),
}

#[derive(Clone, Debug)]
enum FingerRole {
    Stick {
        origin: [f32; 2],
        current: [f32; 2],
        sprinting: bool,
    },
    Look {
        start: [f32; 2],
        last: [f32; 2],
        down: Instant,
        dragged: bool,
        attacking: bool,
    },
    Button(TouchButton),
}

#[derive(Default)]
pub struct TouchControls {
    fingers: Vec<(u64, FingerRole)>,
    held_buttons: Vec<TouchButton>,
    pub esc_finger: Option<u64>,
}

impl TouchControls {
    pub fn reset(&mut self) -> Vec<TouchAction> {
        self.esc_finger = None;
        self.reset_gameplay()
    }

    pub fn reset_gameplay(&mut self) -> Vec<TouchAction> {
        let mut out = Vec::new();
        for btn in self.held_buttons.drain(..) {
            out.push(TouchAction::ButtonUp(btn));
        }
        for (_, role) in self.fingers.drain(..) {
            match role {
                FingerRole::Stick { sprinting, .. } => {
                    out.push(TouchAction::Stick([0.0, 0.0]));
                    if sprinting {
                        out.push(TouchAction::Sprint(false));
                    }
                }
                FingerRole::Look { attacking: true, .. } => {
                    out.push(TouchAction::AttackStop);
                }
                _ => {}
            }
        }
        out
    }

    /// Action buttons laid out matching Starling / Bedrock proportions.
    /// Returns: (button, center [x, y], radius, label).
    pub fn buttons(&self, gw: f32, gh: f32) -> Vec<(TouchButton, [f32; 2], f32, &'static str)> {
        let u = gw.min(gh) / 9.0;
        vec![
            (TouchButton::Jump, [0.905 * gw, 0.7556 * gh], 0.8 * u, "Jump"),
            (TouchButton::Sneak, [0.805 * gw, 0.8556 * gh], 0.6 * u, "Sneak"),
            (TouchButton::Pause, [0.50 * gw, 0.0833 * gh], 0.45 * u, "Esc"),
            (TouchButton::Chat, [0.88 * gw, 0.111 * gh], 0.55 * u, "Chat"),
            (TouchButton::Inventory, [0.95 * gw, 0.111 * gh], 0.55 * u, "Bag"),
        ]
    }

    /// Center and radius of the Esc button in GUI coordinates.
    pub fn esc_button(&self, gw: f32, gh: f32) -> ([f32; 2], f32) {
        let u = gw.min(gh) / 9.0;
        ([0.50 * gw, 0.0833 * gh], 0.45 * u)
    }

    /// Checks and handles touch input on the Esc button when an overlay/inventory screen is open.
    /// Returns `(consumed, triggered)`.
    pub fn handle_esc_touch(&mut self, touch: &Touch, gui_scale: f32, gw: f32, gh: f32) -> (bool, bool) {
        let (center, radius) = self.esc_button(gw, gh);
        let p = [
            (touch.location.x as f32) / gui_scale.max(1.0),
            (touch.location.y as f32) / gui_scale.max(1.0),
        ];
        let dx = p[0] - center[0];
        let dy = p[1] - center[1];
        let in_esc = (dx * dx + dy * dy).sqrt() <= radius * 1.35;

        match touch.phase {
            TouchPhase::Started => {
                if in_esc {
                    self.esc_finger = Some(touch.id);
                    (true, false)
                } else {
                    (false, false)
                }
            }
            TouchPhase::Moved => {
                if self.esc_finger == Some(touch.id) {
                    (true, false)
                } else {
                    (false, false)
                }
            }
            TouchPhase::Ended => {
                if self.esc_finger == Some(touch.id) {
                    self.esc_finger = None;
                    (true, in_esc)
                } else {
                    (false, false)
                }
            }
            TouchPhase::Cancelled => {
                if self.esc_finger == Some(touch.id) {
                    self.esc_finger = None;
                    (true, false)
                } else {
                    (false, false)
                }
            }
        }
    }

    /// Draws the Esc button when an inventory or container screen is open.
    pub fn draw_esc(&self, batch: &mut HudBatch, atlas: &HudAtlas) {
        let (gw, gh) = (batch.width, batch.height);
        let scale = batch.scale;
        let mut ui = horizon_client::ui_theme::Ui::new(atlas, batch, (-1.0, -1.0));
        let to_dp = scale / ui.density;

        let plate = |a: f32| horizon_client::hud::rgba(255, 255, 255, (255.0 * 0.16 * a) as u8);
        let pressed = |a: f32| horizon_client::hud::rgba(255, 255, 255, (255.0 * 0.38 * a) as u8);
        let ink = |a: f32| horizon_client::hud::rgba(255, 255, 255, (255.0 * 0.85 * a) as u8);

        let circle = |ui: &mut horizon_client::ui_theme::Ui, c: [f32; 2], r: f32, fill: u32| {
            let cx = c[0] * to_dp;
            let cy = c[1] * to_dp;
            let cr = r * to_dp;
            ui.rounded(cx - cr, cy - cr, 2.0 * cr, 2.0 * cr, cr, fill);
        };

        let (center, radius) = self.esc_button(gw, gh);
        let is_held = self.esc_finger.is_some();
        let fill = if is_held { pressed(1.0) } else { plate(1.0) };
        circle(&mut ui, center, radius, fill);
        ui.finish();

        let label = "Esc";
        let tw = HudBatch::text_width(atlas, label);
        let tx = center[0] - tw * 0.5;
        let ty = center[1] - 4.0;
        batch.text(atlas, label, tx, ty, ink(1.0), true);
    }

    pub fn handle_touch(&mut self, touch: &Touch, gui_scale: f32, gw: f32, gh: f32) -> Vec<TouchAction> {
        let p = [
            (touch.location.x as f32) / gui_scale.max(1.0),
            (touch.location.y as f32) / gui_scale.max(1.0),
        ];
        let u = gw.min(gh) / 9.0;
        let mut out = Vec::new();

        match touch.phase {
            TouchPhase::Started => {
                // 1. Virtual action buttons (Jump, Sneak, Pause, Chat, Bag)
                for (btn, center, radius, _) in self.buttons(gw, gh) {
                    let dx = p[0] - center[0];
                    let dy = p[1] - center[1];
                    if (dx * dx + dy * dy).sqrt() <= radius * 1.15 {
                        self.fingers.push((touch.id, FingerRole::Button(btn)));
                        if !self.held_buttons.contains(&btn) {
                            self.held_buttons.push(btn);
                            out.push(TouchAction::ButtonDown(btn));
                        }
                        return out;
                    }
                }

                // 2. Hotbar slots (bottom center)
                let hotbar_x = gw / 2.0 - 90.0;
                let hotbar_y = gh - 22.0;
                if p[1] >= hotbar_y && p[1] <= hotbar_y + 22.0 && p[0] >= hotbar_x && p[0] <= hotbar_x + 180.0 {
                    let slot = (((p[0] - hotbar_x) / 20.0).floor() as usize).min(8);
                    out.push(TouchAction::SelectSlot(slot));
                    return out;
                }

                // 3. Floating movement stick (left 40% of screen)
                if p[0] < gw * 0.40 && p[1] > gh * 0.30 {
                    if !self.fingers.iter().any(|(_, r)| matches!(r, FingerRole::Stick { .. })) {
                        self.fingers.push((
                            touch.id,
                            FingerRole::Stick {
                                origin: p,
                                current: p,
                                sprinting: false,
                            },
                        ));
                        return out;
                    }
                }

                // 4. In-world touch: look or tap-to-use / hold-to-break
                self.fingers.push((
                    touch.id,
                    FingerRole::Look {
                        start: p,
                        last: p,
                        down: Instant::now(),
                        dragged: false,
                        attacking: false,
                    },
                ));
            }
            TouchPhase::Moved => {
                let Some((_, role)) = self.fingers.iter_mut().find(|(f, _)| *f == touch.id) else {
                    return out;
                };
                match role {
                    FingerRole::Stick { origin, current, sprinting } => {
                        *current = p;
                        let r = 1.3 * u;
                        let dx = p[0] - origin[0];
                        let dy = p[1] - origin[1];
                        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                        let k = len.min(r) / len;
                        let stick = [dx * k / r, -dy * k / r];
                        out.push(TouchAction::Stick(stick));
                        let forward = stick[1] > stick[0].abs();
                        let sprint = if *sprinting {
                            forward && stick[1] > 0.5
                        } else {
                            forward && len > r * 1.15
                        };
                        if sprint != *sprinting {
                            *sprinting = sprint;
                            out.push(TouchAction::Sprint(sprint));
                        }
                    }
                    FingerRole::Look { start, last, dragged, attacking, .. } => {
                        let dist = ((p[0] - start[0]).powi(2) + (p[1] - start[1]).powi(2)).sqrt();
                        if !*dragged && dist > u * 0.15 {
                            *dragged = true;
                            if *attacking {
                                *attacking = false;
                                out.push(TouchAction::AttackStop);
                            }
                        }
                        if *dragged {
                            let scale = 1200.0 / gh;
                            let dx = (p[0] - last[0]) * scale;
                            let dy = (p[1] - last[1]) * scale;
                            out.push(TouchAction::Look(dx as f64, dy as f64));
                        }
                        *last = p;
                    }
                    FingerRole::Button(..) => {}
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(idx) = self.fingers.iter().position(|(f, _)| *f == touch.id) else {
                    return out;
                };
                let (_, role) = self.fingers.remove(idx);
                match role {
                    FingerRole::Stick { sprinting, .. } => {
                        out.push(TouchAction::Stick([0.0, 0.0]));
                        if sprinting {
                            out.push(TouchAction::Sprint(false));
                        }
                    }
                    FingerRole::Button(btn) => {
                        self.held_buttons.retain(|b| *b != btn);
                        out.push(TouchAction::ButtonUp(btn));
                    }
                    FingerRole::Look { down, dragged, attacking, last, .. } => {
                        if attacking {
                            out.push(TouchAction::AttackStop);
                        } else if touch.phase == TouchPhase::Ended && !dragged && down.elapsed() < HOLD {
                            out.push(TouchAction::Use(last));
                        }
                    }
                }
            }
        }
        out
    }

    /// Evaluates continuous hold state (breaks block after HOLD duration).
    pub fn frame(&mut self, now: Instant) -> Vec<TouchAction> {
        let mut out = Vec::new();
        for (_, role) in &mut self.fingers {
            if let FingerRole::Look { down, dragged: false, attacking, .. } = role {
                if !*attacking && now.duration_since(*down) >= HOLD {
                    *attacking = true;
                    out.push(TouchAction::AttackStart);
                }
            }
        }
        out
    }

    pub fn active_stick(&self) -> Option<([f32; 2], [f32; 2])> {
        self.fingers.iter().find_map(|(_, r)| match r {
            FingerRole::Stick { origin, current, .. } => Some((*origin, *current)),
            _ => None,
        })
    }

    /// Where a touch finger points while it may tap or while it holds to break.
    pub fn aim(&self) -> Option<[f32; 2]> {
        self.fingers.iter().rev().find_map(|(_, role)| match role {
            FingerRole::Look {
                last,
                dragged,
                attacking,
                ..
            } if !*dragged || *attacking => Some(*last),
            _ => None,
        })
    }

    pub fn holding(&self) -> bool {
        self.fingers.iter().any(|(_, role)| match role {
            FingerRole::Look { attacking, .. } => *attacking,
            _ => false,
        })
    }

    /// Renders Starling's clean translucent Bedrock-style touch controls.
    pub fn draw(&self, batch: &mut HudBatch, atlas: &HudAtlas, mining_progress: f32) {
        let (gw, gh) = (batch.width, batch.height);
        let scale = batch.scale;
        let u = gw.min(gh) / 9.0;
        let mut ui = horizon_client::ui_theme::Ui::new(atlas, batch, (-1.0, -1.0));
        let to_dp = scale / ui.density;

        let plate = |a: f32| horizon_client::hud::rgba(255, 255, 255, (255.0 * 0.16 * a) as u8);
        let pressed = |a: f32| horizon_client::hud::rgba(255, 255, 255, (255.0 * 0.38 * a) as u8);
        let ink = |a: f32| horizon_client::hud::rgba(255, 255, 255, (255.0 * 0.85 * a) as u8);

        let circle = |ui: &mut horizon_client::ui_theme::Ui, c: [f32; 2], r: f32, fill: u32| {
            let cx = c[0] * to_dp;
            let cy = c[1] * to_dp;
            let cr = r * to_dp;
            ui.rounded(
                cx - cr,
                cy - cr,
                2.0 * cr,
                2.0 * cr,
                cr,
                fill,
            );
        };
        let ring = |ui: &mut horizon_client::ui_theme::Ui, c: [f32; 2], r: f32, color: u32| {
            let cx = c[0] * to_dp;
            let cy = c[1] * to_dp;
            let cr = r * to_dp;
            ui.border(
                cx - cr,
                cy - cr,
                2.0 * cr,
                2.0 * cr,
                cr,
                color,
            );
        };

        // 1. Movement stick
        let stick_home = [0.12 * gw, 0.733 * gh];
        let stick_radius = 1.3 * u;
        if let Some((origin, current)) = self.active_stick() {
            circle(&mut ui, origin, stick_radius, plate(0.7));
            let dx = current[0] - origin[0];
            let dy = current[1] - origin[1];
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let k = len.min(stick_radius) / len;
            let knob = [origin[0] + dx * k, origin[1] + dy * k];
            circle(&mut ui, knob, stick_radius * 0.45, pressed(0.9));
        } else {
            circle(&mut ui, stick_home, stick_radius, plate(0.35));
            circle(&mut ui, stick_home, stick_radius * 0.45, plate(0.45));
        }

        // 2. Action buttons
        for (btn, center, radius, _) in self.buttons(gw, gh) {
            let is_held = self.held_buttons.contains(&btn);
            let fill = if is_held { pressed(1.0) } else { plate(1.0) };
            circle(&mut ui, center, radius, fill);
        }

        // 3. Touch aim & breaking circle (Bedrock / Starling)
        if let Some(p) = self.aim() {
            let radius = 18.0;
            ring(&mut ui, p, radius + 1.0, horizon_client::hud::rgba(0, 0, 0, 120));
            ring(&mut ui, p, radius, horizon_client::hud::rgba(255, 255, 255, 210));
            if self.holding() && mining_progress > 0.0 {
                let inner = (radius - 2.0).max(0.0) * mining_progress.clamp(0.0, 1.0);
                if inner > 0.0 {
                    circle(&mut ui, p, inner, horizon_client::hud::rgba(255, 255, 255, 80));
                    ring(&mut ui, p, inner, horizon_client::hud::rgba(255, 255, 255, 230));
                }
            }
        }

        ui.finish();

        // 4. Centered button labels
        for (_, center, _, label) in self.buttons(gw, gh) {
            let tw = HudBatch::text_width(atlas, label);
            let tx = center[0] - tw * 0.5;
            let ty = center[1] - 4.0;
            batch.text(atlas, label, tx, ty, ink(1.0), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_touch(id: u64, phase: TouchPhase, x: f64, y: f64) -> Touch {
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
        let mut pointer = TouchPointer::default();
        let t_start = make_touch(1, TouchPhase::Started, 100.0, 100.0);
        let events = pointer.event(&t_start, SLOP_PX);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], WindowEvent::CursorMoved { .. }));

        let t_end = make_touch(1, TouchPhase::Ended, 100.0, 100.0);
        let events = pointer.event(&t_end, SLOP_PX);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], WindowEvent::CursorMoved { .. }));
        assert!(matches!(events[1], WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }));
        assert!(matches!(events[2], WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. }));
    }

    #[test]
    fn touch_pointer_vertical_drag_produces_wheel() {
        let mut pointer = TouchPointer::default();
        pointer.event(&make_touch(1, TouchPhase::Started, 100.0, 100.0), SLOP_PX);
        let events = pointer.event(&make_touch(1, TouchPhase::Moved, 100.0, 130.0), SLOP_PX);
        assert!(!events.is_empty());
        assert!(matches!(events[0], WindowEvent::MouseWheel { .. }));
    }

    #[test]
    fn touch_controls_stick_moves_and_centres() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        // Touch on left side
        let actions = controls.handle_touch(&make_touch(1, TouchPhase::Started, 50.0, 200.0), 1.0, gw, gh);
        assert!(actions.is_empty());
        assert!(controls.active_stick().is_some());

        // Move forward
        let actions = controls.handle_touch(&make_touch(1, TouchPhase::Moved, 50.0, 160.0), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        match actions[0] {
            TouchAction::Stick([sx, sy]) => {
                assert_eq!(sx, 0.0);
                assert!(sy > 0.5);
            }
            _ => panic!("Expected TouchAction::Stick"),
        }

        // Release stick
        let actions = controls.handle_touch(&make_touch(1, TouchPhase::Ended, 50.0, 160.0), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::Stick([0.0, 0.0])));
    }

    #[test]
    fn touch_controls_button_press_and_release() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        let buttons = controls.buttons(gw, gh);
        let (_, center, radius, _) = buttons.iter().find(|(b, ..)| *b == TouchButton::Jump).unwrap();

        // Tap inside Jump circle
        let tap_x = center[0] + radius * 0.5;
        let tap_y = center[1] + radius * 0.5;
        let actions = controls.handle_touch(&make_touch(2, TouchPhase::Started, tap_x as f64, tap_y as f64), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::ButtonDown(TouchButton::Jump)));

        let actions = controls.handle_touch(&make_touch(2, TouchPhase::Ended, tap_x as f64, tap_y as f64), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::ButtonUp(TouchButton::Jump)));
    }

    #[test]
    fn touch_controls_tap_produces_use() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        // Tap in right side of screen (world area)
        let _ = controls.handle_touch(&make_touch(4, TouchPhase::Started, 400.0, 200.0), 1.0, gw, gh);
        let actions = controls.handle_touch(&make_touch(4, TouchPhase::Ended, 400.0, 200.0), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::Use(_)));
    }

    #[test]
    fn touch_controls_drag_produces_look() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        let _ = controls.handle_touch(&make_touch(5, TouchPhase::Started, 400.0, 200.0), 1.0, gw, gh);
        // Drag horizontally
        let actions = controls.handle_touch(&make_touch(5, TouchPhase::Moved, 430.0, 200.0), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        match actions[0] {
            TouchAction::Look(dx, dy) => {
                assert!(dx > 0.0);
                assert_eq!(dy, 0.0);
            }
            _ => panic!("Expected TouchAction::Look"),
        }
    }

    #[test]
    fn touch_controls_hold_produces_attack() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        let _ = controls.handle_touch(&make_touch(6, TouchPhase::Started, 400.0, 200.0), 1.0, gw, gh);
        // Before HOLD: no attack
        let actions = controls.frame(Instant::now());
        assert!(actions.is_empty());

        // After HOLD (350ms): attack starts
        let actions = controls.frame(Instant::now() + Duration::from_millis(350));
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::AttackStart));

        // Lift finger: attack stops
        let actions = controls.handle_touch(&make_touch(6, TouchPhase::Ended, 400.0, 200.0), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::AttackStop));
    }

    #[test]
    fn touch_controls_draw_scales_circles_to_gui_bounds() {
        let atlas = HudAtlas::boot();
        let mut batch = HudBatch::new([1920, 1080]);
        let controls = TouchControls::default();
        controls.draw(&mut batch, &atlas, 0.0);

        let (gw, gh) = (batch.width, batch.height);
        let max_x = batch.vertices.iter().map(|v| v.pos[0]).fold(0.0f32, f32::max);
        let max_y = batch.vertices.iter().map(|v| v.pos[1]).fold(0.0f32, f32::max);
        assert!(max_x > 0.85 * gw, "circles must scale to full GUI bounds: max_x={max_x}, gw={gw}");
        assert!(max_y > 0.70 * gh, "circles must scale to full GUI bounds: max_y={max_y}, gh={gh}");
    }

    #[test]
    fn touch_controls_handle_esc_touch_and_draw_esc() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        let (esc_center, _) = controls.esc_button(gw, gh);

        // 1. Touch outside Esc
        let (consumed, triggered) = controls.handle_esc_touch(
            &make_touch(10, TouchPhase::Started, 100.0, 100.0),
            1.0,
            gw,
            gh,
        );
        assert!(!consumed);
        assert!(!triggered);

        // 2. Touch on Esc button
        let (consumed, triggered) = controls.handle_esc_touch(
            &make_touch(11, TouchPhase::Started, esc_center[0] as f64, esc_center[1] as f64),
            1.0,
            gw,
            gh,
        );
        assert!(consumed);
        assert!(!triggered);
        assert_eq!(controls.esc_finger, Some(11));

        // 3. Move finger
        let (consumed, triggered) = controls.handle_esc_touch(
            &make_touch(11, TouchPhase::Moved, esc_center[0] as f64 + 1.0, esc_center[1] as f64 + 1.0),
            1.0,
            gw,
            gh,
        );
        assert!(consumed);
        assert!(!triggered);

        // 4. Release finger on Esc -> triggers
        let (consumed, triggered) = controls.handle_esc_touch(
            &make_touch(11, TouchPhase::Ended, esc_center[0] as f64, esc_center[1] as f64),
            1.0,
            gw,
            gh,
        );
        assert!(consumed);
        assert!(triggered);
        assert_eq!(controls.esc_finger, None);

        // 5. Test draw_esc
        let atlas = HudAtlas::boot();
        let mut batch = HudBatch::new([1280, 720]);
        controls.draw_esc(&mut batch, &atlas);
        assert!(!batch.vertices.is_empty());
    }
}
