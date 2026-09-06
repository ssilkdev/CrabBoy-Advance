//! Diagnostic Report Data Model & Exporter
//!
//! Serializes comprehensive system health reports (Audio, Video, Bus I/O) to JSON
//! or formatted terminal output for autonomous AI Agent auditing and CI regression tests.

use super::audio_linter::{AudioHealthReport, HealthGrade};
use super::flight_recorder::IoEvent;
use super::video_linter::{VideoHealthGrade, VideoHealthReport};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverallHealthGrade {
    Pass,
    Warning,
    Critical,
}

impl std::fmt::Display for OverallHealthGrade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverallHealthGrade::Pass => write!(f, "PASS"),
            OverallHealthGrade::Warning => write!(f, "WARNING"),
            OverallHealthGrade::Critical => write!(f, "CRITICAL"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub rom_title: String,
    pub rom_game_code: String,
    pub frames_executed: u64,
    pub cycles_executed: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub overall_grade: OverallHealthGrade,
    pub system_info: SystemInfo,
    pub audio: AudioHealthReport,
    pub video: VideoHealthReport,
    pub all_anomalies: Vec<String>,
    pub recent_io_events: Vec<IoEvent>,
}

impl DiagnosticReport {
    pub fn new(
        system_info: SystemInfo,
        audio: AudioHealthReport,
        video: VideoHealthReport,
        recent_io_events: Vec<IoEvent>,
    ) -> Self {
        let mut all_anomalies = Vec::new();
        all_anomalies.extend(audio.anomalies.iter().cloned());
        all_anomalies.extend(video.anomalies.iter().cloned());

        let overall_grade = if audio.grade == HealthGrade::Critical
            || video.grade == VideoHealthGrade::Critical
        {
            OverallHealthGrade::Critical
        } else if audio.grade == HealthGrade::Warning
            || video.grade == VideoHealthGrade::Warning
        {
            OverallHealthGrade::Warning
        } else {
            OverallHealthGrade::Pass
        };

        Self {
            overall_grade,
            system_info,
            audio,
            video,
            all_anomalies,
            recent_io_events,
        }
    }

    /// Serialize report to formatted JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Save report directly to file
    pub fn save_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json = self
            .to_json()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())
    }

    /// Human/AI-readable terminal summary
    pub fn format_cli_summary(&self) -> String {
        let mut out = String::new();
        out.push_str("================================================================================\n");
        out.push_str("                     🦀 CRABBOY ADVANCE DIAGNOSTIC REPORT                       \n");
        out.push_str("================================================================================\n");
        out.push_str(&format!(
            "ROM Title:          {}\n",
            if self.system_info.rom_title.is_empty() {
                "Unknown"
            } else {
                &self.system_info.rom_title
            }
        ));
        out.push_str(&format!(
            "Game Code:          {}\n",
            if self.system_info.rom_game_code.is_empty() {
                "N/A"
            } else {
                &self.system_info.rom_game_code
            }
        ));
        out.push_str(&format!(
            "Frames Monitored:   {}\n",
            self.system_info.frames_executed
        ));
        out.push_str(&format!(
            "Cycles Executed:    {}\n",
            self.system_info.cycles_executed
        ));
        out.push_str(&format!(
            "Overall Health:     [{}]\n",
            self.overall_grade
        ));
        out.push_str("--------------------------------------------------------------------------------\n");
        out.push_str("AUDIO SUBSYSTEM HEALTH:\n");
        out.push_str(&format!("  Grade:            {:?}\n", self.audio.grade));
        out.push_str(&format!("  Peak Amplitude:   {:.3}\n", self.audio.peak_amplitude));
        out.push_str(&format!("  DC Bias:          {:.4}\n", self.audio.dc_bias));
        out.push_str(&format!(
            "  Clipping Rate:    {:.2}% ({} samples)\n",
            self.audio.clipping_rate * 100.0,
            self.audio.clipping_samples
        ));
        out.push_str(&format!(
            "  Silence Ratio:    {:.1}%\n",
            self.audio.silence_ratio * 100.0
        ));
        out.push_str(&format!(
            "  Channel Triggers: [DirectA: {}, DirectB: {}, SQ1: {}, SQ2: {}, Wave: {}, Noise: {}]\n",
            self.audio.channel_triggers[0],
            self.audio.channel_triggers[1],
            self.audio.channel_triggers[2],
            self.audio.channel_triggers[3],
            self.audio.channel_triggers[4],
            self.audio.channel_triggers[5],
        ));
        if !self.audio.stuck_notes_detected.is_empty() {
            out.push_str("  Stuck Notes:      ");
            out.push_str(&self.audio.stuck_notes_detected.join(", "));
            out.push('\n');
        }

        out.push_str("--------------------------------------------------------------------------------\n");
        out.push_str("VIDEO SUBSYSTEM HEALTH:\n");
        out.push_str(&format!("  Grade:            {:?}\n", self.video.grade));
        out.push_str(&format!(
            "  Latest Frame CRC: 0x{:08X}\n",
            self.video.latest_frame_crc32
        ));
        out.push_str(&format!(
            "  Avg Brightness:   {:.2}%\n",
            self.video.average_brightness * 100.0
        ));
        out.push_str(&format!(
            "  Frozen Frames:    {}\n",
            self.video.frozen_frame_count
        ));
        out.push_str(&format!(
            "  Active Sprites:   {}\n",
            self.video.active_sprites
        ));
        out.push_str(&format!(
            "  Layers Active:    0x{:02X}\n",
            self.video.visible_layers_mask
        ));

        out.push_str("--------------------------------------------------------------------------------\n");
        out.push_str(&format!("ANOMALIES DETECTED ({})\n", self.all_anomalies.len()));
        if self.all_anomalies.is_empty() {
            out.push_str("  (None - System performing within nominal parameters)\n");
        } else {
            for (idx, anomaly) in self.all_anomalies.iter().enumerate() {
                out.push_str(&format!("  {}. {}\n", idx + 1, anomaly));
            }
        }

        out.push_str("--------------------------------------------------------------------------------\n");
        out.push_str(&format!(
            "RECENT IO BUS EVENTS (showing last {} recorded):\n",
            self.recent_io_events.len().min(15)
        ));
        let display_count = self.recent_io_events.len().min(15);
        let start_idx = self.recent_io_events.len().saturating_sub(display_count);
        for ev in &self.recent_io_events[start_idx..] {
            out.push_str(&format!(
                "  [@{:>10}] PC=0x{:08X} {} 0x{:03X} ({:<10}) = 0x{:08X} ({}b)\n",
                ev.cycle,
                ev.pc,
                if ev.is_write { "WR" } else { "RD" },
                ev.addr,
                ev.reg_name,
                ev.val,
                ev.size,
            ));
        }
        out.push_str("================================================================================\n");

        out
    }
}
