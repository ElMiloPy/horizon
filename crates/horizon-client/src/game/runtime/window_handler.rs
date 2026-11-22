//! winit's `ApplicationHandler` for the game window: window and device events, shutting down,
//! and the idle step (`about_to_wait`).
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ClientQuit {
    Running,
    Queued,
    ShuttingDown,
}

impl Game<'_> {
    pub(super) fn touch_playing(&self) -> bool {
        self.screen == Screen::Playing
            && self.title.is_none()
            && self.player.is_some()
            && !self.screen_over_playing()
            && !self.touch_settings_open
            && !self.accounts_ui.is_open()
    }

    pub(super) fn touch_screen_shows_esc(&self) -> bool {
        self.player.is_some()
            && self.title.is_none()
            && !self.touch_settings_open
            && (matches!(
                self.screen,
                Screen::Inventory
                    | Screen::Creative
                    | Screen::Chat
                    | Screen::Advancements
                    | Screen::Stats
            ) || self.screen_over_playing())
    }

    pub(super) fn touch_hits_entity(&self) -> bool {
        if let Some((_, distance, _)) = self.mob_hit()
            && self.before_block(distance)
            && self.held_range_allows(distance)
        {
            return true;
        }
        if let Some(hit) = self.remote_object_hit(true)
            && self.before_block(hit.distance)
            && self.held_range_allows(hit.distance)
        {
            return true;
        }
        false
    }

    /// Normalized `[0..1, 0..1]` aim point for tap-to-interact touch schemes; `None` in crosshair modes.
    pub(super) fn touch_aim_point(&self) -> Option<[f32; 2]> {
        if !self.touch_active || !self.touch_playing() || self.touch_controls.crosshair() {
            return None;
        }
        self.touch_controls.aim.or_else(|| {
            self.touch_tapped
                .filter(|(until, _)| Instant::now() < *until)
                .map(|(_, p)| p)
        })
    }

    pub(super) fn touch_aim_dir(&self) -> Option<[f64; 3]> {
        let p = self.touch_aim_point()?;
        let fov = self.locator_camera?.3;
        let (w, h) = self
            .renderer
            .as_ref()
            .map_or((1280.0, 720.0), |r| (r.config.width.max(1) as f64, r.config.height.max(1) as f64));
        let (sx, sy) = (2.0 * f64::from(p[0]) - 1.0, 1.0 - 2.0 * f64::from(p[1]));
        let t = (f64::from(fov).to_radians() / 2.0).tan();
        let forward = glam::DVec3::from_array(view_vector(self.yaw, self.pitch));
        let yaw = f64::from(self.yaw).to_radians();
        let right = glam::DVec3::new(-yaw.cos(), 0.0, -yaw.sin());
        let up = right.cross(forward);
        let dir = forward + right * (sx * t * w / h) + up * (sy * t);
        Some(dir.normalize().to_array())
    }

    #[cfg(target_os = "android")]
    pub(super) fn android_input_frame(&mut self, event_loop: &dyn EventLoopCtl) {
        if crate::android::pointer_capture_lost() && self.captured {
            self.capture(false);
        }
        if crate::android::keyboard_used() {
            self.touch_active = false;
            if self.screen == Screen::Playing && !self.captured {
                self.capture(true);
            }
        }
        let events = crate::android::take_mouse_events();
        if events.is_empty() {
            return;
        }
        self.touch_active = false;
        let Some(id) = self.window.as_ref().map(|w| w.id()) else {
            return;
        };
        let device_id = DeviceId::dummy();
        for event in events {
            let event = match event {
                crate::android::MouseEvent::Moved(x, y) => WindowEvent::CursorMoved {
                    device_id,
                    position: winit::dpi::PhysicalPosition::new(x, y),
                },
                crate::android::MouseEvent::Button(button, down) => WindowEvent::MouseInput {
                    device_id,
                    state: if down {
                        ElementState::Pressed
                    } else {
                        ElementState::Released
                    },
                    button,
                },
                crate::android::MouseEvent::Wheel(x, y) => WindowEvent::MouseWheel {
                    device_id,
                    delta: MouseScrollDelta::LineDelta(x, y),
                    phase: winit::event::TouchPhase::Moved,
                },
                crate::android::MouseEvent::Motion(x, y) => {
                    if self.screen == Screen::Playing && !self.captured {
                        self.capture(true);
                    }
                    if self.pending_save.is_none()
                        && (self.captured || self.screen == Screen::Playing)
                        && (self.focused || cfg!(target_os = "android"))
                        && self.keyboard_mouse
                        && (!self.options.offscreen || self.split_window)
                        && self.screen == Screen::Playing
                    {
                        self.note_input(
                            input_trace::InputOrigin::User,
                            input_trace::TraceKind::Motion { x, y },
                        );
                        self.mouse.on_move(0.0, 0.0, x, y, true, true);
                        self.last_input = Instant::now();
                    }
                    continue;
                }
            };
            self.handle_window_event(event_loop, id, event, input_trace::InputOrigin::User);
        }
    }

    /// Polls Starling's `TouchControls` once per frame while playing, syncing contextual
    /// player state, feeding `pad.injected`, and handling short world taps (tap-to-attack mobs
    /// vs use/place) and shortcut buttons (Chat, Settings, F3, F5, Swap Offhand).
    pub(super) fn touch_frame(&mut self) {
        let projectile_crosshair = self
            .stack_in(self.selected)
            .is_some_and(|s| horizon_client_render::touch::projectile_crosshair(&s.item));
        let main_shield = self
            .stack_in(self.selected)
            .is_some_and(|s| &*s.item == "minecraft:shield");
        let off_shield = self
            .offhand
            .as_ref()
            .is_some_and(|s| &*s.item == "minecraft:shield");
        let consumable = self
            .stack_in(self.selected)
            .is_some_and(|s| self.consumable(s).is_some());
        self.touch_controls.vertical_movement = self.player.as_ref().is_some_and(|p| p.flying);
        self.touch_controls.swimming = self
            .player
            .as_ref()
            .is_some_and(|p| p.fluids.was_touching_water);
        self.touch_controls.mounted = self.riding.is_some();
        self.touch_controls.sneaking = self
            .player
            .as_ref()
            .is_some_and(|p| p.shift_key_down);
        self.touch_controls.break_progress = self.mining.as_ref().map_or(0.0, |m| m.progress);
        self.touch_controls.projectile_crosshair = projectile_crosshair;
        self.touch_controls.shield_available = main_shield || off_shield;
        self.touch_controls.hold_uses_item = projectile_crosshair || main_shield || consumable;

        let mut pad_state = self.touch_controls.poll_state();
        let now = Instant::now();
        if self.touch_use_until.is_some_and(|t| now < t) {
            pad_state.triggers[0] = 1.0;
        } else {
            self.touch_use_until = None;
        }
        if self.touch_attack_until.is_some_and(|t| now < t) {
            pad_state.triggers[1] = 1.0;
        } else {
            self.touch_attack_until = None;
        }
        self.pad.injected = pad_state;

        if let Some(held_use) = self.touch_controls.take_tap() {
            self.touch_tapped = self
                .touch_controls
                .aim
                .map(|p| (now + Duration::from_millis(160), p));
            let sneaking = self.touch_controls.sneaking;
            if !held_use && !sneaking && self.touch_hits_entity() {
                self.pad.injected.triggers[1] = 1.0;
                self.touch_attack_until = Some(now + Duration::from_millis(80));
                self.clicks.miss_time = 0;
                if self.remote.is_some() {
                    self.remote_start_attack();
                } else {
                    self.attack();
                }
            } else {
                self.pad.injected.triggers[0] = 1.0;
                self.touch_use_until = Some(now + Duration::from_millis(80));
            }
        }

        if std::mem::take(&mut self.touch_controls.clicked) {
            self.audio.play_button_click();
        }
        if std::mem::take(&mut self.touch_controls.chat_requested) {
            let _ = self.key_pressed(KeyCode::KeyT, KeyCode::KeyT);
        }
        if std::mem::take(&mut self.touch_controls.settings_requested) {
            self.touch_settings_open = true;
            self.touch_controls.clear();
            self.pad.injected = Default::default();
            self.audio.play_button_click();
        }
        if std::mem::take(&mut self.touch_controls.debug_requested) {
            let _ = self.key_pressed(KeyCode::F3, KeyCode::F3);
        }
        if std::mem::take(&mut self.touch_controls.perspective_requested) {
            let _ = self.key_pressed(KeyCode::F5, KeyCode::F5);
        }
        for (token, down) in self.touch_controls.key_events() {
            if down && token == "action:key.swapOffhand" {
                let _ = self.key_pressed(KeyCode::KeyF, KeyCode::KeyF);
            }
        }
    }

    /// Record the request without saving or entering other mod hooks from this hook.
    pub(super) fn request_client_quit(&self) -> bool {
        if !self.options.is_client_start()
            || self.options.headless
            || self.error.is_some()
            || self.client_quit.get() != ClientQuit::Running
        {
            return false;
        }
        self.client_quit.set(ClientQuit::Queued);
        true
    }

    /// A run that a person's stray key, click or mouse move would spoil: a screenshot sequence, a
    /// `HORIZON_TICK_SCRIPT` scenario or a `record-replay`. While one runs, the keyboard, mouse
    /// and pad at this computer don't reach the game (`locks_out`, `poll_pads`), the window's
    /// focus stays as the run found it (`held_focus`) and its pointer grab stays inside the game
    /// (`internal_grab`).
    pub(super) fn scripted_run(&self) -> bool {
        self.export.is_some()
            || self.screenshot_sequence.is_some()
            || self.tick_script.playing()
            || self.repro.replaying()
    }
}

