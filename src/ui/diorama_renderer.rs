//! 2.5D / 3D Diorama Perspective Renderer (Inspired by 3Dsen)
//!
//! Projects isolated Background planes, Window planes, and OAM Sprites into 3D space
//! with an interactive orbit perspective camera, depth sorting, and optional voxel relief extrusion.

use crate::gba::ppu::diorama::{DioramaFrameData, Sprite3D};
use crate::gba::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions};

/// Interactive 3D Orbit Perspective Camera
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub yaw: f32,      // Rotation around Y axis (radians)
    pub pitch: f32,    // Rotation around X axis (radians)
    pub distance: f32, // Distance from orbit center (zoom)
    pub pan_x: f32,    // Horizontal pan offset
    pub pan_y: f32,    // Vertical pan offset
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl OrbitCamera {
    pub const DEFAULT_YAW: f32 = 0.32;      // ~18.3 degrees
    pub const DEFAULT_PITCH: f32 = 0.26;    // ~14.9 degrees
    pub const DEFAULT_DISTANCE: f32 = 320.0;

    pub fn new() -> Self {
        Self {
            yaw: Self::DEFAULT_YAW,
            pitch: Self::DEFAULT_PITCH,
            distance: Self::DEFAULT_DISTANCE,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.yaw = Self::DEFAULT_YAW;
        self.pitch = Self::DEFAULT_PITCH;
        self.distance = Self::DEFAULT_DISTANCE;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
    }

    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.007;
        self.pitch = (self.pitch + dy * 0.007).clamp(-1.35, 1.35); // Clamped to ~77 degrees
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance - delta * 18.0).clamp(120.0, 750.0);
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.pan_x += dx;
        self.pan_y += dy;
    }

    /// Transforms a 3D point (x, y, z) into 2D viewport coordinates.
    /// Screen center (0,0) corresponds to GBA center (120, 80).
    /// Returns (screen_pos, view_cam_z).
    #[inline]
    pub fn project_point(
        &self,
        x: f32,
        y: f32,
        z: f32,
        viewport_center: Pos2,
        scale: f32,
    ) -> (Pos2, f32) {
        // Target center point of GBA screen
        let px = x;
        let py = y;
        let pz = z - 20.0; // Anchor around mid-layer depth

        // Rotate yaw (around Y axis)
        let cos_y = self.yaw.cos();
        let sin_y = self.yaw.sin();
        let x1 = px * cos_y - pz * sin_y;
        let z1 = px * sin_y + pz * cos_y;
        let y1 = py;

        // Rotate pitch (around X axis)
        let cos_p = self.pitch.cos();
        let sin_p = self.pitch.sin();
        let y2 = y1 * cos_p - z1 * sin_p;
        let z2 = y1 * sin_p + z1 * cos_p;
        let x2 = x1;

        // Perspective camera distance
        let cam_z = (z2 + self.distance).max(10.0);
        let factor = self.distance / cam_z;

        let proj_x = (x2 + self.pan_x) * factor * scale;
        let proj_y = (y2 + self.pan_y) * factor * scale;

        (
            Pos2::new(viewport_center.x + proj_x, viewport_center.y + proj_y),
            cam_z,
        )
    }
}

/// A textured 3D quad ready for Painter's algorithm depth sorting
struct ProjectedQuad {
    pub points: [Pos2; 4],
    pub uvs: [Pos2; 4],
    pub avg_cam_z: f32,
    pub texture_id: egui::TextureId,
    pub tint: Color32,
}

/// High-Performance 2.5D Diorama Renderer
pub struct DioramaRenderer {
    pub camera: OrbitCamera,
    pub layer_depth_separation: f32, // Spacing between BG layers (default 18.0)
    pub sprite_depth_elevation: f32, // Elevation offset for sprites (default 22.0)
    pub voxel_relief_strength: f32,  // 0.0 = flat quads, >0.0 = extruded relief blocks
    pub show_diorama_grid: bool,     // Floor grid visualization
    pub show_layer_shadows: bool,    // Drop shadows behind floating layers

