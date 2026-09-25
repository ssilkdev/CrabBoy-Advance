//! Game Boy Advance Simulator Library

pub mod autosave;
pub mod dmg;
pub mod frame_pacing;
pub mod fs_util;
pub mod gba;
pub mod nds;
#[cfg(feature = "desktop")]
pub mod ui;
