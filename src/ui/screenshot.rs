//! Lossless BMP Screenshot Capturing Engine

use eframe::egui::ColorImage;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn generate_screenshot_path(rom_name: &str, enhanced: bool) -> (PathBuf, String) {
    let _ = fs::create_dir_all("screenshots");
    let safe_name: String = rom_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let suffix = if enhanced { "_enhanced" } else { "_raw" };
    let filename = format!("{}_{}{}.bmp", safe_name, now, suffix);
    let path = Path::new("screenshots").join(&filename);
    (path, filename)
}

/// Encodes and saves a 32-bit BMP image directly to disk.
pub fn save_bmp(path: &Path, width: usize, height: usize, rgba_pixels: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let file_size = 54 + width * height * 4;
    let mut data = Vec::with_capacity(file_size);

    // Bitmap File Header (14 bytes)
    data.extend_from_slice(b"BM");
    data.extend_from_slice(&(file_size as u32).to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes()); // Reserved
    data.extend_from_slice(&54u32.to_le_bytes()); // Pixel data offset

    // DIB Header (BITMAPINFOHEADER - 40 bytes)
    data.extend_from_slice(&40u32.to_le_bytes());
    data.extend_from_slice(&(width as i32).to_le_bytes());
    data.extend_from_slice(&(-(height as i32)).to_le_bytes()); // Top-down order
    data.extend_from_slice(&1u16.to_le_bytes()); // 1 color plane
    data.extend_from_slice(&32u16.to_le_bytes()); // 32 bits per pixel
    data.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB (uncompressed)
    data.extend_from_slice(&((width * height * 4) as u32).to_le_bytes());
    data.extend_from_slice(&2835u32.to_le_bytes()); // Horizontal resolution (72 DPI)
    data.extend_from_slice(&2835u32.to_le_bytes()); // Vertical resolution (72 DPI)
    data.extend_from_slice(&0u32.to_le_bytes()); // Colors in color table
    data.extend_from_slice(&0u32.to_le_bytes()); // Important colors

    // Pixel data in BGRA order
    for chunk in rgba_pixels.chunks_exact(4) {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];
        data.push(b);
        data.push(g);
        data.push(r);
        data.push(a);
    }

    fs::write(path, data)
}

/// Captures a raw 240x160 GBA LCD framebuffer.
pub fn save_raw_framebuffer(path: &Path, raw_fb: &[u32; 240 * 160]) -> std::io::Result<()> {
    let mut rgba = Vec::with_capacity(240 * 160 * 4);
    for &p in raw_fb {
        rgba.push((p & 0xFF) as u8);
        rgba.push(((p >> 8) & 0xFF) as u8);
        rgba.push(((p >> 16) & 0xFF) as u8);
        rgba.push(0xFF);
    }
    save_bmp(path, 240, 160, &rgba)
}

/// Captures an arbitrary-sized enhanced ColorImage (e.g. upscaled with xBRZ and sharpened with NIS).
pub fn save_color_image(path: &Path, image: &ColorImage) -> std::io::Result<()> {
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for p in &image.pixels {
        rgba.push(p.r());
        rgba.push(p.g());
        rgba.push(p.b());
        rgba.push(p.a());
    }
    save_bmp(path, width, height, &rgba)
}
