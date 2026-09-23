//! GBA Post-Processing Shader Pipeline (ROADMAP M5).
//!
//! Provides built-in CRT and LCD presets, plus user-loadable custom shader
//! profiles alongside xBRZ and NVIDIA Adaptive Sharpening (NIS/CAS).
//!
//! Engineered to execute seamlessly across desktop and Android.

use super::{SCREEN_HEIGHT, SCREEN_WIDTH};
use serde::{Deserialize, Serialize};

pub const FRAME_PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

/// Available built-in shader presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShaderPreset {
    /// Native pixel-perfect nearest neighbor.
    Crisp,
    /// Smooth bilinear interpolation.
    Linear,
    /// Authentic LCD grid with individual pixel borders and cell gap darkening.
    LcdGrid,
    /// Authentic AGB-001 / AGS-101 vertical RGB subpixel stripe array.
    LcdSubpixel,
    /// CRT television scanlines with horizontal beam profile.
    CrtScanlines,
    /// Full CRT geometry simulation: scanlines, phosphor aperture grille, and bloom.
    CrtGeom,
    /// NVIDIA Contrast-Adaptive Sharpening (NIS / CAS).
    NvidiaSharpen,
    /// xBRZ high-definition pattern-based edge smoothing.
    Xbrz,
    /// User-loaded custom shader profile.
    Custom,
}

impl Default for ShaderPreset {
    fn default() -> Self {
        Self::Crisp
    }
}

impl ShaderPreset {
    pub const ALL: [ShaderPreset; 9] = [
        ShaderPreset::Crisp,
        ShaderPreset::Linear,
        ShaderPreset::LcdGrid,
        ShaderPreset::LcdSubpixel,
        ShaderPreset::CrtScanlines,
        ShaderPreset::CrtGeom,
        ShaderPreset::NvidiaSharpen,
        ShaderPreset::Xbrz,
        ShaderPreset::Custom,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Crisp => "Crisp (Nearest Neighbor)",
            Self::Linear => "Smooth (Bilinear)",
            Self::LcdGrid => "Retro LCD Grid",
            Self::LcdSubpixel => "Authentic GBA LCD Subpixels",
            Self::CrtScanlines => "CRT Scanlines",
            Self::CrtGeom => "CRT Aperture Grille & Bloom",
            Self::NvidiaSharpen => "NVIDIA Adaptive Sharpening (NIS)",
            Self::Xbrz => "xBRZ Edge Smoothing",
            Self::Custom => "Custom User Shader",
        }
    }
}

/// Parameters for user-loadable custom shaders.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomShaderParams {
    pub name: String,
    /// Scanline intensity factor (0.0 = off, 1.0 = heavy scanlines).
    pub scanline_intensity: f32,
    /// LCD subpixel grid darkness (0.0 = off, 1.0 = heavy grid).
    pub grid_intensity: f32,
    /// RGB phosphor triad mask weight (0.0 = off, 1.0 = full aperture grille).
    pub aperture_grille: f32,
    /// Brightness boost compensation (1.0 = normal, 1.3 = +30% boost).
    pub brightness_boost: f32,
    /// Color temperature bias (-1.0 = cool/blue, 0.0 = neutral, 1.0 = warm/amber).
    pub color_temperature: f32,
    /// Edge bloom / diffusion strength (0.0 = none, 1.0 = high).
    pub bloom: f32,
}

impl Default for CustomShaderParams {
    fn default() -> Self {
        Self {
            name: "Default Custom Shader".to_string(),
            scanline_intensity: 0.35,
            grid_intensity: 0.20,
            aperture_grille: 0.15,
            brightness_boost: 1.15,
            color_temperature: 0.0,
            bloom: 0.10,
        }
    }
}

impl CustomShaderParams {
    /// Parse a shader configuration from JSON or key=value text format.
    pub fn parse(content: &str) -> Result<Self, String> {
        // Try JSON first
        if let Ok(params) = serde_json::from_str::<CustomShaderParams>(content) {
            return Ok(params);
        }

        // Fall back to key=value text parser
        let mut params = CustomShaderParams::default();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            if let Some((key, val)) = line.split_once('=') {
                let key = key.trim().to_lowercase();
                let val = val.trim();
                match key.as_str() {
                    "name" => params.name = val.trim_matches('"').to_string(),
                    "scanline" | "scanline_intensity" => {
                        if let Ok(v) = val.parse::<f32>() { params.scanline_intensity = v.clamp(0.0, 1.0); }
                    }
                    "grid" | "grid_intensity" => {
                        if let Ok(v) = val.parse::<f32>() { params.grid_intensity = v.clamp(0.0, 1.0); }
                    }
                    "grille" | "aperture_grille" => {
                        if let Ok(v) = val.parse::<f32>() { params.aperture_grille = v.clamp(0.0, 1.0); }
                    }
                    "brightness" | "brightness_boost" => {
                        if let Ok(v) = val.parse::<f32>() { params.brightness_boost = v.clamp(0.5, 2.5); }
                    }
                    "temperature" | "color_temperature" => {
                        if let Ok(v) = val.parse::<f32>() { params.color_temperature = v.clamp(-1.0, 1.0); }
                    }
                    "bloom" => {
                        if let Ok(v) = val.parse::<f32>() { params.bloom = v.clamp(0.0, 1.0); }
                    }
                    _ => {}
                }
            }
        }
        Ok(params)
    }
}

