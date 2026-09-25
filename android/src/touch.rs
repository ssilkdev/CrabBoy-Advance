//! On-screen touch controls: layout, multi-touch hit testing, and drawing.

use std::collections::HashMap;

use egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};

/// Button bitmask shared by touch and controller input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Buttons(pub u16);

impl Buttons {
    pub const A: u16 = 1 << 0;
    pub const B: u16 = 1 << 1;
    pub const SELECT: u16 = 1 << 2;
    pub const START: u16 = 1 << 3;
    pub const RIGHT: u16 = 1 << 4;
    pub const LEFT: u16 = 1 << 5;
    pub const UP: u16 = 1 << 6;
    pub const DOWN: u16 = 1 << 7;
    pub const R: u16 = 1 << 8;
    pub const L: u16 = 1 << 9;
    /// Front-end only: open the menu.
    pub const MENU: u16 = 1 << 14;
    /// Front-end only: toggle fast-forward.
    pub const FAST: u16 = 1 << 15;
}

/// Which hand the controls are laid out for (ROADMAP M10). Mirrors
/// `gba::accessibility::Handedness`; kept separate so this module stays
/// buildable and testable on the host without the emulator crate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Hand {
    #[default]
    Both,
    Left,
    Right,
}

/// Where everything goes on screen, in egui points.
pub struct Layout {
    /// The game image.
    pub screen: Rect,
    pub dpad_center: Pos2,
    pub dpad_radius: f32,
    pub a: (Pos2, f32),
    pub b: (Pos2, f32),
    pub l: Rect,
    pub r: Rect,
    pub start: Rect,
    pub select: Rect,
    pub menu: Rect,
    pub fast: Rect,
}

impl Layout {
    /// Portrait: game on top, controls below. Landscape: a control column on
    /// each side and the game scaled to fit between them.
    pub fn compute(safe: Rect, tex_size: Option<[usize; 2]>) -> Self {
        Self::compute_for(safe, tex_size, Hand::Both)
    }

    /// Layout for a given hand. One-handed layouts stack every control in
    /// one column on that side, so a single thumb reaches all of it.
    pub fn compute_for(safe: Rect, tex_size: Option<[usize; 2]>, hand: Hand) -> Self {
        let [tw, th] = tex_size.unwrap_or([240, 160]);
        let aspect = tw as f32 / th as f32;
        let (w, h) = (safe.width(), safe.height());
        // A comfortable thumb target: ~13% of the short side, within limits.
        let unit = (w.min(h) * 0.135).clamp(34.0, 80.0);
        if hand != Hand::Both {
            return Self::one_handed(safe, aspect, unit, hand == Hand::Left);
        }
        if h > w {
            Self::portrait(safe, aspect, unit)
        } else {
            Self::landscape(safe, aspect, unit)
        }
    }

    fn portrait(safe: Rect, aspect: f32, unit: f32) -> Self {
        let screen_h = (safe.width() / aspect).min(safe.height() * 0.5);
        let screen_w = screen_h * aspect;
        let screen = Rect::from_min_size(
            Pos2::new(safe.center().x - screen_w / 2.0, safe.top()),
            Vec2::new(screen_w, screen_h),
        );
        let ctrl = Rect::from_min_max(Pos2::new(safe.left(), screen.bottom()), safe.max);
        let shoulder_y = ctrl.top() + unit * 0.9;
        let face_y = ctrl.top() + ctrl.height() * 0.45;
        let meta_y = ctrl.bottom() - unit * 0.9;
        let margin = unit * 0.35;
        let (dpad_center, dpad_radius, a, b) = Self::pads(safe.left() + margin, safe.right() - margin, face_y, unit);

        let shoulder = Vec2::new(unit * 1.8, unit * 0.9);
        let l = Rect::from_min_size(Pos2::new(safe.left() + margin, shoulder_y - shoulder.y / 2.0), shoulder);
        let r = Rect::from_min_size(
            Pos2::new(safe.right() - margin - shoulder.x, shoulder_y - shoulder.y / 2.0),
            shoulder,
        );

        let cx = safe.center().x;
        let pill = Vec2::new(unit * 1.6, unit * 0.62);
        let gap = unit * 0.3;
        let select = Rect::from_center_size(Pos2::new(cx - pill.x / 2.0 - gap / 2.0, meta_y), pill);
        let start = Rect::from_center_size(Pos2::new(cx + pill.x / 2.0 + gap / 2.0, meta_y), pill);

        // Menu and fast-forward sit centered between the shoulders, apart
        // enough that a sloppy tap cannot hit both.
        let small = Vec2::splat(unit * 0.85);
        let small_gap = unit * 0.5;
        let menu = Rect::from_center_size(Pos2::new(cx - (small.x + small_gap) / 2.0, shoulder_y), small);
        let fast = Rect::from_center_size(Pos2::new(cx + (small.x + small_gap) / 2.0, shoulder_y), small);

        Self { screen, dpad_center, dpad_radius, a, b, l, r, start, select, menu, fast }
    }

