//! Hardware Cartridge Sensors Dialog (Solar Sensor, Gyro/Tilt & Rumble)

use crate::gba::mmu::sensors::SensorType;
use crate::gba::Gba;
use egui::{Color32, Pos2, RichText, Stroke, Vec2, Window};

#[derive(Default)]
pub struct SensorsDialog {
    pub is_open: bool,
}

impl SensorsDialog {
    pub fn new() -> Self {
        Self { is_open: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, gba: &mut Gba, toast: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("🎮 Cartridge Hardware Sensors")
            .open(&mut open)
            .default_width(460.0)
            .resizable(false)
            .show(ctx, |ui| {
                let (detected_sensor, title) = if let Some(ref cart) = gba.mmu.cartridge {
                    (cart.sensors.sensor_type, cart.title.clone())
                } else {
                    (SensorType::None, "No Cartridge Loaded".to_string())
                };

                ui.heading("Hardware Cartridge Sensors");
                let sensor_name = match detected_sensor {
                    SensorType::Solar => "☀️ Photodiode Solar Sensor Detected!",
                    SensorType::GyroTilt => "🌀 Piezoelectric Gyro / Accelerometer Detected!",
                    SensorType::Rumble => "📳 Cartridge Rumble Pak (Force Feedback) Detected!",
                    SensorType::None => "No specialized cartridge sensor hardware in current ROM.",
                };
                let badge_color = match detected_sensor {
                    SensorType::None => Color32::GRAY,
                    _ => Color32::LIGHT_GREEN,
                };
                ui.label(RichText::new(format!("Cartridge: '{}'", title)).strong());
                ui.label(RichText::new(sensor_name).color(badge_color).strong());
                ui.separator();

                // 1. Solar Sensor Controls
                ui.heading("☀️ Solar Sensor (Photodiode)");
                ui.label(RichText::new("Controls the cartridge photodiode light intensity sensor.").weak().small());

                if let Some(ref mut cart) = gba.mmu.cartridge {
                    let mut level = cart.sensors.sunlight_level;
                    let icons = ["🌑 Darkness", "🌘 Level 1", "🌘 Level 2", "🌗 Level 3", "🌗 Level 4", "🌖 Level 5", "🌖 Level 6", "🌕 Level 7", "🌕 Level 8", "☀️ Level 9", "🔥 Maximum Sun (10)"];
                    let current_icon = icons.get(level as usize).unwrap_or(&"☀️");

                    ui.horizontal(|ui| {
                        ui.label("Sunlight Intensity:");
                        if ui.add(egui::Slider::new(&mut level, 0..=10).text(current_icon.to_string())).changed() {
                            cart.sensors.set_sunlight(level);
                        }
                    });

                    ui.horizontal(|ui| {
                        if ui.button("🌑 Darkness (0)").clicked() { cart.sensors.set_sunlight(0); }
                        if ui.button("🌗 Moderate (5)").clicked() { cart.sensors.set_sunlight(5); }
                        if ui.button("🔥 Max Sunlight (10)").clicked() { cart.sensors.set_sunlight(10); }
                    });

                    ui.checkbox(&mut cart.sensors.auto_diurnal_cycle, "Auto-sync with real-world Day / Night solar curve");
                }

                ui.separator();

                // 2. Gyro & Tilt Sensor Controls
                ui.heading("🌀 Gyro & Tilt Sensor (WarioWare Twisted / Yoshi)");
                ui.label(RichText::new("Simulates cartridge rotation and tilt angles.").weak().small());

                if let Some(ref mut cart) = gba.mmu.cartridge {
                    let mut tx = cart.sensors.tilt_x;
                    let mut ty = cart.sensors.tilt_y;

                    ui.horizontal(|ui| {
                        ui.label("Tilt X (Roll):");
                        if ui.add(egui::Slider::new(&mut tx, -1.0..=1.0).text("Horizontal")).changed() {
                            cart.sensors.set_tilt(tx, ty);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Tilt Y (Pitch):");
                        if ui.add(egui::Slider::new(&mut ty, -1.0..=1.0).text("Vertical")).changed() {
                            cart.sensors.set_tilt(tx, ty);
                        }
                    });

                    if ui.button("🎯 Center Neutral (0.0, 0.0)").clicked() {
                        cart.sensors.set_tilt(0.0, 0.0);
                    }

                    // Visual Bubble Level Widget
                    ui.label("Bubble Level:");
                    let (rect, _response) = ui.allocate_exact_size(Vec2::new(120.0, 120.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(30, 30, 40));
                    ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0_f32, Color32::DARK_GRAY), egui::StrokeKind::Outside);
                    let center = rect.center();
                    ui.painter().circle_stroke(center, 40.0, Stroke::new(1.0_f32, Color32::from_rgb(80, 80, 100)));
                    ui.painter().circle_stroke(center, 15.0, Stroke::new(1.0_f32, Color32::from_rgb(100, 100, 140)));

                    let bubble_pos = Pos2::new(
                        center.x + (cart.sensors.tilt_x * 40.0).clamp(-50.0, 50.0),
                        center.y - (cart.sensors.tilt_y * 40.0).clamp(-50.0, 50.0),
                    );
                    ui.painter().circle_filled(bubble_pos, 8.0, Color32::from_rgb(100, 220, 255));
                }

                ui.separator();

                // 3. Rumble Pak Controls
                ui.heading("📳 Cartridge Rumble Pak (Drill Dozer / Pokémon Pinball)");
                if let Some(ref mut cart) = gba.mmu.cartridge {
                    let rumble_status = if cart.sensors.rumble_active {
                        ("🔴 MOTOR SPINNING (Vibrating)", Color32::from_rgb(255, 100, 100))
                    } else {
                        ("⚪ Motor Idle", Color32::GRAY)
                    };
                    ui.label(RichText::new(rumble_status.0).color(rumble_status.1).strong());

                    ui.horizontal(|ui| {
                        ui.label("Rumble Strength:");
                        ui.add(egui::Slider::new(&mut cart.sensors.rumble_strength, 0.0..=1.0).text("Gain"));
                    });

                    if ui.button("💥 Test 200ms Vibration Pulse").clicked() {
                        cart.sensors.rumble_active = true;
                        *toast = Some("Simulating Controller Vibration".to_string());
                    }
                }
            });
        self.is_open = open;
    }
}
