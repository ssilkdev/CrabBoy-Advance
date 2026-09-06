//! Comprehensive Diagnostic Subsystem for Autonomous AI Agent Auditing & Continuous Integration
//!
//! Integrates:
//! 1. Flight Recorder - zero-allocation circular buffer of IO register accesses
//! 2. Audio Health Linter - DC bias, clipping, stuck notes, rapid re-trigger loops
//! 3. Video Health Linter - screen freeze detection, black screen hangs, frame CRC32
//! 4. Diagnostic Report - JSON and CLI report generation

pub mod audio_linter;
pub mod flight_recorder;
pub mod report;
pub mod video_linter;

pub use audio_linter::{AudioHealthReport, AudioLinter, HealthGrade};
pub use flight_recorder::{FlightRecorder, IoAccessSize, IoEvent, FLIGHT_RECORDER_CAPACITY};
pub use report::{DiagnosticReport, OverallHealthGrade, SystemInfo};
pub use video_linter::{VideoHealthGrade, VideoHealthReport, VideoLinter};

use crate::gba::apu::Apu;
use crate::gba::mmu::Mmu;
use crate::gba::ppu::Ppu;

#[derive(Clone, Debug)]
pub struct SystemDiagnostics {
    pub audio_linter: AudioLinter,
    pub video_linter: VideoLinter,
    pub enabled: bool,
}

impl Default for SystemDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemDiagnostics {
    pub fn new() -> Self {
        Self {
            audio_linter: AudioLinter::new(),
            video_linter: VideoLinter::new(),
            enabled: true,
        }
    }

    /// Update per-frame diagnostics from PPU and APU state
    pub fn on_frame(&mut self, ppu: &Ppu, apu: &Apu) {
        if !self.enabled {
            return;
        }
        self.video_linter.on_frame(ppu);
        self.audio_linter.on_frame(apu);
    }

    /// Process audio samples pushed to host
    pub fn process_audio_samples(&mut self, samples: &[f32]) {
        if !self.enabled {
            return;
        }
        self.audio_linter.process_samples(samples);
    }

    /// Reset diagnostic telemetry states
    pub fn reset(&mut self) {
        self.audio_linter.reset();
        self.video_linter.reset();
    }

    /// Generate an authoritative DiagnosticReport
    pub fn generate_report(&self, mmu: &Mmu, frames: u64, cycles: u64) -> DiagnosticReport {
        let (rom_title, rom_game_code) = if let Some(ref cart) = mmu.cartridge {
            (cart.title.clone(), cart.game_code.clone())
        } else {
            ("No Cartridge".to_string(), "N/A".to_string())
        };

        let system_info = SystemInfo {
            rom_title,
            rom_game_code,
            frames_executed: frames,
            cycles_executed: cycles,
        };

        let audio_report = self.audio_linter.evaluate_health();
        let video_report = self.video_linter.evaluate_health();
        let recent_io = mmu.flight_recorder.recent_events(FLIGHT_RECORDER_CAPACITY);

        DiagnosticReport::new(system_info, audio_report, video_report, recent_io)
    }
}