/// A key, a modifier, a click, the wheel, the pointer or the input method. Resize and close are
/// not, so a person can still close the window. A modifier change counts: `reconcile` would
/// release a Shift or Ctrl the run holds.
fn is_input(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::KeyboardInput { .. }
            | WindowEvent::ModifiersChanged(_)
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::Ime(_)
    )
}

fn input_release(event: &WindowEvent) -> bool {
    match event {
        WindowEvent::KeyboardInput { event, .. } => event.state == ElementState::Released,
        WindowEvent::MouseInput { state, .. } => *state == ElementState::Released,
        _ => false,
    }
}

/// A person's input during a scripted run is dropped. What the run itself sends (the control
/// socket, a replay, the game's own) still plays.
fn locks_out(origin: input_trace::InputOrigin, scripted: bool, event: &WindowEvent) -> bool {
    scripted && origin == input_trace::InputOrigin::User && is_input(event)
}

/// Linux can drop a confined pointer grab while a touchpad is moving and a
/// movement key is held. Reassert it on the next key press as well as on the
/// pointer events that already did so.
fn should_reassert_key(state: ElementState, repeat: bool) -> bool {
    state == ElementState::Pressed && !repeat
}

pub(super) fn should_reassert_pointer_capture(event: &WindowEvent) -> bool {
    match event {
        WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => true,
        WindowEvent::KeyboardInput { event, .. } => should_reassert_key(event.state, event.repeat),
        _ => false,
    }
}

