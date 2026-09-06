//! GBA LCD Viewport Rendering and Post-Processing Filters

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

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

            let tex_options = TextureOptions::LINEAR;
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
                for i in 0..240 * 160 {
                    let pixel = raw_fb[i];
                    let r = (pixel & 0xFF) as u8;
                    let g = ((pixel >> 8) & 0xFF) as u8;
                    let b = ((pixel >> 16) & 0xFF) as u8;

                    let (final_r, final_g, final_b) = if color_correction {
                        apply_gba_color_correction(r, g, b)
                    } else {
                        (r, g, b)
                    };

                    self.image_buffer.pixels[i] = Color32::from_rgb(final_r, final_g, final_b);
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
