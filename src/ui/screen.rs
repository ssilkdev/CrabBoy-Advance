//! GBA LCD Viewport Rendering, Post-Processing Filters, and Aspect Ratio Management

use crate::gba::{SCREEN_HEIGHT, SCREEN_WIDTH};
use egui::{Color32, ColorImage, TextureHandle, TextureOptions, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayFilter {
    Crisp,
    NvidiaSharpen,
    Xbrz,
    Linear,
    LcdGrid,
    CrtScanlines,
}

impl DisplayFilter {
    #[allow(non_upper_case_globals)]
    pub const Nearest: Self = Self::Crisp;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    Scale1x,
    Scale2x,
    Scale3x,
    Scale4x,
    Scale5x,
    Scale6x,
    Scale8x,
    Scale10x,
    Scale12x,
    Scale14x,
    IntegerAuto,
    Fit,
}

/// Selectable display aspect ratios for GBA emulation viewport
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatio {
    Native,          // 3:2 (Exact GBA hardware 240x160)
    ClassicTv,       // 4:3 (Standard CRT / retro broadcast)
    Square,          // 1:1 (Square pixel viewport)
    GameBoyOriginal, // 10:9 (Classic Game Boy DMG/CGB 160x144)
    Widescreen16_9,  // 16:9 (Modern HDTV / widescreen)
    Widescreen16_10, // 16:10 (Steam Deck / PC laptops)
    Ultrawide21_9,   // 21:9 (Cinematic ultrawide)
    Stretch,         // Stretch to fill window without aspect constraint
}

impl Default for AspectRatio {
    fn default() -> Self {
        Self::Native
    }
}

impl AspectRatio {
    pub const ALL: [AspectRatio; 8] = [
        AspectRatio::Native,
        AspectRatio::ClassicTv,
        AspectRatio::Square,
        AspectRatio::GameBoyOriginal,
        AspectRatio::Widescreen16_9,
        AspectRatio::Widescreen16_10,
        AspectRatio::Ultrawide21_9,
        AspectRatio::Stretch,
    ];

    /// Numerical width-to-height ratio (or None for stretch)
    #[inline]
    pub fn ratio(&self) -> Option<f32> {
        match self {
            Self::Native => Some(3.0 / 2.0),
            Self::ClassicTv => Some(4.0 / 3.0),
            Self::Square => Some(1.0 / 1.0),
            Self::GameBoyOriginal => Some(10.0 / 9.0),
            Self::Widescreen16_9 => Some(16.0 / 9.0),
            Self::Widescreen16_10 => Some(16.0 / 10.0),
            Self::Ultrawide21_9 => Some(21.0 / 9.0),
            Self::Stretch => None,
        }
    }

    /// User-friendly label for menus and tooltips
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Native => "Native (3:2) - GBA Original",
            Self::ClassicTv => "Classic CRT / TV (4:3)",
            Self::Square => "Square (1:1)",
            Self::GameBoyOriginal => "Classic Game Boy (10:9)",
            Self::Widescreen16_9 => "Widescreen (16:9)",
            Self::Widescreen16_10 => "Widescreen (16:10) - Steam Deck",
            Self::Ultrawide21_9 => "Ultrawide (21:9)",
            Self::Stretch => "Stretch to Window (Fill)",
        }
    }

    /// Compact badge text for status bar
    pub fn badge_text(&self) -> &'static str {
        match self {
            Self::Native => "[3:2]",
            Self::ClassicTv => "[4:3]",
            Self::Square => "[1:1]",
            Self::GameBoyOriginal => "[10:9]",
            Self::Widescreen16_9 => "[16:9]",
            Self::Widescreen16_10 => "[16:10]",
            Self::Ultrawide21_9 => "[21:9]",
            Self::Stretch => "[STRETCH]",
        }
    }

    /// Cycle to the next aspect ratio in sequence
    pub fn cycle(&self) -> Self {
        match self {
            Self::Native => Self::ClassicTv,
            Self::ClassicTv => Self::Square,
            Self::Square => Self::GameBoyOriginal,
            Self::GameBoyOriginal => Self::Widescreen16_9,
            Self::Widescreen16_9 => Self::Widescreen16_10,
            Self::Widescreen16_10 => Self::Ultrawide21_9,
            Self::Ultrawide21_9 => Self::Stretch,
            Self::Stretch => Self::Native,
        }
    }

    /// Calculates the target viewport size in points given the available area and scale mode.
    pub fn calculate_target_size(&self, available_size: Vec2, scale_mode: ScaleMode) -> Vec2 {
        let native_w = SCREEN_WIDTH as f32;
        let native_h = SCREEN_HEIGHT as f32;
        let native_aspect = native_w / native_h; // 1.5

        match scale_mode {
            ScaleMode::Fit => {
                match self.ratio() {
                    Some(aspect) => {
                        let w_by_h = available_size.y * aspect;
                        if w_by_h <= available_size.x {
                            Vec2::new(w_by_h, available_size.y)
                        } else {
                            Vec2::new(available_size.x, available_size.x / aspect)
                        }
                    }
                    None => available_size, // Stretch mode fills entire available area
                }
            }
            ScaleMode::IntegerAuto => {
                let aspect = self.ratio().unwrap_or(native_aspect);
                let max_h = (available_size.y / native_h).floor() as u32;
                let max_w = (available_size.x / (native_h * aspect)).floor() as u32;
                let scale = max_w.min(max_h).max(1);
                let target_h = native_h * scale as f32;
                let target_w = target_h * aspect;
                Vec2::new(target_w, target_h)
            }
            fixed_scale => {
                let factor = match fixed_scale {
                    ScaleMode::Scale1x => 1.0,
                    ScaleMode::Scale2x => 2.0,
                    ScaleMode::Scale3x => 3.0,
                    ScaleMode::Scale4x => 4.0,
                    ScaleMode::Scale5x => 5.0,
                    ScaleMode::Scale6x => 6.0,
                    ScaleMode::Scale8x => 8.0,
                    ScaleMode::Scale10x => 10.0,
                    ScaleMode::Scale12x => 12.0,
                    ScaleMode::Scale14x => 14.0,
                    _ => 1.0,
                };
                let target_h = native_h * factor;
                let aspect = self.ratio().unwrap_or(native_aspect);
                let target_w = target_h * aspect;
                Vec2::new(target_w, target_h)
            }
        }
    }
}

