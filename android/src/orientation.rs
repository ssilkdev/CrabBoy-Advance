//! Screen rotation preference.
//!
//! The default follows the accelerometer even when the system rotation lock
//! is on. With the lock on, `fullUser` keeps the app in portrait, and immersive
//! mode hides the navigation bar's "rotate" button, so the user would have no
//! way to turn the game sideways. Plain logic, host-tested.

/// `android.content.pm.ActivityInfo.SCREEN_ORIENTATION_*` values.
mod activity_info {
    pub const SENSOR_LANDSCAPE: i32 = 6;
    pub const SENSOR_PORTRAIT: i32 = 7;
    pub const FULL_SENSOR: i32 = 10;
    pub const FULL_USER: i32 = 13;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Orientation {
    /// Rotate with the device (all four directions), ignoring the rotation lock.
    #[default]
    Auto,
    /// Rotate only if system auto-rotate is on.
    System,
    /// Portrait only (upright or upside down, by sensor).
    Portrait,
    /// Landscape only (either side, by sensor).
    Landscape,
}

impl Orientation {
    pub const ALL: [Orientation; 4] = [Self::Auto, Self::System, Self::Portrait, Self::Landscape];

    /// Value for `Activity.setRequestedOrientation`.
    pub fn activity_info(self) -> i32 {
        match self {
            Self::Auto => activity_info::FULL_SENSOR,
            Self::System => activity_info::FULL_USER,
            Self::Portrait => activity_info::SENSOR_PORTRAIT,
            Self::Landscape => activity_info::SENSOR_LANDSCAPE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Rotation: Auto",
            Self::System => "Rotation: Follow system",
            Self::Portrait => "Rotation: Portrait",
            Self::Landscape => "Rotation: Landscape",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&o| o == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// Stored form (one word in `orientation.txt`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::System => "system",
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }

    /// Unknown or missing values fall back to `Auto`.
    pub fn parse(s: &str) -> Self {
        Self::ALL.into_iter().find(|o| o.as_str() == s.trim()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_through_every_mode_and_back() {
        let mut o = Orientation::Auto;
        let mut seen = vec![];
        for _ in 0..4 {
            seen.push(o);
            o = o.next();
        }
        assert_eq!(o, Orientation::Auto);
        assert_eq!(seen, Orientation::ALL);
    }

    #[test]
    fn round_trips_through_storage() {
        for o in Orientation::ALL {
            assert_eq!(Orientation::parse(o.as_str()), o);
        }
        assert_eq!(Orientation::parse("landscape\n"), Orientation::Landscape);
        assert_eq!(Orientation::parse(""), Orientation::Auto);
        assert_eq!(Orientation::parse("garbage"), Orientation::Auto);
    }

    #[test]
    fn default_ignores_the_rotation_lock() {
        // FULL_SENSOR, not FULL_USER: holding the phone sideways must rotate
        // the game even with system auto-rotate off.
        assert_eq!(Orientation::default().activity_info(), 10);
        assert_eq!(Orientation::System.activity_info(), 13);
    }
}
