//! Official Pokémon Trainer's Strategy Guide & Hardware Field Manual Dialog
//! Embedded directly into the CrabBoy Advance binary executable for release builds.

use eframe::egui::{self, Color32, RichText, ScrollArea, TextureHandle, TextureOptions, Vec2};
use std::path::PathBuf;

pub const EMBEDDED_MANUAL_PDF: &[u8] = include_bytes!("../../assets/guide/manual.pdf");

pub const EMBEDDED_PAGES: [&[u8]; 8] = [
    include_bytes!("../../assets/guide/page_1.png"),
    include_bytes!("../../assets/guide/page_2.png"),
    include_bytes!("../../assets/guide/page_3.png"),
    include_bytes!("../../assets/guide/page_4.png"),
    include_bytes!("../../assets/guide/page_5.png"),
    include_bytes!("../../assets/guide/page_6.png"),
    include_bytes!("../../assets/guide/page_7.png"),
    include_bytes!("../../assets/guide/page_8.png"),
];

const CHAPTER_TITLES: [&str; 8] = [
    "Cover: Trainer's Field Manual & Index",
    "Ch 1: Professor's Welcome & Quick Start",
    "Ch 2: Controls & Joypad Configuration",
    "Ch 3: Visual Filters & Display Shaders",
    "Ch 4: Live Companion & Memory Explorer",
    "Ch 5: Silph Co. RTC, Solar & Sensors",
    "Ch 6: SIO Link Cable, Cheats & Audio Gym",
    "Ch 7: TAS Speedrunning & Troubleshooting",
];

pub struct GuideDialog {
    pub is_open: bool,
    pub current_page: usize,
    textures: [Option<TextureHandle>; 8],
    zoom: f32,
    fit_to_width: bool,
}

