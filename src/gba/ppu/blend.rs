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
    let (tr, tg, tb) = bgr555_to_rgb888(top.color);
    let (br, bg, bb) = bgr555_to_rgb888(bot.color);

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

    match mode {
        BlendMode::Alpha => {
            let ca = eva.min(16) as u32;
            let cb = evb.min(16) as u32;

            let r = ((tr as u32 * ca + br as u32 * cb) / 16).min(255) as u8;
            let g = ((tg as u32 * ca + bg as u32 * cb) / 16).min(255) as u8;
            let b = ((tb as u32 * ca + bb as u32 * cb) / 16).min(255) as u8;
            rgb888_to_bgr555(r, g, b)
        }
        BlendMode::BrightnessIncrease => {
            let ey = evy.min(16) as u32;
            let r = (tr as u32 + (255 - tr as u32) * ey / 16).min(255) as u8;
            let g = (tg as u32 + (255 - tg as u32) * ey / 16).min(255) as u8;
            let b = (tb as u32 + (255 - tb as u32) * ey / 16).min(255) as u8;
            rgb888_to_bgr555(r, g, b)
        }
        BlendMode::BrightnessDecrease => {
            let ey = evy.min(16) as u32;
            let r = (tr as u32).saturating_sub((tr as u32 * ey) / 16) as u8;
            let g = (tg as u32).saturating_sub((tg as u32 * ey) / 16) as u8;
            let b = (tb as u32).saturating_sub((tb as u32 * ey) / 16) as u8;
            rgb888_to_bgr555(r, g, b)
        }
        BlendMode::None => top.color,
    }
}