/// A person clicking into the window or away from it during a scripted run. A replay's look and
/// clicks act by the focus and the pointer grab, which that would change mid-run: the run keeps
/// the focus it started with, and the last change applies once it ends (`settle_after_run`).
fn held_focus(
    origin: input_trace::InputOrigin,
    scripted: bool,
    event: &WindowEvent,
) -> Option<bool> {
    match event {
        WindowEvent::Focused(focused) if scripted && origin == input_trace::InputOrigin::User => {
            Some(*focused)
        }
        _ => None,
    }
}

/// What a person's event does at an agent's window no one has clicked yet.
#[derive(Debug, PartialEq)]
enum Unclaimed {
    /// A mouse button pressed inside it: the window is theirs from this press on.
    Claims,
    /// Their keys, wheel, input method and button releases, and the focus the system gives the
    /// window or takes from it: the game the agent drives hears none of them.
    Dropped,
    /// Everything else, and whatever the agent or the game sends.
    Passes,
}

/// Only a press claims the window. The focus doesn't: Windows and X11 can hand it to a window
/// nobody clicked, and the person's keys and mouse would then steer the agent's game (#15449).
/// The pointer's position still passes, so the press that claims a menu lands where it points.
fn at_unclaimed(
    origin: input_trace::InputOrigin,
    unclaimed: bool,
    event: &WindowEvent,
) -> Unclaimed {
    if !unclaimed || origin != input_trace::InputOrigin::User {
        return Unclaimed::Passes;
    }
    match event {
        WindowEvent::MouseInput {
            state: ElementState::Pressed,
            ..
        } => Unclaimed::Claims,
        WindowEvent::CursorMoved { .. } => Unclaimed::Passes,
        WindowEvent::Focused(_) => Unclaimed::Dropped,
        _ if is_input(event) => Unclaimed::Dropped,
        _ => Unclaimed::Passes,
    }
}

/// The Shift, Ctrl or Alt a person holds at an agent's window no one has clicked yet. The game
/// doesn't hear of them, but winit tells of modifiers only as they change: the last state is
/// kept for the claim, where a Ctrl held since before the click would otherwise read as up.
fn held_modifiers(
    origin: input_trace::InputOrigin,
    unclaimed: bool,
    event: &WindowEvent,
) -> Option<winit::event::Modifiers> {
    match event {
        WindowEvent::ModifiersChanged(modifiers)
            if unclaimed && origin == input_trace::InputOrigin::User =>
        {
            Some(*modifiers)
        }
        _ => None,
    }
}