    fn landscape(safe: Rect, aspect: f32, unit: f32) -> Self {
        let margin = unit * 0.35;
        // Each side column holds the D-pad or the A/B cluster.
        let column = margin * 2.0 + unit * 2.9;
        let fit_w = (safe.width() - 2.0 * column).max(safe.width() * 0.5);
        let screen_w = (safe.height() * aspect).min(fit_w);
        let screen = Rect::from_center_size(safe.center(), Vec2::new(screen_w, screen_w / aspect));

        let left_x = safe.left() + margin;
        let right_x = safe.right() - margin;
        let (dpad_center, dpad_radius, a, b) = Self::pads(left_x, right_x, safe.center().y + unit * 0.2, unit);

        // Top of each column: shoulder plus a small button (menu left, fast right).
        let top_y = safe.top() + unit * 0.7;
        let shoulder = Vec2::new(unit * 1.6, unit * 0.85);
        let small = Vec2::splat(unit * 0.85);
        let l = Rect::from_min_size(Pos2::new(left_x, top_y - shoulder.y / 2.0), shoulder);
        let menu = Rect::from_center_size(Pos2::new(l.right() + margin + small.x / 2.0, top_y), small);
        let r = Rect::from_min_size(Pos2::new(right_x - shoulder.x, top_y - shoulder.y / 2.0), shoulder);
        let fast = Rect::from_center_size(Pos2::new(r.left() - margin - small.x / 2.0, top_y), small);

        // Bottom of each column: Select under the D-pad, Start under A/B.
        let bottom_y = safe.bottom() - unit * 0.55;
        let pill = Vec2::new(unit * 1.6, unit * 0.62);
        let select = Rect::from_center_size(Pos2::new(dpad_center.x, bottom_y), pill);
        let start = Rect::from_center_size(Pos2::new((a.0.x + b.0.x) / 2.0, bottom_y), pill);

        Self { screen, dpad_center, dpad_radius, a, b, l, r, start, select, menu, fast }
    }

    /// Height of the one-handed control stack, in units.
    const STACK_UNITS: f32 = 9.2;
    /// Width of the one-handed control stack, in units (without margins).
    const STACK_WIDTH_UNITS: f32 = 3.5;

