//! Touch input for Android (horizon-android).
//!
//! Provides:
//! - `TouchPointer`: converts touch on menu screens (Title, Loading, Pause, Options, Inventory)
//!   into standard mouse window events (cursor movement, left button clicks, and vertical scrolling).
//! - `TouchControls`: on-screen virtual controls in the game world (`Screen::Playing`),
//!   including a walking joystick on the left, camera look drag on the right, action buttons
//!   (Jump, Sneak, Attack, Use, Inventory, Pause, Chat), and hotbar slot selection.
//! - Direct HUD rendering via `HudBatch` and `HudAtlas`.

use horizon_client_render::hud::{HudAtlas, HudBatch, rgba};
use std::time::Instant;
use winit::{
    dpi::PhysicalPosition,
    event::{DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent},
};

/// Threshold in pixels before a touch move is considered a drag/scroll rather than a tap.
pub const SLOP_PX: f32 = 14.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuGesture {
    Undecided,
    Scroll,
    Drag,
}

/// Translates screen touches on menu screens into mouse events.
pub struct TouchPointer {
    finger: Option<u64>,
    device: Option<DeviceId>,
    start: [f32; 2],
    last: [f32; 2],
    gesture: MenuGesture,
    scroll_scale: f32,
}

impl Default for TouchPointer {
    fn default() -> Self {
        Self {
            finger: None,
            device: None,
            start: [0.0, 0.0],
            last: [0.0, 0.0],
            gesture: MenuGesture::Undecided,
            scroll_scale: 0.5,
        }
    }
}

impl TouchPointer {
    pub fn owns(&self, finger: u64) -> bool {
        self.finger == Some(finger)
    }

    pub fn down(&self) -> bool {
        self.finger.is_some()
    }

    pub fn set_gui_scale(&mut self, gui_scale: f32, sensitivity: f32) {
        let s = (gui_scale.max(0.1) * sensitivity.max(0.01)) * 0.5;
        self.scroll_scale = 1.0 / s.max(0.1);
    }

    pub fn reset(&mut self) {
        self.finger = None;
        self.gesture = MenuGesture::Undecided;
    }

