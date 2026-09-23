//! ROADMAP M4: Save synchronization between desktop and Android.
//!
//! "Done when: a game started on desktop continues on Android and back
//! again with nothing lost."
//!
//! Verifies:
//! 1. Bidirectional sync between simulated desktop and Android directory structures.
//! 2. Automatic mapping between desktop Slot 0 (`{stem}_slot0.state`) and Android primary (`{stem}.state`).
//! 3. Newest-wins conflict resolution with automatic `.bak.<timestamp>` archives.
//! 4. Backup limit retention and rotation.
//! 5. Byte-identical files detected and skipped without disk writes.
//! 6. Real v3 save-state roundtrip: state saved on desktop continues execution on Android.

use gba_simulator::gba::replay::boot;
use gba_simulator::gba::save_sync::{
    are_files_identical, is_syncable_file, list_backups_for, SaveSync, SyncStatus,
};
use gba_simulator::ui::save_manager::SaveStateManager;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(1);

struct TestEnv {
    base_dir: PathBuf,
    desktop_saves: PathBuf,
    android_states: PathBuf,
    android_roms: PathBuf,
    cloud_sync: PathBuf,
}

impl TestEnv {
    fn new(name: &str) -> Self {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let base_dir = std::env::temp_dir().join(format!("crabboy_test_sync_{name}_{pid}_{id}"));
        let desktop_saves = base_dir.join("desktop_saves");
        let android_states = base_dir.join("android/states");
        let android_roms = base_dir.join("android/roms");
        let cloud_sync = base_dir.join("cloud_drive");

        fs::create_dir_all(&desktop_saves).unwrap();
        fs::create_dir_all(&android_states).unwrap();
        fs::create_dir_all(&android_roms).unwrap();
        fs::create_dir_all(&cloud_sync).unwrap();

        Self {
            base_dir,
            desktop_saves,
            android_states,
            android_roms,
            cloud_sync,
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base_dir);
    }
}

fn set_file_time(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime");
    file.set_modified(time).expect("set_modified");
}

