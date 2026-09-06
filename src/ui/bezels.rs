//! Retro GBA Console Bezels & Handheld Overlays

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BezelMode {
    None,
    GbaClassicIndigo,
    GbaClassicGlacier,
    GbaSpFlameRed,
    GameBoyPlayer,
}

impl Default for BezelMode {
    fn default() -> Self {
        Self::None
    }
}

pub struct BezelRenderer {
    pub mode: BezelMode,
}

impl Default for BezelRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl BezelRenderer {
    pub fn new() -> Self {
        Self {
            mode: BezelMode::None,
        }
    }

    /// Paints the retro handheld bezel surrounding the game screen viewport
    pub fn render_bezel(&self, ui: &mut Ui, screen_rect: Rect, keys_pressed: &[bool; 10]) {
        if self.mode == BezelMode::None {
            return;
        }

        let painter = ui.painter();

        // Calculate shell casing dimensions surrounding the screen
        let shell_margin_x = (screen_rect.width() * 0.18).clamp(60.0, 160.0);
        let shell_margin_y = (screen_rect.height() * 0.22).clamp(50.0, 140.0);

        let shell_rect = Rect::from_min_max(
            Pos2::new(screen_rect.min.x - shell_margin_x, screen_rect.min.y - shell_margin_y),
            Pos2::new(screen_rect.max.x + shell_margin_x, screen_rect.max.y + shell_margin_y),
        );

        let (casing_color, rim_color, accent_color) = match self.mode {
            BezelMode::GbaClassicIndigo => (
                Color32::from_rgb(75, 60, 135), // Authentic Indigo Purple
                Color32::from_rgb(95, 80, 160),
                Color32::from_rgb(180, 170, 210),
            ),
            BezelMode::GbaClassicGlacier => (
                Color32::from_rgba_premultiplied(70, 110, 140, 230), // Glacier Ice Blue
                Color32::from_rgb(110, 160, 190),
                Color32::from_rgb(200, 230, 255),
            ),
            BezelMode::GbaSpFlameRed => (
                Color32::from_rgb(180, 20, 25), // Flame Red Metallic
                Color32::from_rgb(210, 45, 50),
                Color32::from_rgb(230, 210, 100),
            ),
            BezelMode::GameBoyPlayer => (
                Color32::from_rgb(35, 30, 45), // GameCube Purple-Black
                Color32::from_rgb(55, 50, 70),
                Color32::from_rgb(140, 120, 200),
            ),
            BezelMode::None => return,
        };

        // 1. Outer Handheld Casing with Smooth Rounding
        painter.rect_filled(shell_rect, 24.0, casing_color);
        painter.rect_stroke(shell_rect, 24.0, Stroke::new(3.0_f32, rim_color), egui::StrokeKind::Outside);

        // 2. Inner Screen Lens Border (Dark Grey LCD Bezel)
        let lens_rect = Rect::from_min_max(
            Pos2::new(screen_rect.min.x - 12.0, screen_rect.min.y - 12.0),
            Pos2::new(screen_rect.max.x + 12.0, screen_rect.max.y + 12.0),
        );
        painter.rect_filled(lens_rect, 8.0, Color32::from_rgb(20, 22, 28));
        painter.rect_stroke(lens_rect, 8.0, Stroke::new(2.0_f32, Color32::from_rgb(45, 48, 56)), egui::StrokeKind::Outside);

        // 3. Power LED Indicator (Top Right of Screen)
        let led_pos = Pos2::new(screen_rect.max.x + 24.0, screen_rect.min.y - 18.0);
        painter.circle_filled(led_pos, 4.5, Color32::from_rgb(40, 255, 60)); // Bright Green Power LED
        painter.circle_stroke(led_pos, 5.5, Stroke::new(1.0_f32, Color32::from_rgb(20, 120, 30)));

        // 4. "GAME BOY ADVANCE" Embossed Logo under the screen
        let logo_y = screen_rect.max.y + 22.0;
        let logo_pos = Pos2::new(screen_rect.center().x, logo_y);
        painter.text(
            logo_pos,
            egui::Align2::CENTER_CENTER,
            "GAME BOY ADVANCE",
            egui::FontId::proportional(12.0),
            accent_color,
        );

        // 5. Interactive D-Pad on Left Wing
        let dpad_center = Pos2::new(shell_rect.min.x + shell_margin_x * 0.45, screen_rect.center().y);
        let dpad_arm = 14.0;
        let dpad_thick = 10.0;

        // Draw D-pad cross
        let cross_h = Rect::from_center_size(dpad_center, Vec2::new(dpad_arm * 2.0, dpad_thick));
        let cross_v = Rect::from_center_size(dpad_center, Vec2::new(dpad_thick, dpad_arm * 2.0));
        painter.rect_filled(cross_h, 3.0, Color32::from_rgb(30, 30, 35));
        painter.rect_filled(cross_v, 3.0, Color32::from_rgb(30, 30, 35));

        // Active D-pad arrow highlights (keys: [A=0, B=1, Sel=2, Start=3, Right=4, Left=5, Up=6, Down=7, R=8, L=9])
        if keys_pressed[6] { painter.circle_filled(Pos2::new(dpad_center.x, dpad_center.y - 10.0), 4.0, Color32::LIGHT_GREEN); } // Up
        if keys_pressed[7] { painter.circle_filled(Pos2::new(dpad_center.x, dpad_center.y + 10.0), 4.0, Color32::LIGHT_GREEN); } // Down
        if keys_pressed[5] { painter.circle_filled(Pos2::new(dpad_center.x - 10.0, dpad_center.y), 4.0, Color32::LIGHT_GREEN); } // Left
        if keys_pressed[4] { painter.circle_filled(Pos2::new(dpad_center.x + 10.0, dpad_center.y), 4.0, Color32::LIGHT_GREEN); } // Right

        // 6. Interactive A & B Buttons on Right Wing
        let b_pos = Pos2::new(shell_rect.max.x - shell_margin_x * 0.55, screen_rect.center().y + 8.0);
        let a_pos = Pos2::new(shell_rect.max.x - shell_margin_x * 0.32, screen_rect.center().y - 8.0);

        let btn_radius = 11.0;
        let b_color = if keys_pressed[1] { Color32::from_rgb(100, 220, 255) } else { Color32::from_rgb(35, 40, 50) };
        let a_color = if keys_pressed[0] { Color32::from_rgb(100, 220, 255) } else { Color32::from_rgb(35, 40, 50) };

        painter.circle_filled(b_pos, btn_radius, b_color);
        painter.circle_stroke(b_pos, btn_radius, Stroke::new(1.5_f32, rim_color));
        painter.text(b_pos, egui::Align2::CENTER_CENTER, "B", egui::FontId::monospace(11.0), Color32::LIGHT_GRAY);

        painter.circle_filled(a_pos, btn_radius, a_color);
        painter.circle_stroke(a_pos, btn_radius, Stroke::new(1.5_f32, rim_color));
        painter.text(a_pos, egui::Align2::CENTER_CENTER, "A", egui::FontId::monospace(11.0), Color32::LIGHT_GRAY);

        // 7. Speaker Grille (6 small circular holes bottom right)
        let speaker_origin = Pos2::new(shell_rect.max.x - shell_margin_x * 0.45, shell_rect.max.y - 30.0);
        for row in 0..2 {
            for col in 0..3 {
                let dot_pos = Pos2::new(speaker_origin.x + (col as f32) * 8.0, speaker_origin.y + (row as f32) * 8.0);
                painter.circle_filled(dot_pos, 2.0, Color32::from_rgb(15, 15, 20));
            }
        }
    }
}
