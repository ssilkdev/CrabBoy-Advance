//! End-to-end check that a remap actually survives a relaunch.
//!
//! The unit tests in `ui::controls` prove the wire format round-trips; this
//! proves the *file* does — that `AppConfig::save` writes something
//! `AppConfig::load` accepts, through the real config path, with the real
//! XDG resolution. That is the property the user actually cares about
//! ("my mapping is still there tomorrow"), and it is the one a serde attribute
//! typo would silently break.

use gba_simulator::ui::config::AppConfig;
use gba_simulator::ui::controls::{PadAction, PadInput, PadProfile};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Config resolution reads process-wide environment variables, so two tests
/// pointing it at different scratch dirs would race inside the same test
/// binary. Serialize them instead of resorting to `--test-threads=1`, which
/// would be an invisible requirement the next person forgets.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Points config at a scratch dir so the test never touches the developer's
/// real settings, and so two runs cannot race each other.
fn scoped_config_home(tag: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("crabboy_cfg_test_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("scratch config dir");
    std::env::set_var("XDG_CONFIG_HOME", &base);
    // macOS/Windows resolve via HOME/APPDATA instead of XDG.
    std::env::set_var("HOME", &base);
    std::env::set_var("APPDATA", &base);
    base
}

#[test]
fn a_remap_survives_a_simulated_relaunch() {
    let _guard = env_lock();
    let base = scoped_config_home("remap");

    // Session 1: user forks a profile, rebinds A, retunes the deadzone,
    // assigns it to their pad and picks that pad as Player 1.
    let mut cfg = AppConfig::default();
    let mut custom = PadProfile::ultimate2_pro();
    custom.name = "My Ultimate 2".to_string();
    custom.builtin = false;
    custom.deadzone = 0.07;
    custom.guide_scroll_speed = 1450.0;
    custom.rebind_single(PadAction::A, PadInput::Button(gilrs::Button::West));
    custom.bind(
        PadAction::GuideToggle,
        PadInput::chord(gilrs::Button::Mode, gilrs::Button::North),
    );
    cfg.controllers.profiles.push(custom);
    cfg.controllers
        .pad_profiles
        .insert("deadbeef".to_string(), "My Ultimate 2".to_string());
    cfg.controllers.preferred_pad = Some("deadbeef".to_string());
    cfg.controllers.merge_all_pads = true;
    cfg.controllers.guide_captures_pad = false;

    let written = cfg.save().expect("config should save");
    assert!(written.starts_with(&base), "wrote outside scratch: {:?}", written);
    assert!(written.exists(), "config file was not created");

    // Session 2: fresh load, as if the emulator had been restarted.
    let back = AppConfig::load();
    let p = back
        .controllers
        .profile("My Ultimate 2")
        .expect("custom profile missing after reload");

    assert!(!p.builtin);
    assert!((p.deadzone - 0.07).abs() < 1e-6, "deadzone: {}", p.deadzone);
    assert!((p.guide_scroll_speed - 1450.0).abs() < 1e-3);
    assert_eq!(p.binds(PadAction::A), &[PadInput::Button(gilrs::Button::West)]);
    assert!(p
        .binds(PadAction::GuideToggle)
        .contains(&PadInput::chord(gilrs::Button::Mode, gilrs::Button::North)));

    assert_eq!(back.controllers.preferred_pad.as_deref(), Some("deadbeef"));
    assert_eq!(
        back.controllers.profile_for("deadbeef", "whatever"),
        "My Ultimate 2"
    );
    assert!(back.controllers.merge_all_pads);
    assert!(!back.controllers.guide_captures_pad);

    // Built-ins must still be there alongside the user's copy.
    assert!(back
        .controllers
        .profile(&PadProfile::ultimate2_pro().name)
        .is_some());

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_corrupt_config_is_backed_up_rather_than_silently_replaced() {
    let _guard = env_lock();
    let base = scoped_config_home("corrupt");
    let path = gba_simulator::ui::config::config_path().expect("config path");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ this is not json").unwrap();

    let cfg = AppConfig::load();
    // Falls back to stock bindings...
    assert!(cfg
        .controllers
        .profile(&PadProfile::ultimate2_pro().name)
        .is_some());
    // ...without throwing the user's file away.
    assert!(
        path.with_extension("json.bak").exists(),
        "corrupt config was not backed up"
    );

    let _ = std::fs::remove_dir_all(&base);
}