pub struct ScreenRenderer {
    texture: Option<TextureHandle>,
    image_buffer: ColorImage,
}

impl Default for ScreenRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenRenderer {
    pub fn new() -> Self {
        Self {
            texture: None,
            image_buffer: ColorImage::new([240, 160], Color32::BLACK),
        }
    }

    pub fn last_image(&self) -> &ColorImage {
        &self.image_buffer
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_framebuffer(
        &mut self,
        ctx: &egui::Context,
        raw_fb: &[u32; 240 * 160],
        filter: DisplayFilter,
        nvidia_sharpen: bool,
        nvidia_sharpness: f32,
        color_correction: bool,
        xbrz_factor: usize,
    ) -> &TextureHandle {
        let should_sharpen = nvidia_sharpen || filter == DisplayFilter::NvidiaSharpen;

        if filter == DisplayFilter::Xbrz {
            let factor = xbrz_factor.clamp(2, 6);
            let target_w = 240 * factor;
            let target_h = 160 * factor;

            let mut src_rgba = Vec::with_capacity(240 * 160 * 4);
            for &pixel in raw_fb {
                let mut r = (pixel & 0xFF) as u8;
                let mut g = ((pixel >> 8) & 0xFF) as u8;
                let mut b = ((pixel >> 16) & 0xFF) as u8;
                if color_correction {
                    let (cr, cg, cb) = apply_gba_color_correction(r, g, b);
                    r = cr;
                    g = cg;
                    b = cb;
                }
                src_rgba.push(r);
                src_rgba.push(g);
                src_rgba.push(b);
                src_rgba.push(0xFF);
            }

            let scaled_bytes = xbrz::scale_rgba(&src_rgba, 240, 160, factor);
            let mut scaled_image = ColorImage::from_rgba_unmultiplied([target_w, target_h], &scaled_bytes);

            if should_sharpen {
                apply_nvidia_adaptive_sharpening_image(&mut scaled_image, nvidia_sharpness);
            }

            let tex_options = TextureOptions::NEAREST;
            let tex = self.texture.get_or_insert_with(|| {
                ctx.load_texture("gba_screen", scaled_image.clone(), tex_options)
            });

            self.image_buffer = scaled_image.clone();
            tex.set(scaled_image, tex_options);
            return tex;
        }

        if self.image_buffer.size != [240, 160] {
            self.image_buffer = ColorImage::new([240, 160], Color32::BLACK);
        }

        match filter {
            DisplayFilter::LcdGrid => {
                for y in 0..160 {
                    let is_grid_y = y % 2 == 1;
                    for x in 0..240 {
                        let is_grid_x = x % 2 == 1;
                        let pixel = raw_fb[y * 240 + x];
                        let mut r = (pixel & 0xFF) as u8;
                        let mut g = ((pixel >> 8) & 0xFF) as u8;
                        let mut b = ((pixel >> 16) & 0xFF) as u8;

                        if color_correction {
                            let (cr, cg, cb) = apply_gba_color_correction(r, g, b);
                            r = cr;
                            g = cg;
                            b = cb;
                        }

                        if is_grid_x || is_grid_y {
                            r = (r as u16 * 220 / 256) as u8;
                            g = (g as u16 * 220 / 256) as u8;
                            b = (b as u16 * 220 / 256) as u8;
                        }

                        self.image_buffer.pixels[y * 240 + x] = Color32::from_rgb(r, g, b);
                    }
                }
            }
            DisplayFilter::CrtScanlines => {
                for y in 0..160 {
                    let is_scanline = y % 2 == 1;
                    for x in 0..240 {
                        let pixel = raw_fb[y * 240 + x];
                        let mut r = (pixel & 0xFF) as u8;
                        let mut g = ((pixel >> 8) & 0xFF) as u8;
                        let mut b = ((pixel >> 16) & 0xFF) as u8;

                        if color_correction {
                            let (cr, cg, cb) = apply_gba_color_correction(r, g, b);
                            r = cr;
                            g = cg;
                            b = cb;
                        }

                        if is_scanline {
                            r = (r as u16 * 190 / 256) as u8;
                            g = (g as u16 * 190 / 256) as u8;
                            b = (b as u16 * 190 / 256) as u8;
                        }

                        self.image_buffer.pixels[y * 240 + x] = Color32::from_rgb(r, g, b);
                    }
                }
            }
            _ => {
                // Crisp / Nearest & Linear & NvidiaSharpen direct pixel copy
                for (dst, &pixel) in self.image_buffer.pixels.iter_mut().zip(raw_fb.iter()) {
                    let r = (pixel & 0xFF) as u8;
                    let g = ((pixel >> 8) & 0xFF) as u8;
                    let b = ((pixel >> 16) & 0xFF) as u8;

                    let (final_r, final_g, final_b) = if color_correction {
                        apply_gba_color_correction(r, g, b)
                    } else {
                        (r, g, b)
                    };

                    *dst = Color32::from_rgb(final_r, final_g, final_b);
                }
            }
        }

        if should_sharpen {
            apply_nvidia_adaptive_sharpening_image(&mut self.image_buffer, nvidia_sharpness);
        }

        let tex_options = match filter {
            DisplayFilter::Linear => TextureOptions::LINEAR,
            _ => TextureOptions::NEAREST,
        };

        let tex = self.texture.get_or_insert_with(|| {
            ctx.load_texture("gba_screen", self.image_buffer.clone(), tex_options)
        });

        tex.set(self.image_buffer.clone(), tex_options);
        tex
    }
}

/// Applies NVIDIA-style Contrast-Adaptive Sharpening (NIS / CAS) to an arbitrary-sized ColorImage.
pub fn apply_nvidia_adaptive_sharpening_image(
    image: &mut ColorImage,
    sharpness: f32,
) {
    let s = sharpness.clamp(0.0, 1.0);
    if s <= 0.001 {
        return;
    }

    let [width, height] = image.size;
    if width < 2 || height < 2 {
        return;
    }

    let src = image.pixels.clone();

    for y in 0..height {
        let y_top = if y > 0 { y - 1 } else { y };
        let y_bot = if y < height - 1 { y + 1 } else { y };

        for x in 0..width {
            let x_left = if x > 0 { x - 1 } else { x };
            let x_right = if x < width - 1 { x + 1 } else { x };

            let c = src[y * width + x];
            let n = src[y_top * width + x];
            let s_pix = src[y_bot * width + x];
            let w = src[y * width + x_left];
            let e = src[y * width + x_right];

            let (cr, cg, cb) = (c.r() as f32, c.g() as f32, c.b() as f32);
            let (nr, ng, nb) = (n.r() as f32, n.g() as f32, n.b() as f32);
            let (sr, sg, sb) = (s_pix.r() as f32, s_pix.g() as f32, s_pix.b() as f32);
            let (wr, wg, wb) = (w.r() as f32, w.g() as f32, w.b() as f32);
            let (er, eg, eb) = (e.r() as f32, e.g() as f32, e.b() as f32);

            let sharpen_channel = |c: f32, n: f32, s_val: f32, w: f32, e: f32| -> u8 {
                let min_val = c.min(n).min(s_val).min(w).min(e);
                let max_val = c.max(n).max(s_val).max(w).max(e);
                let delta = max_val - min_val;

                if delta < 1.0 {
                    return c.round().clamp(0.0, 255.0) as u8;
                }

                // NVIDIA CAS-style adaptive weight
                let amp = ((min_val.min(255.0 - max_val)) / (delta + 1.0)).sqrt();
                let weight = -s * 0.25 * amp;
                let sum_neighbors = n + s_val + w + e;
                let sharp = (c + weight * sum_neighbors) / (1.0 + 4.0 * weight);

                sharp.clamp(min_val, max_val).clamp(0.0, 255.0).round() as u8
            };

            let r = sharpen_channel(cr, nr, sr, wr, er);
            let g = sharpen_channel(cg, ng, sg, wg, eg);
            let b = sharpen_channel(cb, nb, sb, wb, eb);

            image.pixels[y * width + x] = Color32::from_rgb(r, g, b);
        }
    }
}

/// Applies NVIDIA-style Contrast-Adaptive Sharpening (NIS / CAS)
/// Detects edge gradients and enhances high frequencies without halo artifacts.
pub fn apply_nvidia_adaptive_sharpening(
    raw_fb: &[u32; 240 * 160],
    image_buffer: &mut ColorImage,
    sharpness: f32,
) {
    let s = sharpness.clamp(0.0, 1.0);

    for y in 0..160 {
        let y_top = if y > 0 { y - 1 } else { y };
        let y_bot = if y < 159 { y + 1 } else { y };

        for x in 0..240 {
            let x_left = if x > 0 { x - 1 } else { x };
            let x_right = if x < 239 { x + 1 } else { x };

            let c = raw_fb[y * 240 + x];
            let n = raw_fb[y_top * 240 + x];
            let s_pix = raw_fb[y_bot * 240 + x];
            let w = raw_fb[y * 240 + x_left];
            let e = raw_fb[y * 240 + x_right];

            let (cr, cg, cb) = ((c & 0xFF) as f32, ((c >> 8) & 0xFF) as f32, ((c >> 16) & 0xFF) as f32);
            let (nr, ng, nb) = ((n & 0xFF) as f32, ((n >> 8) & 0xFF) as f32, ((n >> 16) & 0xFF) as f32);
            let (sr, sg, sb) = ((s_pix & 0xFF) as f32, ((s_pix >> 8) & 0xFF) as f32, ((s_pix >> 16) & 0xFF) as f32);
            let (wr, wg, wb) = ((w & 0xFF) as f32, ((w >> 8) & 0xFF) as f32, ((w >> 16) & 0xFF) as f32);
            let (er, eg, eb) = ((e & 0xFF) as f32, ((e >> 8) & 0xFF) as f32, ((e >> 16) & 0xFF) as f32);

            let sharpen_channel = |c: f32, n: f32, s_val: f32, w: f32, e: f32| -> u8 {
                let min_val = c.min(n).min(s_val).min(w).min(e);
                let max_val = c.max(n).max(s_val).max(w).max(e);
                let delta = max_val - min_val;

                if delta < 1.0 || s <= 0.001 {
                    return c.round().clamp(0.0, 255.0) as u8;
                }

                // NVIDIA CAS-style adaptive weight
                let amp = ((min_val.min(255.0 - max_val)) / (delta + 1.0)).sqrt();
                let weight = -s * 0.25 * amp;
                let sum_neighbors = n + s_val + w + e;
                let sharp = (c + weight * sum_neighbors) / (1.0 + 4.0 * weight);

                sharp.clamp(min_val, max_val).clamp(0.0, 255.0).round() as u8
            };

            let r = sharpen_channel(cr, nr, sr, wr, er);
            let g = sharpen_channel(cg, ng, sg, wg, eg);
            let b = sharpen_channel(cb, nb, sb, wb, eb);

            image_buffer.pixels[y * 240 + x] = Color32::from_rgb(r, g, b);
        }
    }
}

/// Applies authentic GBA LCD color and gamma correction to rebalance hyper-saturated palettes for modern displays.
#[inline]
pub fn apply_gba_color_correction(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let rf = r as f32;
    let gf = g as f32;
    let bf = b as f32;

    // Spectral response matrix for authentic GBA LCD reproduction
    let cr = (0.84 * rf + 0.08 * gf + 0.08 * bf).min(255.0);
    let cg = (0.06 * rf + 0.88 * gf + 0.06 * bf).min(255.0);
    let cb = (0.08 * rf + 0.08 * gf + 0.84 * bf).min(255.0);

    // Subtle gamma adaptation (0.92) to match modern sRGB/DCI-P3 gamma curve
    let corr_r = (255.0 * (cr / 255.0).powf(0.92)).clamp(0.0, 255.0).round() as u8;
    let corr_g = (255.0 * (cg / 255.0).powf(0.92)).clamp(0.0, 255.0).round() as u8;
    let corr_b = (255.0 * (cb / 255.0).powf(0.92)).clamp(0.0, 255.0).round() as u8;

    (corr_r, corr_g, corr_b)
}
