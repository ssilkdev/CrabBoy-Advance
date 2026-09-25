//! Game Boy Advance Simulator Library

pub mod autosave;
pub mod dmg;
pub mod frame_pacing;
pub mod fs_util;
pub mod gba;
#[cfg(feature = "desktop")]
pub mod ui;
