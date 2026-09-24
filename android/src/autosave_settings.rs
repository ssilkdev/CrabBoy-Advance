//! `autosave.txt`: the auto-save on/off switch and interval, as
//! "<enabled 0|1> <minutes>". Missing or broken → on, every 5 minutes.

// Mirrors gba_simulator::autosave (a target-only dependency, so host tests
// here can't import it); the core clamps again on use.
const DEFAULT_INTERVAL_MINUTES: u32 = 5;

fn clamp_minutes(m: u32) -> u32 {
    m.clamp(1, 60)
}

pub fn parse(s: &str) -> (bool, u32) {
    let mut it = s.split_whitespace();
    let on = it.next().map(|v| v != "0").unwrap_or(true);
    let min = it.next().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_INTERVAL_MINUTES);
    (on, clamp_minutes(min))
}

pub fn format(enabled: bool, minutes: u32) -> String {
    format!("{} {}", enabled as u8, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_defaults() {
        assert_eq!(parse(""), (true, 5));
        assert_eq!(parse("0 3"), (false, 3));
        assert_eq!(parse("1 0"), (true, 1), "clamped to 1 minute");
        assert_eq!(parse("garbage"), (true, 5));
    }

    #[test]
    fn round_trips() {
        assert_eq!(parse(&format(false, 12)), (false, 12));
        assert_eq!(parse(&format(true, 1)), (true, 1));
    }
}
