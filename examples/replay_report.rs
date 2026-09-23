//! Print the deterministic replay report (ROADMAP M2) so it can be compared
//! with `tests/data/replay_golden.txt` on any platform, e.g. an Android
//! device: build for the device target, `adb push`, run, diff.
fn main() {
    print!("{}", gba_simulator::gba::replay::replay_report());
}
