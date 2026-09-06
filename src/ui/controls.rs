//! Keyboard and Gamepad Input Configuration

use crate::gba::keypad::Key;
use egui::Key as EKey;
use gilrs::{Axis, Button, Event, EventType, Gilrs};

#[derive(Clone)]
pub struct KeyBindings {
    pub a: EKey,
    pub b: EKey,
    pub select: EKey,
    pub start: EKey,
    pub right: EKey,
    pub left: EKey,
    pub up: EKey,
    pub down: EKey,
    pub r: EKey,
    pub l: EKey,
    pub turbo: EKey,
    pub rewind: EKey,
    pub pause: EKey,
    pub frame_step: EKey,
    pub quick_save: EKey,
    pub quick_load: EKey,
    pub reset: EKey,
    pub fullscreen: EKey,
    pub screenshot: EKey,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            a: EKey::Z,
            b: EKey::X,
            select: EKey::Backspace,
            start: EKey::Enter,
            right: EKey::ArrowRight,
            left: EKey::ArrowLeft,
            up: EKey::ArrowUp,
            down: EKey::ArrowDown,
            r: EKey::S,
            l: EKey::A,
            turbo: EKey::Space,
            rewind: EKey::Tab,
            pause: EKey::P,
            frame_step: EKey::F,
            quick_save: EKey::F5,
            quick_load: EKey::F8,
            reset: EKey::R,
            fullscreen: EKey::F11,
            screenshot: EKey::F12,
        }
    }
}

pub struct GamepadManager {
    gilrs: Option<Gilrs>,
    pub connected_gamepad_name: Option<String>,
    pub swap_ab: bool, // false = 8BitDo / Nintendo layout (East=A, South=B), true = Xbox layout (South=A, East=B)
    pub deadzone: f32,
    pub turbo_button_pressed: bool,
    pub rewind_button_down: bool,
    pub pause_button_pressed: bool,
    pub quick_save_pressed: bool,
    pub quick_load_pressed: bool,
    pub fullscreen_pressed: bool,
}

