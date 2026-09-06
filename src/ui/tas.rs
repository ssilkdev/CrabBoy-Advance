//! Tool-Assisted Speedrun (TAS) Engine & Input Macro Recorder

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TasMode {
    #[default]
    Idle,
    Recording,
    Playback,
}

#[derive(Default)]
pub struct TasEngine {
    pub mode: TasMode,
    pub recorded_inputs: Vec<[bool; 10]>, // [A, B, Select, Start, Right, Left, Up, Down, R, L] per frame
    pub playback_frame: usize,
    pub pause_on_finish: bool,
}

impl TasEngine {
    pub fn new() -> Self {
        Self {
            mode: TasMode::Idle,
            recorded_inputs: Vec::with_capacity(3600), // 1 minute buffer
            playback_frame: 0,
            pause_on_finish: true,
        }
    }

    pub fn start_recording(&mut self) {
        self.mode = TasMode::Recording;
        self.recorded_inputs.clear();
        self.playback_frame = 0;
    }

    pub fn stop(&mut self) {
        self.mode = TasMode::Idle;
        self.playback_frame = 0;
    }

    pub fn start_playback(&mut self) {
        if !self.recorded_inputs.is_empty() {
            self.mode = TasMode::Playback;
            self.playback_frame = 0;
        }
    }

    /// Records current frame input if in recording mode
    pub fn record_frame(&mut self, keys: &[bool; 10]) {
        if self.mode == TasMode::Recording {
            self.recorded_inputs.push(*keys);
        }
    }

    /// Retrieves current playback inputs and advances playback cursor
    pub fn get_playback_inputs(&mut self) -> Option<[bool; 10]> {
        if self.mode != TasMode::Playback {
            return None;
        }

        if self.playback_frame < self.recorded_inputs.len() {
            let input = self.recorded_inputs[self.playback_frame];
            self.playback_frame += 1;
            Some(input)
        } else {
            self.mode = TasMode::Idle;
            None
        }
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let mut out = String::new();
        out.push_str("# GBA Simulator TAS Input Script\n");
        out.push_str("# Format: [Frame]: [A,B,Select,Start,Right,Left,Up,Down,R,L]\n");

        for (idx, frame) in self.recorded_inputs.iter().enumerate() {
            let s: String = frame.iter().map(|&pressed| if pressed { '1' } else { '0' }).collect();
            out.push_str(&format!("{:06}: {}\n", idx + 1, s));
        }

        fs::write(path, out)
    }

    pub fn load_from_file<P: AsRef<Path>>(&mut self, path: P) -> std::io::Result<()> {
        let content = fs::read_to_string(path)?;
        self.recorded_inputs.clear();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some((_frame_str, keys_str)) = line.split_once(':') {
                let keys_clean = keys_str.trim();
                let mut frame = [false; 10];
                for (i, ch) in keys_clean.chars().take(10).enumerate() {
                    if ch == '1' || ch == 'X' || ch == 'x' {
                        frame[i] = true;
                    }
                }
                self.recorded_inputs.push(frame);
            }
        }

        self.mode = TasMode::Idle;
        self.playback_frame = 0;
        Ok(())
    }
}
