//! Pure Rust Animated GIF89a Gameplay Video Clip Recorder

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct GifRecorder {
    pub is_recording: bool,
    pub frames_recorded: usize,
    pub max_frames: usize,
    frame_skip: usize,
    frame_counter: usize,
    captured_frames: Vec<Vec<u8>>, // Palette index frames (240x160)
    pub width: usize,
    pub height: usize,
}

impl Default for GifRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl GifRecorder {
    pub fn new() -> Self {
        Self {
            is_recording: false,
            frames_recorded: 0,
            max_frames: 300, // 10 seconds at 30 FPS
            frame_skip: 2,   // Capture every 2nd frame (60 FPS -> 30 FPS)
            frame_counter: 0,
            captured_frames: Vec::with_capacity(300),
            width: 240,
            height: 160,
        }
    }

    pub fn start_recording(&mut self) {
        self.is_recording = true;
        self.frames_recorded = 0;
        self.frame_counter = 0;
        self.captured_frames.clear();
    }

    pub fn stop_and_save(&mut self, rom_name: &str) -> std::io::Result<(PathBuf, String)> {
        self.is_recording = false;
        let _ = fs::create_dir_all("recordings");

        let safe_name: String = rom_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let filename = format!("{}_{}.gif", safe_name, now);
        let path = Path::new("recordings").join(&filename);

        if !self.captured_frames.is_empty() {
            encode_gif(&path, self.width, self.height, &self.captured_frames, 3)?;
        }

        self.captured_frames.clear();
        self.frames_recorded = 0;
        Ok((path, filename))
    }

    /// Captures a 240x160 RGBA framebuffer if currently recording
    pub fn capture_frame(&mut self, fb: &[u32; 240 * 160]) {
        if !self.is_recording {
            return;
        }

        self.frame_counter += 1;
        if !self.frame_counter.is_multiple_of(self.frame_skip) {
            return;
        }

        // Quantize 32-bit RGBA pixels into 256-color palette indices (6x6x6 RGB cube + 16 grayscale)
        let mut indexed_frame = Vec::with_capacity(240 * 160);
        for &p in fb.iter() {
            let r = (p & 0xFF) as u8;
            let g = ((p >> 8) & 0xFF) as u8;
            let b = ((p >> 16) & 0xFF) as u8;

            // Map into 6x6x6 color cube: 0..215
            let r_idx = ((r as u16 * 5 + 127) / 255) as u8;
            let g_idx = ((g as u16 * 5 + 127) / 255) as u8;
            let b_idx = ((b as u16 * 5 + 127) / 255) as u8;
            let color_idx = r_idx * 36 + g_idx * 6 + b_idx;
            indexed_frame.push(color_idx);
        }

        self.captured_frames.push(indexed_frame);
        self.frames_recorded += 1;

        if self.frames_recorded >= self.max_frames {
            self.is_recording = false;
        }
    }
}

/// Builds standard 256-color Global Color Table (6x6x6 RGB cube + 40 grayscales)
fn build_global_color_table() -> [u8; 768] {
    let mut palette = [0u8; 768];
    // 6x6x6 color cube (216 colors)
    for r in 0..6 {
        for g in 0..6 {
            for b in 0..6 {
                let idx = (r * 36 + g * 6 + b) * 3;
                palette[idx] = (r * 51) as u8;
                palette[idx + 1] = (g * 51) as u8;
                palette[idx + 2] = (b * 51) as u8;
            }
        }
    }
    // Remaining 40 colors: fine grayscale ramp
    for i in 0..40 {
        let idx = (216 + i) * 3;
        let gray = ((i as u16 * 255) / 39) as u8;
        palette[idx] = gray;
        palette[idx + 1] = gray;
        palette[idx + 2] = gray;
    }
    palette
}