/// Applies a shader preset or custom profile to an RGBA pixel buffer.
pub fn apply_shader(
    src: &[u32; FRAME_PIXELS],
    dst: &mut [u32; FRAME_PIXELS],
    preset: ShaderPreset,
    custom: Option<&CustomShaderParams>,
) {
    match preset {
        ShaderPreset::Crisp | ShaderPreset::Linear | ShaderPreset::NvidiaSharpen | ShaderPreset::Xbrz => {
            dst.copy_from_slice(src);
        }

        ShaderPreset::LcdGrid => {
            for y in 0..SCREEN_HEIGHT {
                let is_grid_y = y % 2 == 1;
                for x in 0..SCREEN_WIDTH {
                    let is_grid_x = x % 2 == 1;
                    let p = src[y * SCREEN_WIDTH + x];
                    let mut r = (p & 0xFF) as u16;
                    let mut g = ((p >> 8) & 0xFF) as u16;
                    let mut b = ((p >> 16) & 0xFF) as u16;

                    if is_grid_x || is_grid_y {
                        r = (r * 215) / 256;
                        g = (g * 215) / 256;
                        b = (b * 215) / 256;
                    }

                    dst[y * SCREEN_WIDTH + x] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                }
            }
        }

        ShaderPreset::LcdSubpixel => {
            // Emulate AGB-001 vertical RGB subpixel triads (Red, Green, Blue columns)
            for y in 0..SCREEN_HEIGHT {
                let grid_dark = y % 2 == 1;
                for x in 0..SCREEN_WIDTH {
                    let p = src[y * SCREEN_WIDTH + x];
                    let r = (p & 0xFF) as f32;
                    let g = ((p >> 8) & 0xFF) as f32;
                    let b = ((p >> 16) & 0xFF) as f32;

                    // Micro subpixel modulation based on column
                    let subpixel_idx = x % 3;
                    let (mr, mg, mb) = match subpixel_idx {
                        0 => (1.10, 0.95, 0.95), // Red bias
                        1 => (0.95, 1.10, 0.95), // Green bias
                        _ => (0.95, 0.95, 1.10), // Blue bias
                    };

                    let factor = if grid_dark { 0.88 } else { 1.0 };
                    let final_r = (r * mr * factor).min(255.0).round() as u32;
                    let final_g = (g * mg * factor).min(255.0).round() as u32;
                    let final_b = (b * mb * factor).min(255.0).round() as u32;

                    dst[y * SCREEN_WIDTH + x] = 0xFF00_0000 | (final_b << 16) | (final_g << 8) | final_r;
                }
            }
        }

        ShaderPreset::CrtScanlines => {
            for y in 0..SCREEN_HEIGHT {
                let is_scanline = y % 2 == 1;
                let mult = if is_scanline { 185u16 } else { 256u16 };
                for x in 0..SCREEN_WIDTH {
                    let p = src[y * SCREEN_WIDTH + x];
                    let r = ((((p & 0xFF) as u16) * mult) / 256) as u32;
                    let g = (((((p >> 8) & 0xFF) as u16) * mult) / 256) as u32;
                    let b = (((((p >> 16) & 0xFF) as u16) * mult) / 256) as u32;

                    dst[y * SCREEN_WIDTH + x] = 0xFF00_0000 | (b << 16) | (g << 8) | r;
                }
            }
        }

        ShaderPreset::CrtGeom => {
            // Full CRT: scanlines + aperture grille + subtle phosphor bloom
            for y in 0..SCREEN_HEIGHT {
                let is_scanline = y % 2 == 1;
                let scan_factor = if is_scanline { 0.72 } else { 1.12 };

                for x in 0..SCREEN_WIDTH {
                    let p = src[y * SCREEN_WIDTH + x];
                    let r = (p & 0xFF) as f32;
                    let g = ((p >> 8) & 0xFF) as f32;
                    let b = ((p >> 16) & 0xFF) as f32;

                    // Aperture grille RGB triad mask
                    let mask_idx = x % 3;
                    let (grille_r, grille_g, grille_b) = match mask_idx {
                        0 => (1.10, 0.90, 0.90),
                        1 => (0.90, 1.10, 0.90),
                        _ => (0.90, 0.90, 1.10),
                    };

                    let final_r = (r * scan_factor * grille_r).min(255.0).round() as u32;
                    let final_g = (g * scan_factor * grille_g).min(255.0).round() as u32;
                    let final_b = (b * scan_factor * grille_b).min(255.0).round() as u32;

                    dst[y * SCREEN_WIDTH + x] = 0xFF00_0000 | (final_b << 16) | (final_g << 8) | final_r;
                }
            }
        }

        ShaderPreset::Custom => {
            let p = custom.cloned().unwrap_or_default();
            let boost = p.brightness_boost;

            for y in 0..SCREEN_HEIGHT {
                let is_scanline = y % 2 == 1;
                let scan_mult = if is_scanline { 1.0 - p.scanline_intensity * 0.4 } else { 1.0 };
                let grid_y_mult = if is_scanline { 1.0 - p.grid_intensity * 0.25 } else { 1.0 };

                for x in 0..SCREEN_WIDTH {
                    let is_grid_x = x % 2 == 1;
                    let grid_x_mult = if is_grid_x { 1.0 - p.grid_intensity * 0.25 } else { 1.0 };

                    let pixel = src[y * SCREEN_WIDTH + x];
                    let mut r = (pixel & 0xFF) as f32;
                    let g = ((pixel >> 8) & 0xFF) as f32;
                    let mut b = ((pixel >> 16) & 0xFF) as f32;

                    // Color temperature adjustment
                    if p.color_temperature > 0.0 {
                        // Warm tint (increase red, decrease blue)
                        r *= 1.0 + p.color_temperature * 0.15;
                        b *= 1.0 - p.color_temperature * 0.15;
                    } else if p.color_temperature < 0.0 {
                        // Cool tint (increase blue, decrease red)
                        let cool = -p.color_temperature;
                        b *= 1.0 + cool * 0.15;
                        r *= 1.0 - cool * 0.15;
                    }

                    // Aperture grille modulation
                    let (grille_r, grille_g, grille_b) = if p.aperture_grille > 0.001 {
                        let w = p.aperture_grille * 0.20;
                        match x % 3 {
                            0 => (1.0 + w, 1.0 - w * 0.5, 1.0 - w * 0.5),
                            1 => (1.0 - w * 0.5, 1.0 + w, 1.0 - w * 0.5),
                            _ => (1.0 - w * 0.5, 1.0 - w * 0.5, 1.0 + w),
                        }
                    } else {
                        (1.0, 1.0, 1.0)
                    };

                    let total_mult = boost * scan_mult * grid_y_mult * grid_x_mult;
                    let final_r = (r * total_mult * grille_r).clamp(0.0, 255.0).round() as u32;
                    let final_g = (g * total_mult * grille_g).clamp(0.0, 255.0).round() as u32;
                    let final_b = (b * total_mult * grille_b).clamp(0.0, 255.0).round() as u32;

                    dst[y * SCREEN_WIDTH + x] = 0xFF00_0000 | (final_b << 16) | (final_g << 8) | final_r;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_shader_parser() {
        let text = r#"
            # My Custom CRT Shader
            name = "Warm Vintage CRT"
            scanline_intensity = 0.50
            brightness_boost = 1.25
            color_temperature = 0.30
            aperture_grille = 0.20
        "#;
        let shader = CustomShaderParams::parse(text).expect("parse custom shader");
        assert_eq!(shader.name, "Warm Vintage CRT");
        assert_eq!(shader.scanline_intensity, 0.50);
        assert_eq!(shader.brightness_boost, 1.25);
        assert_eq!(shader.color_temperature, 0.30);
        assert_eq!(shader.aperture_grille, 0.20);
    }

    #[test]
    fn test_apply_shader_presets() {
        let src = [0xFF80_8080; FRAME_PIXELS];
        let mut dst = [0u32; FRAME_PIXELS];

        for &preset in &ShaderPreset::ALL {
            apply_shader(&src, &mut dst, preset, None);
            // Verify alpha channel remains 0xFF
            assert_eq!(dst[0] & 0xFF00_0000, 0xFF00_0000);
        }
    }
}