impl Game<'_> {
    /// winit's `window_event`. The control socket and the pad menu call it too, with the
    /// input they make up.
    pub(super) fn handle_window_event(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        id: WindowId,
        event: WindowEvent,
        origin: input_trace::InputOrigin,
    ) {
        let input = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::Ime(_)
        );
        self.with_user_input(input, |game| {
            game.handle_window_event_as(event_loop, id, event, origin);
        });
    }

    pub(super) fn handle_window_event_as(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        id: WindowId,
        mut event: WindowEvent,
        origin: input_trace::InputOrigin,
    ) {
        // A headless game has no window: its events (the frame, the control socket's input)
        // carry `WindowId::dummy()`.
        let ours = match &self.window {
            Some(w) => w.id() == id,
            None => self.options.headless,
        };
        if !ours {
            return;
        }
        if let Some(modifiers) = held_modifiers(origin, self.unclaimed_window, &event) {
            self.unclaimed_modifiers = Some(modifiers);
        }
        let locked = locks_out(origin, self.scripted_run(), &event);
        if locked {
            if !input_release(&event) {
                return;
            }
        } else {
            if let Some(focused) = held_focus(origin, self.scripted_run(), &event) {
                self.focus_after_run = Some(focused);
                return;
            }
            match at_unclaimed(origin, self.unclaimed_window, &event) {
                Unclaimed::Claims => self.claim_window(event_loop),
                Unclaimed::Dropped => return,
                Unclaimed::Passes => {}
            }
        }
        follow_keymap(&mut event);
        if let WindowEvent::Touch(ref touch) = event {
            self.touch_active = true;
            #[cfg(target_os = "android")]
            if self.captured {
                self.capture(false);
            }
            let (sw, sh, gw, gh, gui_scale) = self
                .renderer
                .as_ref()
                .map_or((1280u32, 720u32, 640.0, 360.0, 2.0), |r| {
                    let b = horizon_client::hud::HudBatch::new(
                        [r.config.width, r.config.height],
                        self.settings.gui_scale_inputs(),
                    );
                    (r.config.width.max(1), r.config.height.max(1), b.width, b.height, b.scale)
                });
            self.touch_controls.viewport(sw, sh);
            if !self.touch_playing() {
                if self.touch_controls.owns_pointer(touch.id) || self.pad.injected != Default::default() {
                    self.touch_controls.clear();
                    self.pad.injected = Default::default();
                }
                if self.touch_screen_shows_esc() {
                    let (consumed, triggered) = self.touch_esc.handle(
                        touch,
                        sw as f32,
                        sh as f32,
                        self.touch_controls.esc_rect(),
                    );
                    if triggered {
                        let _ = self.key_pressed(KeyCode::Escape, KeyCode::Escape);
                        #[cfg(target_os = "android")]
                        if self.captured {
                            self.capture(false);
                        }
                    }
                    if consumed {
                        return;
                    }
                }
                let events = self.touch_pointer.event(touch, crate::touch::SLOP_PX);
                for ev in events {
                    self.handle_window_event(event_loop, id, ev, origin);
                }
                return;
            } else {
                let norm = [
                    touch.location.x as f32 / sw as f32,
                    touch.location.y as f32 / sh as f32,
                ];
                if touch.phase == winit::event::TouchPhase::Started {
                    let gui_p = [
                        touch.location.x as f32 / gui_scale.max(1.0),
                        touch.location.y as f32 / gui_scale.max(1.0),
                    ];
                    if let Some(slot) = horizon_client_render::touch::hotbar_slot(gw, gh, gui_p) {
                        self.selected = slot;
                        self.touch_controls.ignore(touch.id);
                        return;
                    }
                }
                let delta = self.touch_controls.update(
                    touch.id,
                    norm,
                    touch.phase == winit::event::TouchPhase::Started,
                    matches!(
                        touch.phase,
                        winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled
                    ),
                );
                if delta != [0.0, 0.0] {
                    let turn = horizon_client_render::touch::camera_delta(
                        delta,
                        sw,
                        sh,
                        &self.touch_controls.settings,
                    );
                    self.turn_player(f64::from(turn[0]) / 0.15, f64::from(turn[1]) / 0.15);
                    self.last_input = Instant::now();
                }
                return;
            }
        }
        // MacosUtil.setCtrlClickEmulatesRightClick: Control + left click is a right click.
        if let WindowEvent::MouseInput { state, button, .. } = &mut event
            && *button == MouseButton::Left
            && self.right_click_emulation.is_right_click(
                *state == ElementState::Pressed,
                self.modifiers.control_key(),
                cfg!(target_os = "macos") && self.settings.ctrl_click_emulates_right_click,
            )
        {
            *button = MouseButton::Right;
        }
        // Who caused it, before the event is handled: the trace wants the
        // window's own keys, never the ones the socket made up.
        if !locked {
            self.trace_window_event(origin, &event);
        }
        if let WindowEvent::KeyboardInput { event, .. } = &event
            && let PhysicalKey::Code(key) = event.physical_key
        {
            if event.state == ElementState::Pressed {
                self.physical_keys.insert(key);
                horizon_client::keybinds::learn_layout(event);
            } else if self.physical_keys.remove(&key)
                && locked
                && let Some(export) = &mut self.export
            {
                export
                    .released_mappings
                    .extend(self.settings.keys.translate_all(key));
            }
        }
        if locked {
            // Only releases reach here. Keep the physical state current without dispatching clicks.
            if let WindowEvent::MouseInput { button, .. } = &event
                && self.physical_buttons.remove(button)
                && let Some(export) = &mut self.export
            {
                export
                    .released_mappings
                    .extend(self.settings.keys.translate_mouse_all(*button));
            }
            return;
        }
        if matches!(
            &event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            }
        ) && !self.input_made_pointer_event()
        {
            self.pad.keyboard_took_over = true;
        }
        if let WindowEvent::MouseInput { state, button, .. } = &event {
            if *state == ElementState::Pressed {
                self.physical_buttons.insert(*button);
            } else {
                self.physical_buttons.remove(button);
            }
        }
        let mouse = if !self.pad_synthetic_input()
            && let WindowEvent::MouseInput { state, button, .. } = &event
        {
            Some((
                glfw_button(*button),
                *state == ElementState::Pressed,
                glfw_modifiers(self.modifiers),
            ))
        } else {
            None
        };
        if let Some((button, pressed, modifiers)) = mouse {
            let mut canceled = false;
            self.native_mouse_button_pre(button, pressed, modifiers, &mut canceled);
            if canceled {
                return;
            }
        }
        if mouse.is_some()
            && let WindowEvent::MouseInput { state, button, .. } = &event
        {
            self.mod_mappings_button(*button, *state == ElementState::Pressed);
        }
        let resized = matches!(event, WindowEvent::Resized(_));
        self.handle_window_event_inner(event_loop, event);
        if resized {
            self.notify_window_resize();
        }
        // A title screen the event asked for opens before the next event.
        self.settle_title_mode();
        if let Some((button, pressed, modifiers)) = mouse {
            self.native_mouse_button_post(button, pressed, modifiers);
        }
    }

    /// The loop ended: close the pads' output, save the world.
    pub(super) fn shut_down(&mut self) {
        self.client_quit.set(ClientQuit::ShuttingDown);
        if self.error.is_none()
            && let Some(renderer) = &mut self.renderer
        {
            renderer.drop_hdr_marker();
        }
        if self.error.is_none() && self.safe_start_proof.exit() {
            horizon_client::gfx::safe_start::clear(&Self::options_path());
        }
        self.cancel_export_on_quit();
        self.end_screenshot_sequence(Err("cancelled: the game is closing".into()));
        self.pad_close_output();
        if let Err(e) = self.save_for_exit() {
            log::warn!("Horizon: failed to save the world on exit: {e}");
        }
        self.finish_profile();
        // The client's last moment: its world is left and saved, its options and
        // resources are still held. A dedicated server and a splitscreen guest skip
        // it, as `client_setup` does.
        if self.options.is_client_start() {
            self.native_client_stopping();
        }
    }

    /// winit's `about_to_wait`: what happens between frames (stop requests, the closing
    /// save, world entry, the control socket, pads), then the next frame.
    pub(super) fn idle(&mut self, event_loop: &dyn EventLoopCtl) {
        // A splitscreen player's game has nothing to save: P1's shuts the process down.
        let client_quit = self.client_quit.get();
        if !self.options.split_guest
            && client_quit != ClientQuit::ShuttingDown
            && (client_quit == ClientQuit::Queued || horizon_threads::stop_requests() > 0)
        {
            // A mod hook or a signal requested the normal closing save.
            if let Err(error) = self.begin_closing_save(true) {
                self.error = Some(error);
                event_loop.exit();
                return;
            }
        }
        self.poll_export();
        // Also runs without a redraw, so a hidden window still handles a server crash.
        self.exit_after_server_crash();
        if self.pending_save.is_some() {
            if self
                .pending_save
                .as_ref()
                .is_some_and(|save| save.disconnecting())
            {
                self.poll_control_requests(event_loop);
            }
            match self.poll_closing_save() {
                Ok(true) => event_loop.exit(),
                Ok(false) => {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    // Yield between polls even when disk I/O is slow.
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        Instant::now() + Duration::from_millis(4),
                    ));
                }
                Err(error) => {
                    self.error = Some(error);
                    self.exit_on_error(event_loop);
                }
            }
            return;
        }
        if let Err(error) = horizon_client::spans::time("world_entry", || self.resume_world_entry())
        {
            self.error = Some(error);
            self.exit_on_error(event_loop);
            return;
        }
        // The world a backup made before its upgrade opens (EditWorldScreen's callback).
        if let Some(folder) = self.title.as_mut().and_then(|t| t.take_open_request())
            && let Err(error) = self.open_world(&folder, None, None, false, None)
        {
            self.error = Some(error);
            event_loop.exit();
            return;
        }
        // A Realm's address came back (GetServerDetailsTask hands over to ConnectScreen).
        if let Some(action) = self.title.as_mut().and_then(|t| t.take_connect_request())
            && let Err(error) = self.title_action(event_loop, action)
        {
            self.error = Some(error);
            event_loop.exit();
            return;
        }
        horizon_client::spans::time("server_packs", || self.tick_server_packs());
        horizon_client::spans::time("control", || self.poll_control(event_loop));
        if self.pending_save.is_some() {
            return;
        }
        if self.options.player_name.is_none()
            && let Some(name) = self.accounts_ui.active_name()
        {
            self.options.player_name = Some(name.to_owned());
        }
        if let Some(name) = self.accounts_ui.poll() {
            self.options.player_name = Some(name);
        }
        #[cfg(target_os = "android")]
        self.android_input_frame(event_loop);
        if self.touch_active && self.touch_playing() {
            self.touch_frame();
        }
        if self.touch_tapped.is_some_and(|(t, _)| Instant::now() >= t) {
            self.touch_tapped = None;
        }
        horizon_client::spans::time("pads", || self.poll_pads(event_loop));
        self.settle_after_run(event_loop);
        self.pause_if_inactive();
        if let Some(limit) = self.options.time_limit()
            && self.started.elapsed() > limit
        {
            self.error = Some(format!("run timed out after {} s", limit.as_secs()).into());
            event_loop.exit();
            return;
        }
        if self.export_held() {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(16),
            ));
            return;
        }
        if self.renderer.as_ref().is_some_and(|r| r.detached) {
            // No window to draw in (Android in the background): wake now and then for the
            // control socket instead of spinning.
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
            ));
            return;
        }
        if let Some(w) = &self.window {
            if self.options.offscreen {
                // Offscreen frames don't wait for the window to be drawn: an
                // occluded window (a locked macOS session) gets no redraws.
                let id = w.id();
                self.handle_window_event(
                    event_loop,
                    id,
                    WindowEvent::RedrawRequested,
                    input_trace::InputOrigin::Game,
                );
            } else {
                w.request_redraw();
            }
        } else if self.options.headless {
            // The headless loop paces the frames itself (`run_headless`).
            self.handle_window_event(
                event_loop,
                WindowId::dummy(),
                WindowEvent::RedrawRequested,
                input_trace::InputOrigin::Game,
            );
            return;
        }
        if self.live_export_waiting() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(1),
            ));
            return;
        }
        // Consume the last frame's result: a hidden compositor may stop sending
        // redraws altogether, leaving a successful result stale indefinitely.
        event_loop.set_control_flow(event_loop::redraw_flow(
            &mut self.frame_drawn,
            self.options.offscreen,
            Instant::now(),
        ));
    }
}