    pub fn event(&mut self, touch: &Touch, slop: f32) -> Vec<WindowEvent> {
        let p = [touch.location.x as f32, touch.location.y as f32];
        let device_id = touch.device_id;
        self.device = Some(device_id);

        let mut out = Vec::new();
        match touch.phase {
            TouchPhase::Started if self.finger.is_none() => {
                self.finger = Some(touch.id);
                self.start = p;
                self.last = p;
                self.gesture = MenuGesture::Undecided;
                out.push(WindowEvent::CursorMoved {
                    device_id,
                    position: PhysicalPosition::new(p[0] as f64, p[1] as f64),
                });
            }
            TouchPhase::Moved if self.owns(touch.id) => {
                let dx = p[0] - self.start[0];
                let dy = p[1] - self.start[1];
                let dist = (dx * dx + dy * dy).sqrt();

                if self.gesture == MenuGesture::Undecided && dist > slop {
                    if dy.abs() > dx.abs() * 1.25 {
                        self.gesture = MenuGesture::Scroll;
                        self.last = self.start;
                    } else {
                        self.gesture = MenuGesture::Drag;
                        out.push(WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Pressed,
                            button: MouseButton::Left,
                        });
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
    Attack,
    Use,
    Pause,
    Chat,
    Inventory,
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x && p[0] <= self.x + self.w && p[1] >= self.y && p[1] <= self.y + self.h
    }
}

pub enum TouchAction {
    Stick([f32; 2]),
    Look(f64, f64),
    ButtonDown(TouchButton),
    ButtonUp(TouchButton),
    SelectSlot(usize),
}

#[derive(Clone, Debug)]
enum FingerRole {
    Stick { origin: [f32; 2], current: [f32; 2] },
    Look { last: [f32; 2], start: [f32; 2], time: Instant },
    Button(TouchButton),
}

pub struct TouchControls {
    fingers: Vec<(u64, FingerRole)>,
    held_buttons: Vec<TouchButton>,
}

impl Default for TouchControls {
    fn default() -> Self {
        Self {
            fingers: Vec::new(),
            held_buttons: Vec::new(),
        }
    }
}

impl TouchControls {
    pub fn reset(&mut self) -> Vec<TouchAction> {
        let mut out = Vec::new();
        for btn in self.held_buttons.drain(..) {
            out.push(TouchAction::ButtonUp(btn));
        }
        if self.fingers.iter().any(|(_, r)| matches!(r, FingerRole::Stick { .. })) {
            out.push(TouchAction::Stick([0.0, 0.0]));
        }
        self.fingers.clear();
        out
    }

    pub fn buttons(&self, gw: f32, gh: f32) -> Vec<(TouchButton, Rect, &'static str)> {
        vec![
            (
                TouchButton::Pause,
                Rect { x: gw - 34.0, y: 6.0, w: 28.0, h: 22.0 },
                "||",
            ),
            (
                TouchButton::Chat,
                Rect { x: gw - 66.0, y: 6.0, w: 28.0, h: 22.0 },
                "Chat",
            ),
            (
                TouchButton::Jump,
                Rect { x: gw - 52.0, y: gh - 60.0, w: 42.0, h: 42.0 },
                "Jump",
            ),
            (
                TouchButton::Sneak,
                Rect { x: gw - 100.0, y: gh - 60.0, w: 42.0, h: 42.0 },
                "Sneak",
            ),
            (
                TouchButton::Attack,
                Rect { x: gw - 52.0, y: gh - 108.0, w: 42.0, h: 42.0 },
                "Hit",
            ),
            (
                TouchButton::Use,
                Rect { x: gw - 100.0, y: gh - 108.0, w: 42.0, h: 42.0 },
                "Use",
            ),
            (
                TouchButton::Inventory,
                Rect { x: gw / 2.0 + 96.0, y: gh - 22.0, w: 22.0, h: 22.0 },
                "···",
            ),
        ]
    }

    pub fn handle_touch(&mut self, touch: &Touch, gui_scale: f32, gw: f32, gh: f32) -> Vec<TouchAction> {
        let p = [
            (touch.location.x as f32) / gui_scale.max(1.0),
            (touch.location.y as f32) / gui_scale.max(1.0),
        ];

        let mut out = Vec::new();
        match touch.phase {
            TouchPhase::Started => {
                // 1. Check virtual buttons
                for (btn, rect, _) in self.buttons(gw, gh) {
                    if rect.contains(p) {
                        self.fingers.push((touch.id, FingerRole::Button(btn)));
                        if !self.held_buttons.contains(&btn) {
                            self.held_buttons.push(btn);
                            out.push(TouchAction::ButtonDown(btn));
                        }
                        return out;
                    }
                }

                // 2. Check hotbar slots
                let hotbar_x = gw / 2.0 - 90.0;
                let hotbar_y = gh - 22.0;
                if p[1] >= hotbar_y && p[1] <= hotbar_y + 22.0 && p[0] >= hotbar_x && p[0] <= hotbar_x + 180.0 {
                    let slot = (((p[0] - hotbar_x) / 20.0).floor() as usize).min(8);
                    out.push(TouchAction::SelectSlot(slot));
                    return out;
                }

                // 3. Movement stick (left side of screen)
                if p[0] < gw * 0.45 {
                    if !self.fingers.iter().any(|(_, r)| matches!(r, FingerRole::Stick { .. })) {
                        self.fingers.push((touch.id, FingerRole::Stick { origin: p, current: p }));
                    }
                    return out;
                }

                // 4. Camera look (right side of screen)
                if !self.fingers.iter().any(|(_, r)| matches!(r, FingerRole::Look { .. })) {
                    self.fingers.push((
                        touch.id,
                        FingerRole::Look {
                            last: p,
                            start: p,
                            time: Instant::now(),
                        },
                    ));
                }
            }
            TouchPhase::Moved => {
                if let Some((_, role)) = self.fingers.iter_mut().find(|(id, _)| *id == touch.id) {
                    match role {
                        FingerRole::Stick { origin, current } => {
                            *current = p;
                            let r = 40.0;
                            let dx = current[0] - origin[0];
                            let dy = current[1] - origin[1];
                            let sx = (dx / r).clamp(-1.0, 1.0);
                            let sy = (-dy / r).clamp(-1.0, 1.0);
                            out.push(TouchAction::Stick([sx, sy]));
                        }
                        FingerRole::Look { last, .. } => {
                            let dx = ((p[0] - last[0]) as f64) * 1.5;
                            let dy = ((p[1] - last[1]) as f64) * 1.5;
                            *last = p;
                            out.push(TouchAction::Look(dx, dy));
                        }
                        FingerRole::Button(_) => {}
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if let Some(pos) = self.fingers.iter().position(|(id, _)| *id == touch.id) {
                    let (_, role) = self.fingers.remove(pos);
                    match role {
                        FingerRole::Stick { .. } => {
                            out.push(TouchAction::Stick([0.0, 0.0]));
                        }
                        FingerRole::Button(btn) => {
                            self.held_buttons.retain(|b| *b != btn);
                            out.push(TouchAction::ButtonUp(btn));
                        }
                        FingerRole::Look { start, time, .. } => {
                            let dx = p[0] - start[0];
                            let dy = p[1] - start[1];
                            let dist = (dx * dx + dy * dy).sqrt();
                            if touch.phase == TouchPhase::Ended && dist < 12.0 && time.elapsed().as_millis() < 240 {
                                // Tap in world: perform a quick use / interact
                                out.push(TouchAction::ButtonDown(TouchButton::Use));
                                out.push(TouchAction::ButtonUp(TouchButton::Use));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    pub fn active_stick(&self) -> Option<([f32; 2], [f32; 2])> {
        self.fingers.iter().find_map(|(_, r)| match r {
            FingerRole::Stick { origin, current } => Some((*origin, *current)),
            _ => None,
        })
    }

    pub fn draw(&self, batch: &mut HudBatch, atlas: &HudAtlas) {
        let (gw, gh) = (batch.width, batch.height);

        // 1. Draw action buttons
        for (btn, rect, label) in self.buttons(gw, gh) {
            let is_held = self.held_buttons.contains(&btn);
            let bg = if is_held {
                rgba(255, 255, 255, 110)
            } else {
                rgba(0, 0, 0, 90)
            };
            batch.fill(atlas, rect.x, rect.y, rect.w, rect.h, bg);

            let border = if is_held {
                rgba(255, 255, 255, 220)
            } else {
                rgba(255, 255, 255, 55)
            };
            batch.fill(atlas, rect.x, rect.y, rect.w, 1.0, border);
            batch.fill(atlas, rect.x, rect.y + rect.h - 1.0, rect.w, 1.0, border);
            batch.fill(atlas, rect.x, rect.y, 1.0, rect.h, border);
            batch.fill(atlas, rect.x + rect.w - 1.0, rect.y, 1.0, rect.h, border);

            let tw = HudBatch::text_width(atlas, label);
            let tx = rect.x + (rect.w - tw) * 0.5;
            let ty = rect.y + (rect.h - 8.0) * 0.5;
            batch.text(atlas, label, tx, ty, rgba(255, 255, 255, 230), true);
        }

        // 2. Draw stick
        if let Some((origin, current)) = self.active_stick() {
            let r = 40.0;
            let dx = current[0] - origin[0];
            let dy = current[1] - origin[1];
            let dist = (dx * dx + dy * dy).sqrt();
            let knob_pos = if dist > r {
                [origin[0] + dx * (r / dist), origin[1] + dy * (r / dist)]
            } else {
                current
            };
            batch.fill(atlas, origin[0] - 25.0, origin[1] - 25.0, 50.0, 50.0, rgba(0, 0, 0, 60));
            batch.fill(atlas, knob_pos[0] - 14.0, knob_pos[1] - 14.0, 28.0, 28.0, rgba(255, 255, 255, 130));
        } else {
            let hx = 45.0;
            let hy = gh - 65.0;
            batch.fill(atlas, hx - 24.0, hy - 24.0, 48.0, 48.0, rgba(0, 0, 0, 35));
            batch.fill(atlas, hx - 12.0, hy - 12.0, 24.0, 24.0, rgba(255, 255, 255, 35));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::DeviceId;

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
        // Drag vertically beyond SLOP_PX (14.0)
        let events = pointer.event(&make_touch(1, TouchPhase::Moved, 100.0, 130.0), SLOP_PX);
        assert!(!events.is_empty());
        assert!(matches!(events[0], WindowEvent::MouseWheel { .. }));
    }

    #[test]
    fn touch_controls_stick_moves_and_centres() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        // Touch on left side (x = 50.0)
        let actions = controls.handle_touch(&make_touch(1, TouchPhase::Started, 50.0, 200.0), 1.0, gw, gh);
        assert!(actions.is_empty());
        assert!(controls.active_stick().is_some());

        // Move forward (upwards, negative y)
        let actions = controls.handle_touch(&make_touch(1, TouchPhase::Moved, 50.0, 160.0), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        match actions[0] {
            TouchAction::Stick([sx, sy]) => {
                assert_eq!(sx, 0.0);
                assert!(sy > 0.5); // Moving forward
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
        let (_, jump_rect, _) = buttons.iter().find(|(b, ..)| *b == TouchButton::Jump).unwrap();

        // Tap inside Jump rect
        let tap_x = jump_rect.x + 5.0;
        let tap_y = jump_rect.y + 5.0;
        let actions = controls.handle_touch(&make_touch(2, TouchPhase::Started, tap_x as f64, tap_y as f64), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::ButtonDown(TouchButton::Jump)));

        let actions = controls.handle_touch(&make_touch(2, TouchPhase::Ended, tap_x as f64, tap_y as f64), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::ButtonUp(TouchButton::Jump)));
    }

    #[test]
    fn touch_controls_hotbar_slot_selection() {
        let mut controls = TouchControls::default();
        let (gw, gh) = (640.0, 360.0);
        let hotbar_x = gw / 2.0 - 90.0;
        let hotbar_y = gh - 22.0;

        // Tap slot 3
        let tap_x = hotbar_x + 3.0 * 20.0 + 5.0;
        let tap_y = hotbar_y + 5.0;
        let actions = controls.handle_touch(&make_touch(3, TouchPhase::Started, tap_x as f64, tap_y as f64), 1.0, gw, gh);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TouchAction::SelectSlot(3)));
    }
}

