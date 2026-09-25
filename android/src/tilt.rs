//! Experimental tilt controls: lean the phone to press the D-pad.
//!
//! Input is the gravity reading from the motion sensor (Android's device
//! axes: x right, y up, z out of the screen), rotated into screen axes. The
//! pose when tilt is turned on (or re-centred) is "neutral"; leaning away
//! from it far enough presses the direction the screen leans towards, like
//! a marble rolling. Plain logic, so it is tested on the host.

use egui::Vec2;

use crate::touch::{self, Buttons};

/// How far from neutral (degrees) the phone must lean before a direction
/// presses...
pub const PRESS: f32 = 12.0;
/// ...and how far back it must come before it lets go, so a hand hovering
/// near the threshold doesn't chatter.
pub const RELEASE: f32 = 8.0;

/// `Display.getRotation()`: 0, 1, 2, 3 = 0, 90, 180, 270 degrees.
pub fn to_screen(g: [f32; 3], rotation: i32) -> [f32; 3] {
    let [x, y, z] = g;
    let (sx, sy) = match rotation & 3 {
        0 => (x, y),
        1 => (-y, x),
        2 => (-x, -y),
        _ => (y, -x),
    };
    [sx, sy, z]
}

#[derive(Debug, Default)]
pub struct Tilt {
    /// (roll, pitch) in degrees at the neutral pose.
    neutral: Option<(f32, f32)>,
    held: u16,
}

impl Tilt {
    /// Use the next reading as the neutral pose.
    pub fn recenter(&mut self) {
        self.neutral = None;
        self.held = 0;
    }

    /// Feed a gravity reading (m/s², device axes) and the display rotation;
    /// returns the D-pad directions held.
    pub fn update(&mut self, gravity: [f32; 3], rotation: i32) -> Buttons {
        let s = to_screen(gravity, rotation);
        let len = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
        if !len.is_finite() || len < 1.0 {
            // No reading yet (or free fall): hold nothing.
            self.held = 0;
            return Buttons(0);
        }
        // Roll: how far the right edge is lowered. Pitch: how far the top
        // edge is lowered (0 = upright). Both are measured about the
        // screen's own axes, so they read the same however far back the
        // phone is held.
        let [x, y, z] = s;
        let roll = (-x).atan2(z).to_degrees();
        let pitch = x.hypot(z).atan2(y).to_degrees();
        let (roll0, pitch0) = *self.neutral.get_or_insert((roll, pitch));
        let wrap = |d: f32| (d + 540.0).rem_euclid(360.0) - 180.0;
        // Screen coordinates, y down: lowering an edge presses towards it,
        // like a marble rolling.
        let lean = Vec2::new(wrap(roll - roll0), -(pitch - pitch0));
        let threshold = if self.held != 0 { RELEASE } else { PRESS };
        self.held = if lean.length() >= threshold { touch::direction_bits(lean) } else { 0 };
        Buttons(self.held)
    }

    pub fn held(&self) -> u16 {
        self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: f32 = 9.81;

    /// Gravity reading for a portrait phone tipped back 40 degrees from
    /// upright (a comfortable grip) plus `back` more, then rolled `right`
    /// degrees about its long axis (right edge down).
    fn reading(right: f32, back: f32) -> [f32; 3] {
        let (r, b) = (right.to_radians(), (40.0 + back).to_radians());
        [-G * b.sin() * r.sin(), G * b.cos(), G * b.sin() * r.cos()]
    }

    #[test]
    fn neutral_presses_nothing_and_leaning_presses_that_way() {
        let mut t = Tilt::default();
        assert_eq!(t.update(reading(0.0, 0.0), 0).0, 0);
        assert_eq!(t.update(reading(5.0, 0.0), 0).0, 0, "small wobble is ignored");
        assert_eq!(t.update(reading(20.0, 0.0), 0).0, Buttons::RIGHT);
        assert_eq!(t.update(reading(-20.0, 0.0), 0).0, Buttons::LEFT);
        assert_eq!(t.update(reading(0.0, 25.0), 0).0, Buttons::UP, "top edge lowered");
        assert_eq!(t.update(reading(0.0, -25.0), 0).0, Buttons::DOWN);
        assert_eq!(t.update(reading(20.0, 20.0), 0).0, Buttons::RIGHT | Buttons::UP);
    }

    #[test]
    fn release_has_hysteresis() {
        let mut t = Tilt::default();
        t.update(reading(0.0, 0.0), 0);
        assert_eq!(t.update(reading(10.0, 0.0), 0).0, 0, "10 degrees is under the press threshold");
        assert_eq!(t.update(reading(15.0, 0.0), 0).0, Buttons::RIGHT);
        assert_eq!(t.update(reading(10.0, 0.0), 0).0, Buttons::RIGHT, "still held above the release threshold");
        assert_eq!(t.update(reading(6.0, 0.0), 0).0, 0);
    }

    #[test]
    fn landscape_rotation_maps_to_screen_directions() {
        // Rotation 1 (90 degrees): the device's top is the screen's left.
        // Lowering the device's top edge = lowering the screen's left edge.
        let mut t = Tilt::default();
        let flat = [0.0, 0.0, G];
        t.update(flat, 1);
        let a = 20f32.to_radians();
        let top_down = [0.0, -G * a.sin(), G * a.cos()];
        assert_eq!(t.update(top_down, 1).0, Buttons::LEFT);
        let mut t = Tilt::default();
        t.update(flat, 3);
        assert_eq!(t.update(top_down, 3).0, Buttons::RIGHT);
    }

    #[test]
    fn recenter_makes_the_current_pose_neutral() {
        let mut t = Tilt::default();
        t.update(reading(0.0, 0.0), 0);
        assert_eq!(t.update(reading(30.0, 0.0), 0).0, Buttons::RIGHT);
        t.recenter();
        assert_eq!(t.update(reading(30.0, 0.0), 0).0, 0);
        assert_eq!(t.update(reading(55.0, 0.0), 0).0, Buttons::RIGHT);
    }

    #[test]
    fn no_reading_presses_nothing() {
        let mut t = Tilt::default();
        assert_eq!(t.update([0.0; 3], 0).0, 0);
        assert_eq!(t.update([f32::NAN; 3], 0).0, 0);
    }
}