    fn one_handed(safe: Rect, aspect: f32, unit: f32, left: bool) -> Self {
        let portrait = safe.height() > safe.width();
        let (screen, area) = if portrait {
            let screen_h = (safe.width() / aspect).min(safe.height() * 0.45);
            let screen_w = screen_h * aspect;
            let screen = Rect::from_min_size(
                Pos2::new(safe.center().x - screen_w / 2.0, safe.top()),
                Vec2::new(screen_w, screen_h),
            );
            (screen, Rect::from_min_max(Pos2::new(safe.left(), screen.bottom()), safe.max))
        } else {
            // Column on the chosen side; the game fills the rest.
            let u = unit.min(safe.height() / Self::STACK_UNITS);
            let col_w = u * (Self::STACK_WIDTH_UNITS + 0.8);
            let (col, rest) = if left {
                (
                    Rect::from_min_max(safe.min, Pos2::new(safe.left() + col_w, safe.bottom())),
                    Rect::from_min_max(Pos2::new(safe.left() + col_w, safe.top()), safe.max),
                )
            } else {
                (
                    Rect::from_min_max(Pos2::new(safe.right() - col_w, safe.top()), safe.max),
                    Rect::from_min_max(safe.min, Pos2::new(safe.right() - col_w, safe.bottom())),
                )
            };
            let screen_w = (rest.height() * aspect).min(rest.width());
            let screen = Rect::from_center_size(rest.center(), Vec2::new(screen_w, screen_w / aspect));
            (screen, col)
        };

        let unit = unit
            .min(area.height() / Self::STACK_UNITS)
            .min(area.width() / (Self::STACK_WIDTH_UNITS + 0.8));
        let margin = unit * 0.4;
        let half_w = unit * Self::STACK_WIDTH_UNITS / 2.0;
        let cx = if left { area.left() + margin + half_w } else { area.right() - margin - half_w };
        let gap = unit * 0.3;
        // Stack from the bottom up, where the thumb rests: Select/Start,
        // D-pad, A/B, shoulders, then menu / fast-forward.
        let mut y = area.bottom() - margin;

        let pill = Vec2::new(unit * 1.5, unit * 0.62);
        y -= pill.y / 2.0;
        let select = Rect::from_center_size(Pos2::new(cx - pill.x / 2.0 - gap / 2.0, y), pill);
        let start = Rect::from_center_size(Pos2::new(cx + pill.x / 2.0 + gap / 2.0, y), pill);
        y -= pill.y / 2.0 + gap;

        let dpad_radius = unit * 1.35;
        y -= dpad_radius;
        let dpad_center = Pos2::new(cx, y);
        y -= dpad_radius * 1.25 + gap;

        let face_r = unit * 0.6;
        y -= face_r * 1.6;
        let a = (Pos2::new(cx + face_r * 1.25, y - face_r * 0.3), face_r);
        let b = (Pos2::new(cx - face_r * 1.25, y + face_r * 0.3), face_r);
        y -= face_r * 1.6 + gap;

        let shoulder = Vec2::new(unit * 1.6, unit * 0.8);
        y -= shoulder.y / 2.0;
        let l = Rect::from_center_size(Pos2::new(cx - shoulder.x / 2.0 - gap / 2.0, y), shoulder);
        let r = Rect::from_center_size(Pos2::new(cx + shoulder.x / 2.0 + gap / 2.0, y), shoulder);
        y -= shoulder.y / 2.0 + gap;

        let small = Vec2::splat(unit * 0.8);
        y -= small.y / 2.0;
        let menu = Rect::from_center_size(Pos2::new(cx - small.x / 2.0 - gap, y), small);
        let fast = Rect::from_center_size(Pos2::new(cx + small.x / 2.0 + gap, y), small);

        Self { screen, dpad_center, dpad_radius, a, b, l, r, start, select, menu, fast }
    }

    /// D-pad hugging `left_x`, A/B cluster hugging `right_x`, both around `y`.
    fn pads(left_x: f32, right_x: f32, y: f32, unit: f32) -> (Pos2, f32, (Pos2, f32), (Pos2, f32)) {
        let dpad_radius = unit * 1.45;
        let dpad_center = Pos2::new(left_x + dpad_radius, y);
        let face_r = unit * 0.62;
        let a = Pos2::new(right_x - face_r, y - face_r * 0.9);
        let b = Pos2::new(a.x - face_r * 2.4, y + face_r * 0.6);
        (dpad_center, dpad_radius, (a, face_r), (b, face_r))
    }

    /// Buttons under one finger. The D-pad resolves to one or two directions
    /// (diagonals), and face buttons get a generous hit radius so a thumb
    /// resting between A and B presses both, like on real hardware.
    pub fn hit(&self, p: Pos2) -> u16 {
        let mut bits = 0;

        let d = p - self.dpad_center;
        let dist = d.length();
        if dist <= self.dpad_radius * 1.25 && dist >= self.dpad_radius * 0.18 {
            let angle = d.y.atan2(d.x).to_degrees(); // 0 = right, 90 = down
            let dir = |center: f32| {
                let diff = (angle - center + 540.0).rem_euclid(360.0) - 180.0;
                diff.abs() <= 67.5
            };
            if dir(0.0) {
                bits |= Buttons::RIGHT;
            }
            if dir(90.0) {
                bits |= Buttons::DOWN;
            }
            if dir(180.0) {
                bits |= Buttons::LEFT;
            }
            if dir(-90.0) {
                bits |= Buttons::UP;
            }
        }

        for ((center, r), bit) in [(self.a, Buttons::A), (self.b, Buttons::B)] {
            if (p - center).length() <= r * 1.3 {
                bits |= bit;
            }
        }
        for (rect, bit) in [
            (self.l, Buttons::L),
            (self.r, Buttons::R),
            (self.start, Buttons::START),
            (self.select, Buttons::SELECT),
            (self.menu, Buttons::MENU),
            (self.fast, Buttons::FAST),
        ] {
            if rect.expand(8.0).contains(p) {
                bits |= bit;
            }
        }
        bits
    }
}