impl Default for GuideDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl GuideDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            current_page: 0,
            textures: [None, None, None, None, None, None, None, None],
            zoom: 1.0,
            fit_to_width: true,
        }
    }

    /// Open the dialog and jump to a specific page (0..=7)
    pub fn open_at_page(&mut self, page: usize) {
        self.current_page = page.min(7);
        self.is_open = true;
    }

    /// Extract and launch the embedded PDF in the system's default viewer
    pub fn open_external_pdf(&self) -> Result<PathBuf, String> {
        let temp_dir = std::env::temp_dir();
        let target_path = temp_dir.join("CrabBoy_Advance_Trainers_Guide.pdf");

        std::fs::write(&target_path, EMBEDDED_MANUAL_PDF)
            .map_err(|e| format!("Failed to write embedded PDF to temp directory: {}", e))?;

        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "start", "", &target_path.to_string_lossy()])
                .spawn();
            if let Err(e) = status {
                return Err(format!("Failed to launch default PDF viewer: {}", e));
            }
        }

        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(&target_path)
                .spawn()
                .map_err(|e| format!("Failed to launch PDF viewer: {}", e))?;
        }

        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&target_path)
                .spawn()
                .map_err(|e| format!("Failed to launch PDF viewer: {}", e))?;
        }

        Ok(target_path)
    }

    /// Export the embedded PDF to a user-specified file path
    pub fn export_pdf_as(&self) -> Result<Option<PathBuf>, String> {
        if let Some(save_path) = rfd::FileDialog::new()
            .set_file_name("CrabBoy_Advance_Trainers_Guide.pdf")
            .add_filter("PDF Document", &["pdf"])
            .save_file()
        {
            std::fs::write(&save_path, EMBEDDED_MANUAL_PDF)
                .map_err(|e| format!("Failed to export PDF: {}", e))?;
            Ok(Some(save_path))
        } else {
            Ok(None)
        }
    }

    /// Lazily load and decode the texture for the requested page
    fn ensure_page_texture(&mut self, ctx: &egui::Context, page_idx: usize) {
        if page_idx >= 8 {
            return;
        }

        if self.textures[page_idx].is_none() {
            let png_bytes = EMBEDDED_PAGES[page_idx];
            if let Ok(img) = image::load_from_memory(png_bytes) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                let texture = ctx.load_texture(
                    format!("guide_page_texture_{}", page_idx + 1),
                    color_img,
                    TextureOptions::LINEAR,
                );
                self.textures[page_idx] = Some(texture);
            }
        }
    }

    /// Renders the strategy guide reader window inside egui
    pub fn show(&mut self, ctx: &egui::Context, toast_out: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut is_open = self.is_open;

        egui::Window::new("📖 CrabBoy Advance: Official Pokémon Trainer's Strategy Guide")
            .open(&mut is_open)
            .default_size(Vec2::new(740.0, 840.0))
            .min_size(Vec2::new(480.0, 520.0))
            .resizable(true)
            .show(ctx, |ui| {
                // Top Header Ribbon
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("★ OFFICIAL TRAINER'S HARDWARE & STRATEGY FIELD MANUAL ★")
                            .color(Color32::from_rgb(255, 215, 0))
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("📥 Open Full PDF").on_hover_text("Open full high-resolution PDF guide in your system's default PDF viewer (Edge, Acrobat, etc.)").clicked() {
                            match self.open_external_pdf() {
                                Ok(_) => *toast_out = Some("📖 Opened Trainer's Guide in PDF Viewer".to_string()),
                                Err(e) => *toast_out = Some(format!("Error opening PDF: {}", e)),
                            }
                        }

                        if ui.button("💾 Export PDF...").on_hover_text("Save a standalone copy of CrabBoy_Advance_Trainers_Guide.pdf to your computer").clicked() {
                            match self.export_pdf_as() {
                                Ok(Some(path)) => *toast_out = Some(format!("Saved: {}", path.file_name().unwrap_or_default().to_string_lossy())),
                                Ok(None) => {},
                                Err(e) => *toast_out = Some(format!("Export error: {}", e)),
                            }
                        }
                    });
                });

                ui.separator();

                // Navigation Toolbar
                ui.horizontal_wrapped(|ui| {
                    let has_prev = self.current_page > 0;
                    if ui.add_enabled(has_prev, egui::Button::new("◀ Prev Page")).clicked() {
                        if self.current_page > 0 {
                            self.current_page -= 1;
                        }
                    }

                    let has_next = self.current_page < 7;
                    if ui.add_enabled(has_next, egui::Button::new("Next Page ▶")).clicked() {
                        if self.current_page < 7 {
                            self.current_page += 1;
                        }
                    }

                    ui.separator();

                    // Chapter Selection ComboBox
                    egui::ComboBox::from_id_salt("guide_chapter_selector")
                        .width(260.0)
                        .selected_text(format!("Page {}: {}", self.current_page + 1, CHAPTER_TITLES[self.current_page]))
                        .show_ui(ui, |ui| {
                            for (idx, title) in CHAPTER_TITLES.iter().enumerate() {
                                if ui.selectable_value(&mut self.current_page, idx, format!("Page {}: {}", idx + 1, title)).clicked() {
                                    // Jump to selected chapter
                                }
                            }
                        });

                    ui.separator();

                    // Zoom Controls
                    ui.label("Zoom:");
                    if ui.button("➖").clicked() {
                        self.zoom = (self.zoom - 0.15).max(0.5);
                        self.fit_to_width = false;
                    }
                    ui.label(format!("{:.0}%", self.zoom * 100.0));
                    if ui.button("➕").clicked() {
                        self.zoom = (self.zoom + 0.15).min(2.5);
                        self.fit_to_width = false;
                    }
                    if ui.checkbox(&mut self.fit_to_width, "Fit Width").clicked() {
                        if self.fit_to_width {
                            self.zoom = 1.0;
                        }
                    }
                });

                // Quick Chapter Jump Pills
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    let quick_tabs = [
                        ("Cover", 0),
                        ("1: Welcome", 1),
                        ("2: Controls", 2),
                        ("3: Filters", 3),
                        ("4: Companion", 4),
                        ("5: Sensors & RTC", 5),
                        ("6: SIO & Cheats", 6),
                        ("7: TAS & FAQ", 7),
                    ];
                    for (name, page) in quick_tabs {
                        let is_active = self.current_page == page;
                        if ui.selectable_label(is_active, name).clicked() {
                            self.current_page = page;
                        }
                    }
                });

                ui.separator();

                // Page Display Viewport (ScrollArea)
                let available_width = ui.available_width();
                let fit_to_width = self.fit_to_width;
                let zoom = self.zoom;
                let page_idx = self.current_page;

                self.ensure_page_texture(ctx, page_idx);

                ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(ref texture) = self.textures[page_idx] {
                            let tex_size = texture.size_vec2();
                            let aspect_ratio = tex_size.y / tex_size.x;

                            let display_width = if fit_to_width {
                                (available_width - 24.0).max(320.0)
                            } else {
                                tex_size.x * 0.7 * zoom
                            };
                            let display_height = display_width * aspect_ratio;

                            ui.vertical_centered(|ui| {
                                ui.image((texture.id(), Vec2::new(display_width, display_height)));
                            });
                        } else {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.label(RichText::new("Loading Guide Page...").size(16.0).color(Color32::LIGHT_GRAY));
                            });
                        }
                    });
            });

        self.is_open = is_open;
    }
}
