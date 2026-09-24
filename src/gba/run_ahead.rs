//! Run-ahead: hide a game's built-in input lag (ROADMAP M3).
//!
//! Many games only react to a button press one or more frames after it is
//! read. Run-ahead shows the player the frame the game *would* show N frames
//! from now, given the current input, and then rolls back:
//!
//! 1. Run the real frame. This is the only frame that produces audio, saves
//!    or any other side effect, so the audio stream and the game's timeline
//!    are exactly the same as without run-ahead.
//! 2. Snapshot the machine (a save state costs ~20 µs; see M2).
//! 3. Run N more frames speculatively with the same input held, with audio
//!    output, audio capture, diagnostics and save-file writes suppressed.
//! 4. Present the last speculative frame's picture.
//! 5. Restore the snapshot, so the next real frame continues the true
//!    timeline.
//!
//! **Second instance** (optional): the speculative frames run on a separate,
//! headless copy of the core that loads the snapshot each frame, so the main
//! core is never rolled back. This costs one extra state load per frame but
//! guarantees that nothing the snapshot might miss can leak into the real
//! timeline. If the shadow core can't be built or rejects the state, it falls
//! back to single-instance mode.
//!
//! Audio never glitches in either mode: speculative frames are silent by
//! construction (not muted after the fact), which `tests/run_ahead.rs`
//! checks by comparing the audio with and without run-ahead bit-for-bit.

use super::mmu::cartridge::Cartridge;
use super::Gba;

/// Upper bound on run-ahead frames. More than a few frames makes games
/// feel "twitchy" and costs N+1 emulated frames per real frame.
pub const MAX_RUN_AHEAD: u32 = 4;

#[derive(Default)]
pub struct RunAhead {
    /// Frames to run ahead (0 = off).
    pub frames: u32,
    /// Use a second, headless core for the speculative frames.
    pub second_instance: bool,
    shadow: Option<Box<Gba>>,
    /// The ROM the shadow core was built from (rebuilt if it changes).
    shadow_rom_len: usize,
    /// Set if the second instance failed and single-instance is being used.
    pub fallback_reason: Option<String>,
}

impl RunAhead {
    pub fn new(frames: u32, second_instance: bool) -> Self {
        Self { frames: frames.min(MAX_RUN_AHEAD), second_instance, ..Default::default() }
    }

    /// Run one displayed frame of `gba` with run-ahead applied. With
    /// `frames == 0` this is exactly `gba.run_frame()`.
    pub fn run_frame(&mut self, gba: &mut Gba) {
        gba.run_frame();
        let n = self.frames.min(MAX_RUN_AHEAD);
        if n == 0 {
            return;
        }
        let state = gba.save_state();

        if self.second_instance {
            if let Some(shadow) = self.shadow_for(gba) {
                if shadow.load_state(&state) {
                    shadow.mmu.keypad.keyinput = gba.mmu.keypad.keyinput;
                    for _ in 0..n {
                        shadow.run_frame();
                    }
                    gba.mmu.ppu.completed_frame.copy_from_slice(&shadow.mmu.ppu.completed_frame[..]);
                    return;
                }
                self.fallback_reason = Some("shadow core rejected the save state".into());
                self.shadow = None;
            }
        }

        // Single instance: speculate on the real core, then roll back,
        // keeping the speculative picture on screen.
        let dirty = gba.mmu.cartridge.as_ref().map(|c| c.save.is_dirty());
        gba.set_speculative(true);
        for _ in 0..n {
            gba.run_frame();
        }
        gba.set_speculative(false);
        // Show the speculative picture: the restore below would otherwise
        // put back the real (older) one, now that states carry framebuffers.
        let ahead = gba.mmu.ppu.completed_frame.clone();
        let restored = gba.load_state(&state);
        gba.mmu.ppu.completed_frame = ahead;
        debug_assert!(restored, "run-ahead: restoring our own state failed");
        // Loading marks the save chip dirty; keep the real flag so run-ahead
        // doesn't rewrite the .sav every second.
        if let (Some(cart), Some(d)) = (gba.mmu.cartridge.as_mut(), dirty) {
            cart.save.set_dirty(d);
        }
    }

    /// The shadow core, (re)built for the ROM `gba` is running.
    fn shadow_for(&mut self, gba: &Gba) -> Option<&mut Gba> {
        let cart = gba.mmu.cartridge.as_ref()?;
        if self.shadow.is_none() || self.shadow_rom_len != cart.rom.len() {
            let mut shadow = Box::new(Gba::new_headless());
            shadow.mmu.load_cartridge(Cartridge::from_bytes(cart.rom.clone()));
            shadow.reset();
            shadow.set_speculative(true);
            self.shadow_rom_len = cart.rom.len();
            self.shadow = Some(shadow);
            self.fallback_reason = None;
        }
        let shadow = self.shadow.as_deref_mut()?;
        // Keep the RTC source identical so speculation matches reality.
        if let (Some(src), Some(dst)) = (gba.mmu.cartridge.as_ref(), shadow.mmu.cartridge.as_mut()) {
            dst.rtc.clock = src.rtc.clock;
        }
        Some(shadow)
    }
}