    // GPU Texture handles
    bg_textures: [Option<TextureHandle>; 4],
    bg_images: [ColorImage; 4],
    obj_composite_texture: Option<TextureHandle>,
    obj_composite_image: ColorImage,
    sprite_atlas_texture: Option<TextureHandle>,
    sprite_atlas_image: ColorImage,
}

impl Default for DioramaRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl DioramaRenderer {
    pub const ATLAS_SIZE: usize = 512;

    pub fn new() -> Self {
        Self {
            camera: OrbitCamera::new(),
            layer_depth_separation: 18.0,
            sprite_depth_elevation: 22.0,
            voxel_relief_strength: 0.0,
            show_diorama_grid: true,
            show_layer_shadows: true,
            bg_textures: [None, None, None, None],
            bg_images: [
                ColorImage::new([SCREEN_WIDTH, SCREEN_HEIGHT], Color32::TRANSPARENT),
                ColorImage::new([SCREEN_WIDTH, SCREEN_HEIGHT], Color32::TRANSPARENT),
                ColorImage::new([SCREEN_WIDTH, SCREEN_HEIGHT], Color32::TRANSPARENT),
                ColorImage::new([SCREEN_WIDTH, SCREEN_HEIGHT], Color32::TRANSPARENT),
            ],
            obj_composite_texture: None,
            obj_composite_image: ColorImage::new([SCREEN_WIDTH, SCREEN_HEIGHT], Color32::TRANSPARENT),
            sprite_atlas_texture: None,
            sprite_atlas_image: ColorImage::new([Self::ATLAS_SIZE, Self::ATLAS_SIZE], Color32::TRANSPARENT),
        }
    }