/// Tracks every active finger so buttons can be held simultaneously and a
/// thumb can slide from one button to another.
#[derive(Default)]
pub struct TouchPad {
    fingers: HashMap<u64, Pos2>,
    held: u16,
    prev: u16,
}

impl TouchPad {
    pub fn update(&mut self, ctx: &egui::Context, layout: &Layout) -> Buttons {
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Touch { id, phase, pos, .. } = ev {
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.fingers.insert(id.0, *pos);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.fingers.remove(&id.0);
                        }
                    }
                }
            }
        });
        self.prev = self.held;
        self.held = self.fingers.values().fold(0, |acc, &p| acc | layout.hit(p));
        Buttons(self.held)
    }

    /// Release everything (used while the menu is open).
    pub fn clear(&mut self) {
        self.fingers.clear();
        self.prev = self.held;
        self.held = 0;
    }

    pub fn held(&self) -> u16 {
        self.held
    }

    pub fn just_pressed(&self, bit: u16) -> bool {
        self.held & bit != 0 && self.prev & bit == 0
    }
}

/// How to draw the controls: theme colours plus optional skin images.
pub struct Look<'a> {
    pub colors: crate::skin::ThemeColors,
    /// Texture and the part of it to show (animation frame) for a control,
    /// by (control, pressed). A pressed image falls back to the normal one.
    pub images: &'a dyn Fn(crate::skin::Control, bool) -> Option<(egui::TextureId, Rect)>,
}

impl Look<'_> {
    fn image(&self, c: crate::skin::Control, pressed: bool) -> Option<(egui::TextureId, Rect)> {
        (self.images)(c, pressed).or_else(|| if pressed { (self.images)(c, false) } else { None })
    }
}

/// Classic colours, no images.
pub fn classic_look() -> Look<'static> {
    Look { colors: crate::skin::Theme::CLASSIC.colors(1.0), images: &|_, _| None }
}

fn draw_image(p: &Painter, (tex, uv): (egui::TextureId, Rect), rect: Rect, pressed: bool) {
    // Pressed without its own image: darken the normal one.
    let tint = if pressed { Color32::from_gray(170) } else { Color32::WHITE };
    p.image(tex, rect, uv, tint);
}