#[test]
fn save_sync_bidirectional_and_alias_mapping() {
    let env = TestEnv::new("bidirectional");

    // 1. Desktop starts playing Pokémon Emerald.
    // Desktop creates battery save and Slot 0 save state.
    let desktop_sav = env.desktop_saves.join("pokemon_emerald.sav");
    let desktop_slot0 = env.desktop_saves.join("pokemon_emerald_slot0.state");

    fs::write(&desktop_sav, b"DESKTOP_BATTERY_SAVE_V1").unwrap();
    fs::write(&desktop_slot0, b"DESKTOP_STATE_SLOT0_V1").unwrap();

    let t1 = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_file_time(&desktop_sav, t1);
    set_file_time(&desktop_slot0, t1);

    // Desktop synchronizes to cloud folder
    let desktop_syncer = SaveSync::new(&env.cloud_sync, vec![env.desktop_saves.clone()]);
    let report1 = desktop_syncer.sync_all();

    assert_eq!(report1.total_transferred(), 3); // .sav, _slot0.state, and mapped .state
    assert!(env.cloud_sync.join("pokemon_emerald.sav").is_file());
    assert!(env.cloud_sync.join("pokemon_emerald_slot0.state").is_file());
    assert!(env.cloud_sync.join("pokemon_emerald.state").is_file()); // Alias created for Android!

    // Verify content in cloud matches desktop
    assert_eq!(
        fs::read(env.cloud_sync.join("pokemon_emerald.sav")).unwrap(),
        b"DESKTOP_BATTERY_SAVE_V1"
    );
    assert_eq!(
        fs::read(env.cloud_sync.join("pokemon_emerald.state")).unwrap(),
        b"DESKTOP_STATE_SLOT0_V1"
    );

    // 2. Android synchronizes from cloud folder
    // Android has states in android/states and roms/saves in android/roms
    let android_syncer = SaveSync::new(
        &env.cloud_sync,
        vec![env.android_states.clone(), env.android_roms.clone()],
    );
    let _report2 = android_syncer.sync_all();

    // Android receives .state in states/ and .sav in roms/ (or states/)
    let android_state = env.android_states.join("pokemon_emerald.state");
    assert!(android_state.is_file(), "Android received state file");
    assert_eq!(fs::read(&android_state).unwrap(), b"DESKTOP_STATE_SLOT0_V1");

    let android_sav_in_roms = env.android_roms.join("pokemon_emerald.sav");
    let android_sav_in_states = env.android_states.join("pokemon_emerald.sav");
    let android_sav = if android_sav_in_roms.is_file() {
        android_sav_in_roms
    } else {
        android_sav_in_states
    };
    assert!(android_sav.is_file(), "Android received battery save");
    assert_eq!(fs::read(&android_sav).unwrap(), b"DESKTOP_BATTERY_SAVE_V1");

    // 3. Android plays ahead and saves a newer state and newer battery save!
    let t2 = t1 + Duration::from_secs(3600); // 1 hour later
    fs::write(&android_state, b"ANDROID_STATE_SLOT0_V2_PROGRESS").unwrap();
    fs::write(&android_sav, b"ANDROID_BATTERY_SAVE_V2_PROGRESS").unwrap();
    set_file_time(&android_state, t2);
    set_file_time(&android_sav, t2);

    let report3 = android_syncer.sync_all();
    assert!(report3.total_transferred() >= 2);

    // Cloud is now updated with Android's version
    assert_eq!(
        fs::read(env.cloud_sync.join("pokemon_emerald.state")).unwrap(),
        b"ANDROID_STATE_SLOT0_V2_PROGRESS"
    );

    // 4. Desktop synchronizes from cloud.
    // Desktop's older files must be backed up, and replaced with Android's newer version.
    let report4 = desktop_syncer.sync_all();
    assert!(report4.total_transferred() >= 2);
    assert!(report4.total_backups() >= 1);

    // Desktop now has Android's progress in both slot0 and battery save
    assert_eq!(
        fs::read(&desktop_sav).unwrap(),
        b"ANDROID_BATTERY_SAVE_V2_PROGRESS"
    );
    assert_eq!(
        fs::read(&desktop_slot0).unwrap(),
        b"ANDROID_STATE_SLOT0_V2_PROGRESS"
    );

    // Desktop's older files were backed up as .bak
    let backups_sav = list_backups_for(&desktop_sav);
    assert!(!backups_sav.is_empty(), "Desktop battery save backed up");
    assert_eq!(fs::read(&backups_sav[0]).unwrap(), b"DESKTOP_BATTERY_SAVE_V1");

    let backups_state = list_backups_for(&desktop_slot0);
    assert!(!backups_state.is_empty(), "Desktop slot 0 state backed up");
    assert_eq!(
        fs::read(&backups_state[0]).unwrap(),
        b"DESKTOP_STATE_SLOT0_V1"
    );
}

#[test]
fn save_sync_newest_wins_and_backup_rotation() {
    let env = TestEnv::new("rotation");
    let local_file = env.desktop_saves.join("game.sav");
    let sync_file = env.cloud_sync.join("game.sav");

    fs::write(&local_file, b"INITIAL_DATA").unwrap();
    let syncer = SaveSync::new(&env.cloud_sync, vec![env.desktop_saves.clone()]).with_max_backups(3);

    // Initial sync
    let status = syncer.sync_file_pair(&local_file, &sync_file).unwrap();
    assert_eq!(status, SyncStatus::CopiedToSync);

    let base_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Simulate 5 successive updates in sync folder, each newer than local
    for i in 1..=5 {
        let content = format!("REMOTE_UPDATE_{i}");
        fs::write(&sync_file, content.as_bytes()).unwrap();
        set_file_time(&sync_file, base_time + Duration::from_secs(i * 100));
        set_file_time(&local_file, base_time + Duration::from_secs(i * 100 - 50));

        let status = syncer.sync_file_pair(&local_file, &sync_file).unwrap();
        assert!(
            matches!(status, SyncStatus::UpdatedLocalFromSync { .. }),
            "Expected update from sync on iteration {i}"
        );
        assert_eq!(fs::read(&local_file).unwrap(), content.as_bytes());

        // Check backup count rotation (should never exceed max_backups = 3)
        let backups = list_backups_for(&local_file);
        assert!(
            backups.len() <= 3,
            "Backup count {} exceeded maximum of 3 on iteration {i}",
            backups.len()
        );
    }

    let final_backups = list_backups_for(&local_file);
    assert_eq!(final_backups.len(), 3, "Exactly 3 backups retained");
}