    /// Updates textures and renders the 3D diorama scene into the egui UI viewport.
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        viewport_rect: Rect,
        diorama_data: &DioramaFrameData,
    ) {
        let center = viewport_rect.center();
        let scale = (viewport_rect.width() / (SCREEN_WIDTH as f32 * 1.5))
            .min(viewport_rect.height() / (SCREEN_HEIGHT as f32 * 1.5))
            .max(0.6);

        // Upload and refresh BG layer textures
        for i in 0..4 {
            let bg = &diorama_data.bg_layers[i];
            let image = &mut self.bg_images[i];
            for (dst, &src) in image.pixels.iter_mut().zip(bg.pixels.iter()) {
                if (src >> 24) != 0 {
                    *dst = Color32::from_rgba_unmultiplied(
                        (src & 0xFF) as u8,
                        ((src >> 8) & 0xFF) as u8,
                        ((src >> 16) & 0xFF) as u8,
                        255,
                    );
                } else {
                    *dst = Color32::TRANSPARENT;
                }
            }

            let tex = self.bg_textures[i].get_or_insert_with(|| {
                ctx.load_texture(format!("diorama_bg_{}", i), image.clone(), TextureOptions::NEAREST)
            });
            tex.set(image.clone(), TextureOptions::NEAREST);
        }

        let bg_tex_ids: [Option<egui::TextureId>; 4] = [
            self.bg_textures[0].as_ref().map(|t| t.id()),
            self.bg_textures[1].as_ref().map(|t| t.id()),
            self.bg_textures[2].as_ref().map(|t| t.id()),
            self.bg_textures[3].as_ref().map(|t| t.id()),
        ];

        // Upload OBJ composite texture
        for (dst, &src) in self.obj_composite_image.pixels.iter_mut().zip(diorama_data.obj_composite.iter()) {
            if (src >> 24) != 0 {
                *dst = Color32::from_rgba_unmultiplied(
                    (src & 0xFF) as u8,
                    ((src >> 8) & 0xFF) as u8,
                    ((src >> 16) & 0xFF) as u8,
                    255,
                );
            } else {
                *dst = Color32::TRANSPARENT;
            }
        }
        let obj_tex_id = {
            let tex = self.obj_composite_texture.get_or_insert_with(|| {
                ctx.load_texture("diorama_obj", self.obj_composite_image.clone(), TextureOptions::NEAREST)
            });
            tex.set(self.obj_composite_image.clone(), TextureOptions::NEAREST);
            tex.id()
        };

        // Pack active individual sprites into dynamic sprite atlas
        self.sprite_atlas_image.pixels.fill(Color32::TRANSPARENT);
        let mut sprite_atlas_uvs: Vec<([Pos2; 4], usize)> = Vec::with_capacity(diorama_data.sprites.len());

        let mut cur_x = 0;
        let mut cur_y = 0;
        let mut row_max_h = 0;

        for (idx, sprite) in diorama_data.sprites.iter().enumerate() {
            if cur_x + sprite.width > Self::ATLAS_SIZE {
                cur_x = 0;
                cur_y += row_max_h + 1;
                row_max_h = 0;
            }
            if cur_y + sprite.height > Self::ATLAS_SIZE {
                break; // Atlas full
            }

            for sy in 0..sprite.height {
                for sx in 0..sprite.width {
                    let p = sprite.pixels[sy * sprite.width + sx];
                    if (p >> 24) != 0 {
                        let dst_idx = (cur_y + sy) * Self::ATLAS_SIZE + (cur_x + sx);
                        self.sprite_atlas_image.pixels[dst_idx] = Color32::from_rgba_unmultiplied(
                            (p & 0xFF) as u8,
                            ((p >> 8) & 0xFF) as u8,
                            ((p >> 16) & 0xFF) as u8,
                            if sprite.is_semi_transparent { 180 } else { 255 },
                        );
                    }
                }
            }

            let inv_size = 1.0 / Self::ATLAS_SIZE as f32;
            let u0 = cur_x as f32 * inv_size;
            let v0 = cur_y as f32 * inv_size;
            let u1 = (cur_x + sprite.width) as f32 * inv_size;
            let v1 = (cur_y + sprite.height) as f32 * inv_size;

            sprite_atlas_uvs.push((
                [
                    Pos2::new(u0, v0),
                    Pos2::new(u1, v0),
                    Pos2::new(u1, v1),
                    Pos2::new(u0, v1),
                ],
                idx,
            ));

            cur_x += sprite.width + 1;
            row_max_h = row_max_h.max(sprite.height);
        }

        let atlas_tex_id = {
            let tex = self.sprite_atlas_texture.get_or_insert_with(|| {
                ctx.load_texture("diorama_sprite_atlas", self.sprite_atlas_image.clone(), TextureOptions::NEAREST)
            });
            tex.set(self.sprite_atlas_image.clone(), TextureOptions::NEAREST);
            tex.id()
        };

        // Collect all quads across layers and sprites
        let mut quads: Vec<ProjectedQuad> = Vec::with_capacity(256);

        // 1. Diorama Pedestal Base / Grid (Z = -10.0)
        if self.show_diorama_grid {
            self.build_pedestal_grid(&mut quads, center, scale, obj_tex_id);
        }

        // 2. Backdrop Plane (Z = 0.0)
        let backdrop_color = Color32::from_rgb(
            diorama_data.backdrop_color[0],
            diorama_data.backdrop_color[1],
            diorama_data.backdrop_color[2],
        );
        self.build_backdrop_quad(&mut quads, center, scale, backdrop_color, obj_tex_id);

        // 3. Background Layers (BG0..3)
        for i in 0..4 {
            let bg = &diorama_data.bg_layers[i];
            if !bg.enabled {
                continue;
            }

            // GBA Hardware Priority: 3 is backmost, 0 is frontmost
            let prio_rank = (3 - bg.priority) as f32;
            let layer_z = 6.0 + prio_rank * self.layer_depth_separation;

            if let Some(tex_id) = bg_tex_ids[i] {
                // Drop shadow
                if self.show_layer_shadows && layer_z > 8.0 {
                    self.build_layer_quad(
                        &mut quads,
                        center,
                        scale,
                        layer_z - 3.0,
                        tex_id,
                        Color32::from_rgba_unmultiplied(0, 0, 0, 70),
                        0.0,
                        &self.bg_images[i],
                    );
                }

                self.build_layer_quad(
                    &mut quads,
                    center,
                    scale,
                    layer_z,
                    tex_id,
                    Color32::WHITE,
                    self.voxel_relief_strength,
                    &self.bg_images[i],
                );
            }
        }

        // 4. Window Plane (Z = between BG and Sprites)
        if diorama_data.window_enabled {
            let win_z = 6.0 + 3.2 * self.layer_depth_separation;
            self.build_window_quad(&mut quads, center, scale, win_z, diorama_data, obj_tex_id);
        }

        // 5. OAM Sprites (Individual 3D floating entities)
        let base_sprite_z = 6.0 + 3.8 * self.layer_depth_separation + self.sprite_depth_elevation;

        if !diorama_data.sprites.is_empty() {
            for &(uvs, idx) in &sprite_atlas_uvs {
                let sprite = &diorama_data.sprites[idx];
                // Sprite priority (0..3) + micro-offset by Y and ID to completely eliminate Z-fighting
                let prio_offset = (3 - sprite.priority) as f32 * 3.5;
                let y_offset = (sprite.y as f32 / SCREEN_HEIGHT as f32) * 1.5;
                let id_offset = (sprite.id as f32) * 0.02;
                let sprite_z = base_sprite_z + prio_offset + y_offset + id_offset;

                // Sprite shadow
                if self.show_layer_shadows {
                    self.build_sprite_quad(
                        &mut quads,
                        center,
                        scale,
                        sprite_z - 4.0,
                        sprite,
                        uvs,
                        atlas_tex_id,
                        Color32::from_rgba_unmultiplied(0, 0, 0, 90),
                    );
                }

                // Sprite quad
                self.build_sprite_quad(
                    &mut quads,
                    center,
                    scale,
                    sprite_z,
                    sprite,
                    uvs,
                    atlas_tex_id,
                    Color32::WHITE,
                );
            }
        } else {
            // Fallback: render composite OBJ quad if individual sprites are empty
            self.build_layer_quad(
                &mut quads,
                center,
                scale,
                base_sprite_z,
                obj_tex_id,
                Color32::WHITE,
                0.0,
                &self.obj_composite_image,
            );
        }

        // 6. Sort all quads back-to-front (Painter's Algorithm)
        quads.sort_by(|a, b| b.avg_cam_z.partial_cmp(&a.avg_cam_z).unwrap_or(std::cmp::Ordering::Equal));

        // 7. Emit sorted quads to egui::Painter
        for quad in quads {
            let mut mesh = egui::Mesh::with_texture(quad.texture_id);
            mesh.vertices.reserve(4);
            mesh.indices.reserve(6);

            let v_base = mesh.vertices.len() as u32;
            for i in 0..4 {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: quad.points[i],
                    uv: quad.uvs[i],
                    color: quad.tint,
                });
            }

            mesh.indices.extend_from_slice(&[
                v_base,
                v_base + 1,
                v_base + 2,
                v_base,
                v_base + 2,
                v_base + 3,
            ]);

            ui.painter().add(Shape::mesh(mesh));
        }

        // Draw 3D screen glass boundary frame
        self.draw_glass_frame(ui, center, scale);
    }

    #[allow(clippy::too_many_arguments)]
    fn build_layer_quad(
        &self,
        quads: &mut Vec<ProjectedQuad>,
        center: Pos2,
        scale: f32,
        z: f32,
        texture_id: egui::TextureId,
        tint: Color32,
        relief: f32,
        image: &ColorImage,
    ) {
        let half_w = SCREEN_WIDTH as f32 / 2.0;
        let half_h = SCREEN_HEIGHT as f32 / 2.0;

        if relief <= 0.001 {
            // Single high-efficiency 240x160 quad
            let (p0, z0) = self.camera.project_point(-half_w, -half_h, z, center, scale);
            let (p1, z1) = self.camera.project_point(half_w, -half_h, z, center, scale);
            let (p2, z2) = self.camera.project_point(half_w, half_h, z, center, scale);
            let (p3, z3) = self.camera.project_point(-half_w, half_h, z, center, scale);

            let avg_cam_z = (z0 + z1 + z2 + z3) * 0.25;

            quads.push(ProjectedQuad {
                points: [p0, p1, p2, p3],
                uvs: [
                    Pos2::new(0.0, 0.0),
                    Pos2::new(1.0, 0.0),
                    Pos2::new(1.0, 1.0),
                    Pos2::new(0.0, 1.0),
                ],
                avg_cam_z,
                texture_id,
                tint,
            });
        } else {
            // 15x10 tile block grid with luminance voxel relief extrusion
            let cols = 15;
            let rows = 10;
            let cell_w = SCREEN_WIDTH as f32 / cols as f32;
            let cell_h = SCREEN_HEIGHT as f32 / rows as f32;

            for r in 0..rows {
                let y0 = -half_h + r as f32 * cell_h;
                let y1 = y0 + cell_h;
                let v0 = r as f32 / rows as f32;
                let v1 = (r + 1) as f32 / rows as f32;

                for c in 0..cols {
                    let x0 = -half_w + c as f32 * cell_w;
                    let x1 = x0 + cell_w;
                    let u0 = c as f32 / cols as f32;
                    let u1 = (c + 1) as f32 / cols as f32;

                    // Sample average luminance of cell
                    let samp_x = (c * (SCREEN_WIDTH / cols) + 8).min(SCREEN_WIDTH - 1);
                    let samp_y = (r * (SCREEN_HEIGHT / rows) + 8).min(SCREEN_HEIGHT - 1);
                    let pixel = image.pixels[samp_y * SCREEN_WIDTH + samp_x];

                    if pixel.a() == 0 {
                        continue; // Skip empty air blocks
                    }

                    let lum = (0.299 * pixel.r() as f32 + 0.587 * pixel.g() as f32 + 0.114 * pixel.b() as f32) / 255.0;
                    let relief_dz = (lum - 0.5) * relief * 12.0;
                    let cell_z = z + relief_dz;

                    let (p0, z0) = self.camera.project_point(x0, y0, cell_z, center, scale);
                    let (p1, z1) = self.camera.project_point(x1, y0, cell_z, center, scale);
                    let (p2, z2) = self.camera.project_point(x1, y1, cell_z, center, scale);
                    let (p3, z3) = self.camera.project_point(x0, y1, cell_z, center, scale);

                    let avg_cam_z = (z0 + z1 + z2 + z3) * 0.25;

                    quads.push(ProjectedQuad {
                        points: [p0, p1, p2, p3],
                        uvs: [
                            Pos2::new(u0, v0),
                            Pos2::new(u1, v0),
                            Pos2::new(u1, v1),
                            Pos2::new(u0, v1),
                        ],
                        avg_cam_z,
                        texture_id,
                        tint,
                    });
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_sprite_quad(
        &self,
        quads: &mut Vec<ProjectedQuad>,
        center: Pos2,
        scale: f32,
        z: f32,
        sprite: &Sprite3D,
        uvs: [Pos2; 4],
        texture_id: egui::TextureId,
        tint: Color32,
    ) {
        let half_w = SCREEN_WIDTH as f32 / 2.0;
        let half_h = SCREEN_HEIGHT as f32 / 2.0;

        let x0 = sprite.x as f32 - half_w;
        let y0 = sprite.y as f32 - half_h;
        let x1 = x0 + sprite.width as f32;
        let y1 = y0 + sprite.height as f32;

        let (p0, z0) = self.camera.project_point(x0, y0, z, center, scale);
        let (p1, z1) = self.camera.project_point(x1, y0, z, center, scale);
        let (p2, z2) = self.camera.project_point(x1, y1, z, center, scale);
        let (p3, z3) = self.camera.project_point(x0, y1, z, center, scale);

        let avg_cam_z = (z0 + z1 + z2 + z3) * 0.25;

        quads.push(ProjectedQuad {
            points: [p0, p1, p2, p3],
            uvs,
            avg_cam_z,
            texture_id,
            tint,
        });
    }

    fn build_backdrop_quad(
        &self,
        quads: &mut Vec<ProjectedQuad>,
        center: Pos2,
        scale: f32,
        color: Color32,
        fallback_tex: egui::TextureId,
    ) {
        let half_w = SCREEN_WIDTH as f32 / 2.0 + 8.0;
        let half_h = SCREEN_HEIGHT as f32 / 2.0 + 8.0;

        let (p0, z0) = self.camera.project_point(-half_w, -half_h, 0.0, center, scale);
        let (p1, z1) = self.camera.project_point(half_w, -half_h, 0.0, center, scale);
        let (p2, z2) = self.camera.project_point(half_w, half_h, 0.0, center, scale);
        let (p3, z3) = self.camera.project_point(-half_w, half_h, 0.0, center, scale);

        let avg_cam_z = (z0 + z1 + z2 + z3) * 0.25;

        quads.push(ProjectedQuad {
            points: [p0, p1, p2, p3],
            uvs: [
                Pos2::new(0.0, 0.0),
                Pos2::new(1.0, 0.0),
                Pos2::new(1.0, 1.0),
                Pos2::new(0.0, 1.0),
            ],
            avg_cam_z,
            texture_id: fallback_tex,
            tint: color,
        });
    }

    fn build_window_quad(
        &self,
        quads: &mut Vec<ProjectedQuad>,
        center: Pos2,
        scale: f32,
        z: f32,
        diorama: &DioramaFrameData,
        tex_id: egui::TextureId,
    ) {
        let half_w = SCREEN_WIDTH as f32 / 2.0;
        let half_h = SCREEN_HEIGHT as f32 / 2.0;

        if let Some(w) = diorama.win0_bounds {
            let x0 = w[0] as f32 - half_w;
            let y0 = w[1] as f32 - half_h;
            let x1 = w[2] as f32 - half_w;
            let y1 = w[3] as f32 - half_h;

            let (p0, z0) = self.camera.project_point(x0, y0, z, center, scale);
            let (p1, z1) = self.camera.project_point(x1, y0, z, center, scale);
            let (p2, z2) = self.camera.project_point(x1, y1, z, center, scale);
            let (p3, z3) = self.camera.project_point(x0, y1, z, center, scale);

            quads.push(ProjectedQuad {
                points: [p0, p1, p2, p3],
                uvs: [Pos2::ZERO, Pos2::ZERO, Pos2::ZERO, Pos2::ZERO],
                avg_cam_z: (z0 + z1 + z2 + z3) * 0.25,
                texture_id: tex_id,
                tint: Color32::from_rgba_unmultiplied(255, 255, 255, 25), // Subtle translucent glass
            });
        }
    }

    fn build_pedestal_grid(&self, quads: &mut Vec<ProjectedQuad>, center: Pos2, scale: f32, tex_id: egui::TextureId) {
        let grid_size = 180.0;
        let grid_z = -12.0;

        let (p0, z0) = self.camera.project_point(-grid_size, -grid_size * 0.8, grid_z, center, scale);
        let (p1, z1) = self.camera.project_point(grid_size, -grid_size * 0.8, grid_z, center, scale);
        let (p2, z2) = self.camera.project_point(grid_size, grid_size * 0.8, grid_z, center, scale);
        let (p3, z3) = self.camera.project_point(-grid_size, grid_size * 0.8, grid_z, center, scale);

        quads.push(ProjectedQuad {
            points: [p0, p1, p2, p3],
            uvs: [Pos2::ZERO, Pos2::ZERO, Pos2::ZERO, Pos2::ZERO],
            avg_cam_z: (z0 + z1 + z2 + z3) * 0.25 + 50.0, // Backmost
            texture_id: tex_id,
            tint: Color32::from_rgba_unmultiplied(15, 18, 25, 230),
        });
    }

    fn draw_glass_frame(&self, ui: &egui::Ui, center: Pos2, scale: f32) {
        let half_w = SCREEN_WIDTH as f32 / 2.0;
        let half_h = SCREEN_HEIGHT as f32 / 2.0;

        let (p0, _) = self.camera.project_point(-half_w, -half_h, 0.0, center, scale);
        let (p1, _) = self.camera.project_point(half_w, -half_h, 0.0, center, scale);
        let (p2, _) = self.camera.project_point(half_w, half_h, 0.0, center, scale);
        let (p3, _) = self.camera.project_point(-half_w, half_h, 0.0, center, scale);

        let stroke = Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(100, 180, 255, 120));
        ui.painter().line_segment([p0, p1], stroke);
        ui.painter().line_segment([p1, p2], stroke);
        ui.painter().line_segment([p2, p3], stroke);
        ui.painter().line_segment([p3, p0], stroke);
    }
}