impl ApplicationHandler for Game<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            // Back from the background (Android): the same window, a new native surface.
            let size = window.inner_size();
            if let Some(r) = &mut self.renderer
                && let Err(e) = r.resume(size.width, size.height)
            {
                log::warn!("Horizon: the window's surface did not come back: {e}");
            }
            self.audio.resume_output();
            window.request_redraw();
            return;
        }
        debug_keys::warm_debug_cpu();
        // The Window constructor: created fullscreen when options.txt or --fullscreen says so.
        let fullscreen = self.settings.fullscreen || self.options.fullscreen;
        let size = self.options.window_size.map(f64::from);
        self.fullscreen = horizon_client::window::FullscreenState::new(fullscreen);
        let attributes =
            horizon_client::window::with_icon(Window::default_attributes().with_title("Horizon"));
        // Wayland names the window by its app id, which the horizon.desktop entry matches.
        #[cfg(target_os = "linux")]
        let attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(
            attributes, "horizon", "horizon",
        );
        // A game an agent started opens without the focus, as splitscreen's windows do, and
        // the person keeps their keyboard and pointer until they click it (`unclaimed_window`).
        // X11 activates it anyway, and Windows can too: the focus claims nothing.
        let agent_started = self.options.control.is_some();
        self.unclaimed_window = agent_started && !self.options.offscreen;
        // `--offscreen` renders into a texture: its window would only show white.
        let window_step = startup::step("window.create");
        let window = match event_loop.create_window(
            attributes
                .with_inner_size(winit::dpi::LogicalSize::new(size[0], size[1]))
                .with_fullscreen(fullscreen.then_some(winit::window::Fullscreen::Borderless(None)))
                .with_visible(!self.options.offscreen)
                .with_active(!agent_started),
        ) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(e.into());
                event_loop.exit();
                return;
            }
        };
        horizon_client::window::settle_created_fullscreen(&window, fullscreen);
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = hwnd(&window) {
            horizon_threads::system_menu::suppress(hwnd);
        }
        drop(window_step);
        match self.new_renderer(window.clone()) {
            Ok(r) => {
                self.renderer = Some(r);
                self.world_renderer_ready = true;
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
                return;
            }
        }
        self.resize_state = Some(self.resize_state_of(window.inner_size()));
        self.window = Some(window);
        self.ime_allowed = false;
        self.ime_area = None;
        self.apply_settings();
        self.previous = Instant::now();
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let key = if self.window.as_ref().is_some_and(|w| w.id() == id)
            && !self.scripted_run()
            && !self.unclaimed_window
            && let WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } = &event
        {
            Some((event.clone(), glfw_modifiers(self.modifiers)))
        } else {
            None
        };
        self.handle_window_event(event_loop, id, event, input_trace::InputOrigin::User);
        if let Some((event, modifiers)) = key {
            #[cfg(not(target_os = "android"))]
            use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
            let name = match event.physical_key {
                PhysicalKey::Code(code) => horizon_client::keybinds::key_name(code),
                PhysicalKey::Unidentified(_) => None,
            };
            #[cfg(not(target_os = "android"))]
            let layout_key = horizon_client::keybinds::layout_key_name(
                event.physical_key,
                &event.key_without_modifiers(),
            );
            #[cfg(target_os = "android")]
            let layout_key =
                horizon_client::keybinds::layout_key_name(event.physical_key, &event.logical_key);
            let raw_code = horizon_client::keybinds::raw_key_code(event.physical_key);
            let action = match (event.state, event.repeat) {
                (winit::event::ElementState::Released, _) => mod_api::KeyAction::Release,
                (_, false) => mod_api::KeyAction::Press,
                (_, true) => mod_api::KeyAction::Repeat,
            };
            // Still the key's handling: a mod's key hook may reach the clipboard.
            self.with_user_input(true, |game| {
                game.key_input(name, layout_key, raw_code, action, modifiers);
            });
        }
    }
    fn device_event(&mut self, event_loop: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        // A composited view renders offscreen but its keyboard/mouse owner still
        // controls a visible native window. A headless offscreen run never does.
        if self.pending_save.is_none()
            && !self.scripted_run()
            && !self.unclaimed_window
            && self.captured
            && self.focused
            && self.keyboard_mouse
            && (!self.options.offscreen || self.split_window)
            && self.screen == Screen::Playing
            && let DeviceEvent::MouseMotion { delta: (x, y) } = event
        {
            if self.pad_radial_captures() {
                return;
            }
            // A replay's first-person view belongs to the recording, not to the mouse.
            if self.replay.playing() && !self.studio.flying() {
                return;
            }
            if x != 0.0 || y != 0.0 {
                self.pad_yield_input(event_loop);
            }
            // MouseHandler.onMove with the pointer grabbed; applied once per frame.
            self.note_input(
                input_trace::InputOrigin::User,
                input_trace::TraceKind::Motion { x, y },
            );
            self.mouse.on_move(0.0, 0.0, x, y, true, true);
            self.last_input = Instant::now();
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.idle(event_loop);
    }
    /// Android took the window away (the app went to the background, the screen went off): its
    /// surface goes now, and drawing pauses until resume.
    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(r) = &mut self.renderer {
            r.suspend();
        }
        self.audio.suspend_output();
        #[cfg(target_os = "android")]
        if self.captured {
            self.capture(false);
        }
    }
    /// The application is ending. On macOS, Quit (Cmd+Q) ends it through
    /// applicationWillTerminate without a CloseRequested; vanilla's GLFW
    /// window turns it into a window close, so Minecraft.stop saves the world.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shut_down();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_trace::InputOrigin;
    use std::cell::Cell;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::{Ime, Modifiers, TouchPhase};
    use winit::keyboard::ModifiersState;

    #[derive(Default)]
    struct TestLoop {
        exit_requested: Cell<bool>,
    }
    impl EventLoopCtl for TestLoop {
        fn exit(&self) {
            self.exit_requested.set(true);
        }
        fn set_control_flow(&self, _: ControlFlow) {}
    }

    #[test]
    #[ignore = "daily"]
    fn client_mod_quit_waits_for_the_title_hook_and_runs_normal_shutdown() {
        use mod_api::{
            ClientHost, ClientStoppingEvent, Flow, Hooks, ModEntry, ScreenKeyPressedPreEvent,
        };
        use std::sync::Mutex;

        struct QuitMod(Arc<Mutex<Vec<(&'static str, bool)>>>);
        #[mod_api::__subscriptions]
        impl Hooks for QuitMod {
            fn screen_key_pressed_pre(
                &self,
                host: &dyn ClientHost,
                event: &mut ScreenKeyPressedPreEvent<'_>,
            ) -> Flow {
                if event.screen.id != "title" || event.key != "key.keyboard.enter" {
                    return Flow::Continue;
                }
                let accepted = host.request_client_quit();
                let duplicate = host.request_client_quit();
                self.0
                    .lock()
                    .unwrap()
                    .extend([("confirm", accepted), ("duplicate", duplicate)]);
                Flow::Cancel
            }
            fn client_stopping(&self, host: &mut dyn ClientHost, _: &mut ClientStoppingEvent) {
                let accepted = host.request_client_quit();
                self.0.lock().unwrap().push(("stopping", accepted));
            }
        }

        let log = Arc::new(Mutex::new(Vec::new()));
        let mods = vec![ModEntry::new("quit", Box::new(QuitMod(log.clone())))].into();
        mod_platform::testing::with_mods(mods, || {
            crate::game::native_entity_refs::tests::with_client_game(|game| {
                game.title = Some(game.new_title(game.save.root.join("saves")));
                let batch = HudBatch::scratch([854, 480]);
                let screen = game.title_screen_view(&batch).unwrap();
                let event_loop = TestLoop::default();
                let mut canceled = false;
                game.native_screen_key_pressed_pre(
                    &screen,
                    "key.keyboard.enter",
                    false,
                    0,
                    &mut canceled,
                );
                assert!(canceled, "the mod handled the confirmation");
                assert_eq!(
                    *log.lock().unwrap(),
                    [("confirm", true), ("duplicate", false)]
                );
                assert!(
                    game.pending_save.is_none(),
                    "the hook must not start saving"
                );
                assert!(!event_loop.exit_requested.get());

                game.idle(&event_loop);
                assert!(
                    event_loop.exit_requested.get(),
                    "the idle loop must request exit"
                );
                assert!(game.error.is_none());
                game.shut_down();
                assert_eq!(
                    *log.lock().unwrap(),
                    [("confirm", true), ("duplicate", false), ("stopping", false)]
                );
            });
        });
    }

    #[test]
    #[ignore = "daily"]
    fn client_mod_quit_saves_and_unlocks_the_integrated_world_before_exit() {
        use horizon_client::save::{Level, SessionLock, WorldPreset};
        use horizon_client::title::WorldOptions;
        use mod_api::ClientHost;

        mod_platform::testing::with_mods(Vec::new().into(), || {
            crate::game::native_entity_refs::tests::with_client_game(|game| {
                let world = game.save.root.join("quit-test");
                let create = WorldOptions {
                    world_type: WorldPreset::Flat,
                    structures: false,
                    ..WorldOptions::default()
                };
                game.options.player_name = Some("QuitTester".into());
                game.settings.simulation_distance = 2;
                game.settings.render_distance = 2;
                game.open_integrated(
                    world.to_str().unwrap(),
                    Some("Before quit"),
                    Some(42),
                    false,
                    Some(&create),
                )
                .unwrap();
                let server = game.integrated.as_ref().unwrap();
                assert!(SessionLock::is_locked(&world));
                server
                    .call(|server_game| server_game.level.name = "Saved on quit".into())
                    .expect("the active integrated server must take the change");

                assert!((game as &dyn ClientHost).request_client_quit());
                let event_loop = TestLoop::default();
                let deadline = Instant::now() + Duration::from_secs(30);
                while !event_loop.exit_requested.get() {
                    game.idle(&event_loop);
                    assert!(game.error.is_none(), "{:?}", game.error);
                    assert!(
                        Instant::now() < deadline,
                        "the integrated save did not finish"
                    );
                    if !event_loop.exit_requested.get() {
                        std::thread::sleep(Duration::from_millis(4));
                    }
                }

                assert!(game.integrated.is_none());
                assert_eq!(Level::read(&world).unwrap().name, "Saved on quit");
                assert!(!SessionLock::is_locked(&world));
            });
        });
    }

    #[test]
    #[ignore = "daily"]
    fn client_mod_quit_refuses_inactive_host_contexts() {
        use crate::game::execution_role::ExecutionRole;
        use mod_api::__private::{ModPhase, PhaseOwner};
        use mod_api::ClientHost;

        mod_platform::testing::with_mods(Vec::new().into(), || {
            crate::game::native_entity_refs::tests::with_client_game(|game| {
                game.options.headless = true;
                assert!(!(game as &dyn ClientHost).request_client_quit());
                game.options.headless = false;
                game.options.split_guest = true;
                assert!(!(game as &dyn ClientHost).request_client_quit());
                game.options.split_guest = false;
                for role in [
                    ExecutionRole::IntegratedServer,
                    ExecutionRole::DedicatedServer,
                ] {
                    game.options.execution_role = role;
                    assert!(!(game as &dyn ClientHost).request_client_quit());
                }
                game.options.execution_role = ExecutionRole::Client;
                {
                    let _reload = PhaseOwner::scope(ModPhase::Reloading);
                    assert!(!(game as &dyn ClientHost).request_client_quit());
                }
                assert!((game as &dyn ClientHost).request_client_quit());
            });
        });
    }

    // A key's `KeyEvent` can't be made outside winit; these are the person's other inputs.
    fn inputs() -> Vec<WindowEvent> {
        let device_id = DeviceId::dummy();
        vec![
            WindowEvent::MouseInput {
                device_id,
                state: ElementState::Pressed,
                button: MouseButton::Left,
            },
            WindowEvent::MouseWheel {
                device_id,
                delta: MouseScrollDelta::LineDelta(0.0, 1.0),
                phase: TouchPhase::Moved,
            },
            WindowEvent::CursorMoved {
                device_id,
                position: PhysicalPosition::new(10.0, 20.0),
            },
            WindowEvent::ModifiersChanged(Modifiers::from(ModifiersState::empty())),
            WindowEvent::Ime(Ime::Commit("a".into())),
        ]
    }

    #[test]
    fn pointer_capture_reasserts_for_pointer_events_but_not_cursor_moves() {
        let events = inputs();
        assert!(should_reassert_pointer_capture(&events[0]));
        assert!(should_reassert_pointer_capture(&events[1]));
        assert!(!should_reassert_pointer_capture(&events[2]));
        assert!(!should_reassert_pointer_capture(&events[3]));
        assert!(!should_reassert_pointer_capture(&events[4]));
    }

    #[test]
    fn pointer_capture_ignores_key_repeats() {
        assert!(should_reassert_key(ElementState::Pressed, false));
        assert!(!should_reassert_key(ElementState::Pressed, true));
        assert!(!should_reassert_key(ElementState::Released, false));
    }

    #[test]
    fn a_person_is_locked_out_of_a_scripted_run_only() {
        for event in inputs() {
            assert!(locks_out(InputOrigin::User, true, &event), "{event:?}");
            assert!(!locks_out(InputOrigin::User, false, &event), "{event:?}");
        }
    }

    #[test]
    fn what_the_run_sends_still_plays() {
        for event in inputs() {
            assert!(!locks_out(InputOrigin::Agent, true, &event), "{event:?}");
            assert!(!locks_out(InputOrigin::Game, true, &event), "{event:?}");
        }
    }

    #[test]
    fn a_person_can_still_close_the_window() {
        for event in [
            WindowEvent::CloseRequested,
            WindowEvent::Resized(PhysicalSize::new(854, 480)),
        ] {
            assert!(!locks_out(InputOrigin::User, true, &event), "{event:?}");
            assert_eq!(
                held_focus(InputOrigin::User, true, &event),
                None,
                "{event:?}"
            );
        }
    }

    #[test]
    fn only_a_persons_press_claims_an_agents_window() {
        let device_id = DeviceId::dummy();
        let press = WindowEvent::MouseInput {
            device_id,
            state: ElementState::Pressed,
            button: MouseButton::Left,
        };
        assert_eq!(
            at_unclaimed(InputOrigin::User, true, &press),
            Unclaimed::Claims
        );
        // The focus alone claims nothing, and the game doesn't hear of it.
        for focused in [true, false] {
            let event = WindowEvent::Focused(focused);
            assert_eq!(
                at_unclaimed(InputOrigin::User, true, &event),
                Unclaimed::Dropped
            );
        }
        let release = WindowEvent::MouseInput {
            device_id,
            state: ElementState::Released,
            button: MouseButton::Left,
        };
        for event in inputs().into_iter().skip(1).chain([release]) {
            let want = match event {
                WindowEvent::CursorMoved { .. } => Unclaimed::Passes,
                _ => Unclaimed::Dropped,
            };
            assert_eq!(
                at_unclaimed(InputOrigin::User, true, &event),
                want,
                "{event:?}"
            );
        }
        for event in [
            WindowEvent::CloseRequested,
            WindowEvent::Resized(PhysicalSize::new(854, 480)),
        ] {
            assert_eq!(
                at_unclaimed(InputOrigin::User, true, &event),
                Unclaimed::Passes,
                "{event:?}"
            );
        }
    }

    #[test]
    fn a_persons_modifiers_are_kept_for_the_claim() {
        let shift = Modifiers::from(ModifiersState::SHIFT);
        let event = WindowEvent::ModifiersChanged(shift);
        assert_eq!(held_modifiers(InputOrigin::User, true, &event), Some(shift));
        assert_eq!(held_modifiers(InputOrigin::User, false, &event), None);
        assert_eq!(held_modifiers(InputOrigin::Agent, true, &event), None);
        assert_eq!(
            held_modifiers(InputOrigin::User, true, &WindowEvent::Focused(true)),
            None
        );
    }

    #[test]
    fn the_agent_still_drives_its_unclaimed_window() {
        let events = inputs().into_iter().chain([WindowEvent::Focused(true)]);
        for event in events {
            for origin in [InputOrigin::Agent, InputOrigin::Game] {
                assert_eq!(
                    at_unclaimed(origin, true, &event),
                    Unclaimed::Passes,
                    "{event:?}"
                );
            }
        }
    }

    #[test]
    fn an_ordinary_window_hears_everything() {
        let events = inputs().into_iter().chain([WindowEvent::Focused(true)]);
        for event in events {
            assert_eq!(
                at_unclaimed(InputOrigin::User, false, &event),
                Unclaimed::Passes,
                "{event:?}"
            );
        }
    }

    #[test]
    fn a_persons_focus_change_waits_for_the_run_to_end() {
        for focused in [true, false] {
            let event = WindowEvent::Focused(focused);
            assert_eq!(held_focus(InputOrigin::User, true, &event), Some(focused));
            assert_eq!(held_focus(InputOrigin::User, false, &event), None);
            assert!(!locks_out(InputOrigin::User, true, &event));
        }
    }
}