#[test]
fn save_sync_identical_files_detected_and_skipped() {
    let env = TestEnv::new("identical");
    let local_file = env.desktop_saves.join("game.sav");
    let sync_file = env.cloud_sync.join("game.sav");

    let data = b"EXACTLY_IDENTICAL_SAVE_DATA_0123456789";
    fs::write(&local_file, data).unwrap();
    fs::write(&sync_file, data).unwrap();

    let syncer = SaveSync::new(&env.cloud_sync, vec![env.desktop_saves.clone()]);
    let status = syncer.sync_file_pair(&local_file, &sync_file).unwrap();
    assert_eq!(status, SyncStatus::UpToDate);

    // Verify helper
    assert!(are_files_identical(&local_file, &sync_file).unwrap());
    assert!(is_syncable_file(&local_file));
    assert!(is_syncable_file(Path::new("game.state")));
    assert!(!is_syncable_file(Path::new("game.gba")));
}

#[test]
fn save_sync_real_v3_state_roundtrip() {
    let env = TestEnv::new("v3_roundtrip");

    // 1. Boot a real GBA core (as desktop)
    let mut desktop_core = boot();
    for _ in 0..30 {
        desktop_core.run_frame();
    }
    // 2. Save state via SaveStateManager into Slot 0
    let mut save_mgr = SaveStateManager::new(&env.desktop_saves);
    save_mgr
        .save_slot(0, &desktop_core, "emerald")
        .expect("save slot 0");

    // Check that slot 0 wrote emerald_slot0.state and alias emerald.state
    assert!(env.desktop_saves.join("emerald_slot0.state").is_file());
    assert!(env.desktop_saves.join("emerald.state").is_file());

    // 3. Desktop syncs to cloud
    let desktop_syncer = SaveSync::new(&env.cloud_sync, vec![env.desktop_saves.clone()]);
    let rep = desktop_syncer.sync_all();
    assert!(rep.total_transferred() >= 1);

    // 4. Android syncs from cloud into android_states
    let android_syncer = SaveSync::new(&env.cloud_sync, vec![env.android_states.clone()]);
    let rep_android = android_syncer.sync_all();
    assert!(rep_android.total_transferred() >= 1);

    let android_state_file = env.android_states.join("emerald.state");
    assert!(android_state_file.is_file(), "Android state file exists");

    // 5. Android boots a fresh GBA core and loads the synced state
    let mut android_core = boot();
    let state_bytes = fs::read(&android_state_file).expect("read android state");
    let loaded = android_core.load_state(&state_bytes);
    assert!(loaded, "Android core successfully loaded v3 state");

    // 6. Run 30 more frames on both cores and verify they stay 100% bit-identical
    for _ in 0..30 {
        desktop_core.run_frame();
        android_core.run_frame();
    }

    assert_eq!(
        desktop_core.get_framebuffer(),
        android_core.get_framebuffer(),
        "Framebuffer bit-identical after desktop -> cloud -> android save sync!"
    );
    assert_eq!(
        desktop_core.cpu.regs[15], android_core.cpu.regs[15],
        "CPU program counter identical"
    );
}