pub fn paint(p: &Painter, l: &Layout, held: u16, fast_forward: bool, look: &Look<'_>) {
    use crate::skin::Control;
    let k = look.colors;
    let stroke = Stroke::new(2.0_f32, k.edge);
    let fill = |bit: u16| if held & bit != 0 { k.pressed } else { k.fill };

    // D-pad: a plus shape made of four arms around a hub, or the skin's
    // image (a pressed direction then shows as a highlighted arm).
    let arm = l.dpad_radius * 0.62;
    let thick = l.dpad_radius * 0.62;
    let c = l.dpad_center;
    let dirs = Buttons::UP | Buttons::DOWN | Buttons::LEFT | Buttons::RIGHT;
    let dpad_rect = Rect::from_center_size(c, Vec2::splat(l.dpad_radius * 2.0));
    let dpad_img = look.image(Control::Dpad, held & dirs != 0);
    if let Some(tex) = dpad_img {
        let own_pressed = (look.images)(Control::Dpad, true).is_some();
        draw_image(p, tex, dpad_rect, held & dirs != 0 && !own_pressed);
    }
    for (bit, dir) in [
        (Buttons::UP, Vec2::new(0.0, -1.0)),
        (Buttons::DOWN, Vec2::new(0.0, 1.0)),
        (Buttons::LEFT, Vec2::new(-1.0, 0.0)),
        (Buttons::RIGHT, Vec2::new(1.0, 0.0)),
    ] {
        let center = c + dir * arm;
        let size = if dir.x == 0.0 { Vec2::new(thick, arm * 1.1) } else { Vec2::new(arm * 1.1, thick) };
        let rect = Rect::from_center_size(center, size);
        if dpad_img.is_some() {
            if held & bit != 0 {
                p.rect_filled(rect, 6.0, k.pressed.gamma_multiply(0.6));
            }
            continue;
        }
        p.rect(rect, 6.0, fill(bit), stroke, egui::StrokeKind::Inside);
        // Arrow drawn as a triangle: the default fonts lack ▲/▼.
        let tip = center + dir * arm * 0.35;
        let base = center - dir * arm * 0.05;
        let side = dir.rot90() * thick * 0.2;
        p.add(egui::Shape::convex_polygon(vec![tip, base + side, base - side], k.label, Stroke::NONE));
    }
    if dpad_img.is_none() {
        p.rect_filled(Rect::from_center_size(c, Vec2::splat(thick)), 2.0, k.fill);
    }

    for ((center, r), bit, label, ctl) in [(l.a, Buttons::A, "A", Control::A), (l.b, Buttons::B, "B", Control::B)] {
        let down = held & bit != 0;
        if let Some(tex) = look.image(ctl, down) {
            let own = (look.images)(ctl, true).is_some();
            draw_image(p, tex, Rect::from_center_size(center, Vec2::splat(r * 2.0)), down && !own);
            continue;
        }
        p.circle(center, r, fill(bit), stroke);
        p.text(center, Align2::CENTER_CENTER, label, FontId::proportional(r * 0.9), k.label);
    }

    for (rect, bit, label, ctl) in [
        (l.l, Buttons::L, "L", Control::L),
        (l.r, Buttons::R, "R", Control::R),
        (l.select, Buttons::SELECT, "SELECT", Control::Select),
        (l.start, Buttons::START, "START", Control::Start),
    ] {
        let down = held & bit != 0;
        if let Some(tex) = look.image(ctl, down) {
            let own = (look.images)(ctl, true).is_some();
            draw_image(p, tex, rect, down && !own);
            continue;
        }
        p.rect(rect, rect.height() / 2.0, fill(bit), stroke, egui::StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(rect.height() * 0.42), k.label);
    }

    if let Some(tex) = look.image(Control::Fast, fast_forward) {
        let own = (look.images)(Control::Fast, true).is_some();
        draw_image(p, tex, l.fast, fast_forward && !own);
    } else {
        let ff_fill = if fast_forward { k.pressed } else { k.fill };
        p.rect(l.fast, 10.0, ff_fill, stroke, egui::StrokeKind::Inside);
        p.text(l.fast.center(), Align2::CENTER_CENTER, "⏩", FontId::proportional(l.fast.height() * 0.45), k.label);
    }
}