impl Default for GamepadManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GamepadManager {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                log::info!("Initialized Gilrs gamepad subsystem");
                Some(g)
            }
            Err(e) => {
                log::warn!("Failed to initialize gamepad subsystem: {}", e);
                None
            }
        };

        let mut mgr = Self {
            gilrs,
            connected_gamepad_name: None,
            swap_ab: false, // Default to 8BitDo / Nintendo layout
            deadzone: 0.35,
            turbo_button_pressed: false,
            rewind_button_down: false,
            pause_button_pressed: false,
            quick_save_pressed: false,
            quick_load_pressed: false,
            fullscreen_pressed: false,
        };
        mgr.refresh_connected_name();
        mgr
    }

    pub fn refresh_connected_name(&mut self) {
        if let Some(ref gilrs) = self.gilrs {
            for (_id, gamepad) in gilrs.gamepads() {
                if gamepad.is_connected() {
                    let name = gamepad.name().to_string();
                    log::info!("Connected gamepad: {}", name);
                    self.connected_gamepad_name = Some(name);
                    return;
                }
            }
        }
        self.connected_gamepad_name = None;
    }

    /// Polls gamepad events and returns the current active state of each GBA key
    pub fn poll(&mut self) -> [bool; 10] {
        // GBA Keys: [A, B, Select, Start, Right, Left, Up, Down, R, L]
        let mut keys = [false; 10];
        self.turbo_button_pressed = false;
        self.rewind_button_down = false;
        self.pause_button_pressed = false;
        self.quick_save_pressed = false;
        self.quick_load_pressed = false;
        self.fullscreen_pressed = false;

        let Some(ref mut gilrs) = self.gilrs else {
            return keys;
        };

        let mut needs_refresh = false;

        // Drain event queue to keep state up to date
        while let Some(Event { event, .. }) = gilrs.next_event() {
            match event {
                EventType::Connected | EventType::Disconnected => {
                    needs_refresh = true;
                }
                _ => {}
            }
        }

        // Query active connected gamepad
        let mut active_name = None;
        for (_id, gamepad) in gilrs.gamepads() {
            if !gamepad.is_connected() {
                continue;
            }

            active_name = Some(gamepad.name().to_string());

            // 8BitDo / Nintendo layout vs Xbox layout
            // On 8BitDo, physical button "A" is East (right), "B" is South (bottom)
            let (btn_a, btn_b) = if !self.swap_ab {
                // 8BitDo / Nintendo layout
                let a = gamepad.is_pressed(Button::East) || gamepad.is_pressed(Button::North);
                let b = gamepad.is_pressed(Button::South) || gamepad.is_pressed(Button::West);
                (a, b)
            } else {
                // Standard Xbox layout
                let a = gamepad.is_pressed(Button::South) || gamepad.is_pressed(Button::West);
                let b = gamepad.is_pressed(Button::East) || gamepad.is_pressed(Button::North);
                (a, b)
            };

            if btn_a { keys[0] = true; } // A
            if btn_b { keys[1] = true; } // B
            if gamepad.is_pressed(Button::Select) { keys[2] = true; } // Select
            if gamepad.is_pressed(Button::Start) { keys[3] = true; }  // Start

            // D-pad buttons
            let mut dpad_right = gamepad.is_pressed(Button::DPadRight);
            let mut dpad_left = gamepad.is_pressed(Button::DPadLeft);
            let mut dpad_up = gamepad.is_pressed(Button::DPadUp);
            let mut dpad_down = gamepad.is_pressed(Button::DPadDown);

            // Left Analog Stick with configurable deadzone
            if let Some(axis_x) = gamepad.axis_data(Axis::LeftStickX) {
                if axis_x.value() > self.deadzone {
                    dpad_right = true;
                } else if axis_x.value() < -self.deadzone {
                    dpad_left = true;
                }
            }
            if let Some(axis_y) = gamepad.axis_data(Axis::LeftStickY) {
                if axis_y.value() > self.deadzone {
                    dpad_up = true;
                } else if axis_y.value() < -self.deadzone {
                    dpad_down = true;
                }
            }

            if dpad_right { keys[4] = true; } // Right
            if dpad_left { keys[5] = true; }  // Left
            if dpad_up { keys[6] = true; }    // Up
            if dpad_down { keys[7] = true; }  // Down

            // Shoulders & Triggers (L1 / L2 -> L, R1 / R2 -> R)
            let l_pressed = gamepad.is_pressed(Button::LeftTrigger) || gamepad.is_pressed(Button::LeftTrigger2);
            let r_pressed = gamepad.is_pressed(Button::RightTrigger) || gamepad.is_pressed(Button::RightTrigger2);
            if r_pressed { keys[8] = true; } // R
            if l_pressed { keys[9] = true; } // L

            // Gamepad Hotkeys
            if gamepad.is_pressed(Button::RightThumb) {
                self.turbo_button_pressed = true;
            }
            if gamepad.is_pressed(Button::LeftThumb) {
                self.rewind_button_down = true;
            }
            if gamepad.is_pressed(Button::Mode) {
                self.pause_button_pressed = true;
            }
            if gamepad.is_pressed(Button::Select) && gamepad.is_pressed(Button::Start) {
                self.fullscreen_pressed = true;
            }

            // We only process the primary active gamepad
            break;
        }

        if needs_refresh || self.connected_gamepad_name.is_none() {
            self.connected_gamepad_name = active_name;
        }

        keys
    }
}

pub fn handle_input(
    ctx: &egui::Context,
    bindings: &KeyBindings,
    gamepad: &mut GamepadManager,
    set_key: &mut impl FnMut(Key, bool),
) {
    let gp_keys = gamepad.poll();

    ctx.input(|i| {
        set_key(Key::A, i.key_down(bindings.a) || gp_keys[0]);
        set_key(Key::B, i.key_down(bindings.b) || gp_keys[1]);
        set_key(Key::Select, i.key_down(bindings.select) || gp_keys[2]);
        set_key(Key::Start, i.key_down(bindings.start) || gp_keys[3]);
        set_key(Key::Right, i.key_down(bindings.right) || gp_keys[4]);
        set_key(Key::Left, i.key_down(bindings.left) || gp_keys[5]);
        set_key(Key::Up, i.key_down(bindings.up) || gp_keys[6]);
        set_key(Key::Down, i.key_down(bindings.down) || gp_keys[7]);
        set_key(Key::R, i.key_down(bindings.r) || gp_keys[8]);
        set_key(Key::L, i.key_down(bindings.l) || gp_keys[9]);
    });
}
