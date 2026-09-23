//! GBA PPU Windowing and Color Blending (Alpha Blending, Brightness Up/Down)

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    None = 0,
    Alpha = 1,
    BrightnessIncrease = 2,
    BrightnessDecrease = 3,
}

#[derive(Clone, Copy)]
pub struct Pixel {
    pub color: u16, // BGR555
    pub layer: u8,  // 0=BG0, 1=BG1, 2=BG2, 3=BG3, 4=OBJ, 5=Backdrop
    pub priority: u8,
    pub is_transparent: bool,
    pub is_obj_alpha: bool,
}

impl Default for Pixel {
    fn default() -> Self {
        Self {
            color: 0,
            layer: 5, // Backdrop
            priority: 4,
            is_transparent: true,
            is_obj_alpha: false,
        }
    }
}

pub fn bgr555_to_rgb888(c: u16) -> (u8, u8, u8) {
    let r5 = (c & 0x1F) as u8;
    let g5 = ((c >> 5) & 0x1F) as u8;
    let b5 = ((c >> 10) & 0x1F) as u8;

    // Expand 5-bit to 8-bit: (val * 255) / 31 == (val << 3) | (val >> 2)
    let r = (r5 << 3) | (r5 >> 2);
    let g = (g5 << 3) | (g5 >> 2);
    let b = (b5 << 3) | (b5 >> 2);
    (r, g, b)
}

pub fn rgb888_to_bgr555(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r >> 3) as u16;
    let g5 = (g >> 3) as u16;
    let b5 = (b >> 3) as u16;
    r5 | (g5 << 5) | (b5 << 10)
}

pub fn apply_color_effects(
    top: Pixel,
    bot: Pixel,
    bldcnt: u16,
    eva: u16,
    evb: u16,
    evy: u16,
) -> u16 {
    let top_layer_mask = 1 << top.layer;
    let bot_layer_mask = 1 << bot.layer;

    let target1_match = (bldcnt & top_layer_mask) != 0;
    let target2_match = (bldcnt & (bot_layer_mask << 8)) != 0;

    let mode = if top.is_obj_alpha {
        // Semi-transparent sprites always attempt alpha blend; if 2nd target fails, render normally
        if target2_match {
            BlendMode::Alpha
        } else {
            BlendMode::None
        }
    } else if target1_match {
        match (bldcnt >> 6) & 3 {
            1 => {
                if target2_match {
                    BlendMode::Alpha
                } else {
                    BlendMode::None
                }
            }
            2 => BlendMode::BrightnessIncrease,
            3 => BlendMode::BrightnessDecrease,
            _ => BlendMode::None,
        }
    } else {
        BlendMode::None
    };

    // Hardware blends each 5-bit channel directly (GBATEK "BLDALPHA" /
    // "BLDY"): alpha = min(31, (a*EVA + b*EVB) >> 4), brighten =
    // a + ((31-a)*EVY >> 4), darken = a - (a*EVY >> 4), with coefficients
    // capped at 16. Working in 8-bit space and truncating back (as this used
    // to) drifts by one step on some colors (ROADMAP M1).
    let channels = |c: u16| [c & 0x1F, (c >> 5) & 0x1F, (c >> 10) & 0x1F];
    let pack = |ch: [u16; 3]| ch[0] | (ch[1] << 5) | (ch[2] << 10);
    let t = channels(top.color);
    match mode {
        BlendMode::Alpha => {
            let (ca, cb) = (eva.min(16), evb.min(16));
            let b = channels(bot.color);
            pack([0, 1, 2].map(|i| ((t[i] * ca + b[i] * cb) >> 4).min(31)))
        }
        BlendMode::BrightnessIncrease => {
            let ey = evy.min(16);
            pack(t.map(|a| a + (((31 - a) * ey) >> 4)))
        }
        BlendMode::BrightnessDecrease => {
            let ey = evy.min(16);
            pack(t.map(|a| a - ((a * ey) >> 4)))
        }
        BlendMode::None => top.color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(color: u16, layer: u8) -> Pixel {
        Pixel { color, layer, priority: 0, is_transparent: false, is_obj_alpha: false }
    }

    #[test]
    fn alpha_blend_is_5bit_exact() {
        // BG0 over BG1, EVA = EVB = 8: (31*8 + 0*8) >> 4 = 15 per channel.
        let bldcnt = 0x0001 | (1 << 6) | (0x0002 << 8);
        assert_eq!(apply_color_effects(px(0x7FFF, 0), px(0, 1), bldcnt, 8, 8, 0), 0x3DEF);
        // Saturates at 31.
        assert_eq!(apply_color_effects(px(0x7FFF, 0), px(0x7FFF, 1), bldcnt, 16, 16, 0), 0x7FFF);
    }

    #[test]
    fn brightness_is_5bit_exact() {
        let up = 0x0001 | (2 << 6);
        let down = 0x0001 | (3 << 6);
        // EVY 8: R 1 -> 1 + (30*8 >> 4) = 16; G/B 0 -> 0 + (31*8 >> 4) = 15.
        // Darken: 1 - (1*8 >> 4) = 1.
        assert_eq!(apply_color_effects(px(1, 0), px(0, 1), up, 0, 0, 8), 16 | (15 << 5) | (15 << 10));
        assert_eq!(apply_color_effects(px(1, 0), px(0, 1), down, 0, 0, 8), 1);
        assert_eq!(apply_color_effects(px(0x7FFF, 0), px(0, 1), down, 0, 0, 16), 0);
    }
}