pub fn paint_menu_button(p: &Painter, l: &Layout, look: &Look<'_>) {
    if let Some(tex) = look.image(crate::skin::Control::Menu, false) {
        draw_image(p, tex, l.menu, false);
        return;
    }
    let k = look.colors;
    p.rect(l.menu, 10.0, k.fill, Stroke::new(2.0_f32, k.edge), egui::StrokeKind::Inside);
    p.text(l.menu.center(), Align2::CENTER_CENTER, "☰", FontId::proportional(l.menu.height() * 0.5), k.label);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portrait() -> Layout {
        Layout::compute(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 860.0)), Some([240, 160]))
    }

    #[test]
    fn dpad_directions_and_diagonals() {
        let l = portrait();
        let c = l.dpad_center;
        let r = l.dpad_radius * 0.8;
        assert_eq!(l.hit(c + Vec2::new(r, 0.0)), Buttons::RIGHT);
        assert_eq!(l.hit(c + Vec2::new(0.0, -r)), Buttons::UP);
        assert_eq!(l.hit(c + Vec2::new(r, r) * 0.7), Buttons::RIGHT | Buttons::DOWN);
        assert_eq!(l.hit(c), 0, "the dead zone in the middle presses nothing");
    }

    #[test]
    fn face_buttons_do_not_overlap_the_dpad_or_screen() {
        let l = portrait();
        assert_eq!(l.hit(l.a.0), Buttons::A);
        assert_eq!(l.hit(l.b.0), Buttons::B);
        assert!(l.a.0.y > l.screen.bottom() && l.b.0.y > l.screen.bottom());
        assert_eq!(l.hit(l.screen.center()), 0);
    }

    #[test]
    fn landscape_keeps_game_aspect() {
        let l = Layout::compute(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 400.0)), Some([240, 160]));
        assert!((l.screen.width() / l.screen.height() - 1.5).abs() < 0.01);
        assert_eq!(l.hit(l.start.center()), Buttons::START);
    }

    #[test]
    fn landscape_controls_stay_off_the_game_image() {
        // Typical 20:9 phone in landscape (points).
        let l = Layout::compute(Rect::from_min_size(Pos2::ZERO, Vec2::new(914.0, 411.0)), Some([240, 160]));
        for rect in [l.l, l.r, l.menu, l.fast, l.start, l.select] {
            assert!(!rect.intersects(l.screen), "{rect:?} overlaps {:?}", l.screen);
        }
        assert!(l.dpad_center.x + l.dpad_radius <= l.screen.left());
        assert!(l.b.0.x - l.b.1 >= l.screen.right());
        assert!(l.screen.height() > 411.0 * 0.8, "game should still be large");
    }

    fn all_controls(l: &Layout) -> Vec<Rect> {
        let circle = |(c, r): (Pos2, f32)| Rect::from_center_size(c, Vec2::splat(r * 2.0));
        vec![
            Rect::from_center_size(l.dpad_center, Vec2::splat(l.dpad_radius * 2.0)),
            circle(l.a),
            circle(l.b),
            l.l, l.r, l.start, l.select, l.menu, l.fast,
        ]
    }

    #[test]
    fn one_handed_layouts_fit_one_side_and_every_button_works() {
        for size in [Vec2::new(360.0, 780.0), Vec2::new(412.0, 915.0), Vec2::new(915.0, 412.0), Vec2::new(780.0, 360.0)] {
            let safe = Rect::from_min_size(Pos2::ZERO, size);
            for hand in [Hand::Left, Hand::Right] {
                let l = Layout::compute_for(safe, Some([240, 160]), hand);
                let ctx = format!("{size:?} {hand:?}");
                // Every control is on screen, off the game image, and in a
                // band a thumb on that side can reach (the side 60% of the
                // width in portrait).
                for rect in all_controls(&l) {
                    assert!(safe.expand(1.0).contains_rect(rect), "{ctx}: {rect:?} off screen");
                    assert!(!rect.intersects(l.screen), "{ctx}: {rect:?} covers the game");
                    if size.y > size.x {
                        let reach = size.x * 0.6;
                        let ok = if hand == Hand::Left { rect.right() <= reach } else { rect.left() >= size.x - reach };
                        assert!(ok, "{ctx}: {rect:?} out of thumb reach");
                    }
                }
                // Each button is hit on its own.
                for (p, want) in [
                    (l.a.0, Buttons::A),
                    (l.b.0, Buttons::B),
                    (l.menu.center(), Buttons::MENU),
                    (l.fast.center(), Buttons::FAST),
                    (l.l.center(), Buttons::L),
                    (l.r.center(), Buttons::R),
                    (l.select.center(), Buttons::SELECT),
                    (l.start.center(), Buttons::START),
                    (l.dpad_center + Vec2::new(0.0, -l.dpad_radius * 0.8), Buttons::UP),
                    (l.dpad_center + Vec2::new(0.0, l.dpad_radius * 0.8), Buttons::DOWN),
                ] {
                    assert_eq!(l.hit(p), want, "{ctx}");
                }
                assert!(l.screen.width() > size.x.min(size.y) * 0.6, "{ctx}: game too small");
                assert!((l.screen.width() / l.screen.height() - 1.5).abs() < 0.01, "{ctx}");
            }
        }
    }

    #[test]
    fn buttons_do_not_overlap() {
        for size in [Vec2::new(360.0, 780.0), Vec2::new(412.0, 915.0), Vec2::new(915.0, 412.0)] {
            let l = Layout::compute(Rect::from_min_size(Pos2::ZERO, size), Some([240, 160]));
            for (p, want) in [
                (l.menu.center(), Buttons::MENU),
                (l.fast.center(), Buttons::FAST),
                (l.l.center(), Buttons::L),
                (l.r.center(), Buttons::R),
                (l.select.center(), Buttons::SELECT),
                (l.start.center(), Buttons::START),
            ] {
                assert_eq!(l.hit(p), want, "size {size:?}");
            }
        }
    }
}