/// Encodes frames into a compliant GIF89a file with Netscape loop extension
pub fn encode_gif(path: &Path, width: usize, height: usize, frames: &[Vec<u8>], delay_100ths: u16) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(1024 + frames.len() * (width * height / 2));

    // 1. Header & Logical Screen Descriptor
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&(width as u16).to_le_bytes());
    out.extend_from_slice(&(height as u16).to_le_bytes());
    out.push(0xF7); // Global Color Table Flag (1), 8 bits/pixel, 256 colors
    out.push(0x00); // Background color index
    out.push(0x00); // Pixel aspect ratio

    // 2. Global Color Table (768 bytes)
    let gct = build_global_color_table();
    out.extend_from_slice(&gct);

    // 3. Netscape Application Extension for infinite loop
    out.extend_from_slice(&[
        0x21, 0xFF, 0x0B, // Extension introducer, Application extension, block size (11)
        b'N', b'E', b'T', b'S', b'C', b'A', b'P', b'E', b'2', b'.', b'0',
        0x03, 0x01, 0x00, 0x00, // Sub-block length 3, loop sub-block, 0 = loop forever
        0x00, // Block terminator
    ]);

    // 4. Frames
    for frame in frames {
        // Graphic Control Extension
        out.extend_from_slice(&[
            0x21, 0xF9, 0x04, // Extension introducer, Graphic Control label, block size 4
            0x04,             // Disposal method = 1 (do not dispose)
        ]);
        out.extend_from_slice(&delay_100ths.to_le_bytes());
        out.push(0x00); // Transparent color index
        out.push(0x00); // Block terminator

        // Image Descriptor
        out.push(0x2C); // Image separator
        out.extend_from_slice(&0u16.to_le_bytes()); // Left
        out.extend_from_slice(&0u16.to_le_bytes()); // Top
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        out.push(0x00); // No local color table, non-interlaced

        // LZW compressed image data
        encode_lzw(&mut out, frame, 8);
    }

    // 5. Trailer
    out.push(0x3B);

    fs::write(path, out)
}

/// Bit-packing helper for variable-width LZW output
struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    accumulator: u32,
    bits_in_acc: u8,
    subblock: [u8; 255],
    subblock_len: usize,
}

impl<'a> BitWriter<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            accumulator: 0,
            bits_in_acc: 0,
            subblock: [0; 255],
            subblock_len: 0,
        }
    }

    fn write_bits(&mut self, code: u32, code_size: u8) {
        self.accumulator |= code << self.bits_in_acc;
        self.bits_in_acc += code_size;

        while self.bits_in_acc >= 8 {
            let byte = (self.accumulator & 0xFF) as u8;
            self.write_byte(byte);
            self.accumulator >>= 8;
            self.bits_in_acc -= 8;
        }
    }

    fn write_byte(&mut self, b: u8) {
        self.subblock[self.subblock_len] = b;
        self.subblock_len += 1;
        if self.subblock_len == 255 {
            self.flush_subblock();
        }
    }

    fn flush_subblock(&mut self) {
        if self.subblock_len > 0 {
            self.out.push(self.subblock_len as u8);
            self.out.extend_from_slice(&self.subblock[..self.subblock_len]);
            self.subblock_len = 0;
        }
    }

    fn finish(mut self) {
        if self.bits_in_acc > 0 {
            let byte = (self.accumulator & 0xFF) as u8;
            self.write_byte(byte);
        }
        self.flush_subblock();
        self.out.push(0x00); // Block terminator
    }
}

/// Encodes pixel stream using standard GIF LZW compression
fn encode_lzw(out: &mut Vec<u8>, data: &[u8], min_code_size: u8) {
    out.push(min_code_size);

    let clear_code = 1u32 << min_code_size; // 256
    let eoi_code = clear_code + 1;           // 257

    let mut writer = BitWriter::new(out);
    writer.write_bits(clear_code, min_code_size + 1);

    let mut code_size = min_code_size + 1;
    let mut next_code = eoi_code + 1;

    // Simple hash table for string matching: key = (prefix_code << 8) | next_byte -> code
    let mut dict = std::collections::HashMap::new();

    let mut prefix = if let Some(&first) = data.first() {
        first as u32
    } else {
        writer.write_bits(eoi_code, code_size);
        writer.finish();
        return;
    };

    for &byte in &data[1..] {
        let key = (prefix << 8) | (byte as u32);
        if let Some(&code) = dict.get(&key) {
            prefix = code;
        } else {
            writer.write_bits(prefix, code_size);

            if next_code < 4096 {
                dict.insert(key, next_code);
                next_code += 1;
                if next_code > (1 << code_size) && code_size < 12 {
                    code_size += 1;
                }
            } else {
                // Clear table
                writer.write_bits(clear_code, code_size);
                dict.clear();
                code_size = min_code_size + 1;
                next_code = eoi_code + 1;
            }

            prefix = byte as u32;
        }
    }

    writer.write_bits(prefix, code_size);
    writer.write_bits(eoi_code, code_size);
    writer.finish();
}
