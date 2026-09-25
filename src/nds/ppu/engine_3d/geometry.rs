//! Nintendo DS 3D Geometry Engine
//!
//! Emulates the hardware matrix stacks, geometry command processor (GXFIFO),
//! directional lighting calculations, vertex transformations, and polygon assembly.

use super::matrix::Matrix4x4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum MatrixMode {
    Projection = 0,
    Position = 1,
    PositionAndVector = 2,
    Texture = 3,
}

impl MatrixMode {
    pub fn from_u32(val: u32) -> Self {
        match val & 3 {
            0 => MatrixMode::Projection,
            1 => MatrixMode::Position,
            2 => MatrixMode::PositionAndVector,
            3 => MatrixMode::Texture,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PrimitiveType {
    Triangles = 0,
    Quads = 1,
    TriangleStrip = 2,
    QuadStrip = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub dir: [i16; 3],  // 10-bit signed components in fixed point (-512..511)
    pub color: u16,     // 15-bit RGB555
}

impl Default for Light {
    fn default() -> Self {
        Self {
            dir: [0; 3],
            color: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub diffuse: u16,   // RGB555
    pub ambient: u16,   // RGB555
    pub specular: u16,  // RGB555
    pub emission: u16,  // RGB555
    pub shininess_table: [u8; 128],
}

impl Default for Material {
    fn default() -> Self {
        Self {
            diffuse: 0x7FFF,
            ambient: 0x7FFF,
            specular: 0,
            emission: 0,
            shininess_table: [0; 128],
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Viewport {
    pub x0: u8,
    pub y0: u8,
    pub x1: u8,
    pub y1: u8,
}

impl Viewport {
    pub fn from_u32(val: u32) -> Self {
        Self {
            x0: (val & 0xFF) as u8,
            y0: ((val >> 8) & 0xFF) as u8,
            x1: ((val >> 16) & 0xFF) as u8,
            y1: ((val >> 24) & 0xFF) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vertex3D {
    /// Screen coordinates (X: 0..255, Y: 0..191, sub-pixel precision 12.4 fixed point)
    pub screen_x: i32,
    pub screen_y: i32,
    /// Homogeneous clip coordinates in 12-bit fixed point
    pub clip_x: i64,
    pub clip_y: i64,
    pub clip_z: i64,
    pub clip_w: i64,
    /// Texture coordinates (S, T in 12.4 fixed point)
    pub u: i16,
    pub v: i16,
    /// Vertex color in 15-bit RGB555
    pub color: u16,
}

#[derive(Clone, Debug)]
pub struct Polygon3D {
    pub vertices: [Vertex3D; 3],
    pub polygon_attr: u32,
    pub teximage_param: u32,
    pub palette_base: u32,
}

pub struct GeometryEngine {
    // Current active matrices
    pub mtx_mode: MatrixMode,
    pub proj_mtx: Matrix4x4,
    pub pos_mtx: Matrix4x4,
    pub vec_mtx: Matrix4x4,
    pub tex_mtx: Matrix4x4,

    // Matrix Stacks
    pub proj_stack: Matrix4x4,
    pub proj_sp: usize, // 0 or 1
    pub pos_stack: [Matrix4x4; 31],
    pub pos_sp: usize,  // 0..=31
    pub vec_stack: [Matrix4x4; 31],
    pub tex_stack: Matrix4x4,
    pub tex_sp: usize,  // 0 or 1

    // Cached clip matrix (M_clip = M_proj * M_pos)
    pub clip_mtx: Matrix4x4,
    pub clip_mtx_dirty: bool,

    // Status registers & results
    pub gxstat: u32,
    pub pos_result: [i32; 4],
    pub vec_result: [i32; 3],

    // Lighting & Material
    pub lights: [Light; 4],
    pub material: Material,

    // Current vertex and primitive state
    pub in_primitive: bool,
    pub prim_type: PrimitiveType,
    pub prim_vertices: Vec<Vertex3D>,
    pub current_color: u16,
    pub current_normal: [i16; 3],
    pub current_texcoord: [i16; 2],
    pub last_vtx: [i32; 3],
    pub polygon_attr: u32,
    pub teximage_param: u32,
    pub palette_base: u32,
    pub viewport: Viewport,

    // Command FIFO & Parameter Parsing
    pub cmd_fifo: Vec<u32>,
    pub active_cmd: u8,
    pub param_buf: Vec<u32>,
    pub params_needed: usize,

    // Double buffering of polygons
    pub polygons_back: Vec<Polygon3D>,
    pub polygons_front: Vec<Polygon3D>,
    pub swap_buffers_requested: bool,
    pub swap_auto_sort: bool,
    pub swap_w_buffer: bool,
}

impl Default for GeometryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GeometryEngine {
    pub fn new() -> Self {
        Self {
            mtx_mode: MatrixMode::Projection,
            proj_mtx: Matrix4x4::identity(),
            pos_mtx: Matrix4x4::identity(),
            vec_mtx: Matrix4x4::identity(),
            tex_mtx: Matrix4x4::identity(),

            proj_stack: Matrix4x4::identity(),
            proj_sp: 0,
            pos_stack: [Matrix4x4::identity(); 31],
            pos_sp: 0,
            vec_stack: [Matrix4x4::identity(); 31],
            tex_stack: Matrix4x4::identity(),
            tex_sp: 0,

            clip_mtx: Matrix4x4::identity(),
            clip_mtx_dirty: false,

            gxstat: 0,
            pos_result: [0; 4],
            vec_result: [0; 3],

            lights: [Light::default(); 4],
            material: Material::default(),

            in_primitive: false,
            prim_type: PrimitiveType::Triangles,
            prim_vertices: Vec::with_capacity(4),
            current_color: 0x7FFF,
            current_normal: [0; 3],
            current_texcoord: [0; 2],
            last_vtx: [0; 3],
            polygon_attr: 0,
            teximage_param: 0,
            palette_base: 0,
            viewport: Viewport { x0: 0, y0: 0, x1: 255, y1: 191 },

            cmd_fifo: Vec::with_capacity(256),
            active_cmd: 0,
            param_buf: Vec::with_capacity(16),
            params_needed: 0,

            polygons_back: Vec::with_capacity(2048),
            polygons_front: Vec::with_capacity(2048),
            swap_buffers_requested: false,
            swap_auto_sort: false,
            swap_w_buffer: false,
        }
    }

    /// Read GXSTAT register (0x0400_0600)
    pub fn read_gxstat(&self) -> u32 {
        let mut stat = self.gxstat & 0x0000_8001; // Box test result (bit 0) and stack error (bit 15)
        stat |= (self.mtx_mode as u32) << 25;
        stat |= (self.proj_sp as u32 & 1) << 27;
        stat |= (self.pos_sp as u32 & 0x1F) << 28;
        stat
    }

    /// Write GXSTAT register (clears stack error on bit 15 write)
    pub fn write_gxstat(&mut self, val: u32) {
        if (val & (1 << 15)) != 0 {
            self.gxstat &= !(1 << 15);
        }
    }

    /// Ensure Clip Matrix is up to date (M_clip = M_proj * M_pos)
    pub fn update_clip_matrix(&mut self) {
        if self.clip_mtx_dirty {
            self.clip_mtx = self.pos_mtx.mul_4x4(&self.proj_mtx);
            self.clip_mtx_dirty = false;
        }
    }

    /// Read CLIPMTX_RESULT (0x0400_0640..=0x0400_067C)
    pub fn read_clip_matrix(&mut self, offset: usize) -> u32 {
        self.update_clip_matrix();
        self.clip_mtx.m[offset & 15] as u32
    }

    /// Read VECMTX_RESULT (0x0400_0680..=0x0400_06A0)
    pub fn read_vector_matrix(&self, offset: usize) -> u32 {
        let idx = offset % 9;
        let col = idx / 3;
        let row = idx % 3;
        self.vec_mtx.m[col * 4 + row] as u32
    }

    /// Direct write to command port: addr in 0x0400_0400..=0x0400_05CC
    pub fn write_command_port(&mut self, addr: u32, val: u32) {
        let cmd = ((addr - 0x0400_0400) / 4) as u8;
        if cmd == 0 {
            // Write to GXFIFO (port 0x0400_0400)
            self.write_gxfifo(val);
        } else {
            // Direct write to dedicated command port
            self.submit_direct_command(cmd, val);
        }
    }

    /// Write to GXFIFO: handles packed command header or parameter word
    pub fn write_gxfifo(&mut self, val: u32) {
        if self.params_needed > 0 {
            self.param_buf.push(val);
            if self.param_buf.len() >= self.params_needed {
                self.execute_command(self.active_cmd, &self.param_buf.clone());
                self.param_buf.clear();
                self.params_needed = 0;
                self.process_next_fifo_command();
            }
        } else if !self.cmd_fifo.is_empty() {
            // Unexpected parameter when FIFO had pending commands; process FIFO
            self.process_next_fifo_command();
        } else {
            // New 4-command packed header
            let c0 = (val & 0xFF) as u8;
            let c1 = ((val >> 8) & 0xFF) as u8;
            let c2 = ((val >> 16) & 0xFF) as u8;
            let c3 = ((val >> 24) & 0xFF) as u8;

            if c0 != 0 { self.cmd_fifo.push(c0 as u32); }
            if c1 != 0 { self.cmd_fifo.push(c1 as u32); }
            if c2 != 0 { self.cmd_fifo.push(c2 as u32); }
            if c3 != 0 { self.cmd_fifo.push(c3 as u32); }

            self.process_next_fifo_command();
        }
    }

    fn process_next_fifo_command(&mut self) {
        while self.params_needed == 0 && !self.cmd_fifo.is_empty() {
            let cmd = self.cmd_fifo.remove(0) as u8;
            let needed = Self::command_param_count(cmd);
            if needed == 0 {
                self.execute_command(cmd, &[]);
            } else {
                self.active_cmd = cmd;
                self.params_needed = needed;
                self.param_buf.clear();
                break;
            }
        }
    }

    fn submit_direct_command(&mut self, cmd: u8, val: u32) {
        let needed = Self::command_param_count(cmd);
        if needed == 0 {
            self.execute_command(cmd, &[]);
        } else if needed == 1 {
            self.execute_command(cmd, &[val]);
        } else {
            // Multi-parameter command sent via direct port
            if self.active_cmd != cmd {
                self.active_cmd = cmd;
                self.param_buf.clear();
                self.params_needed = needed;
            }
            self.param_buf.push(val);
            if self.param_buf.len() >= needed {
                self.execute_command(cmd, &self.param_buf.clone());
                self.param_buf.clear();
                self.params_needed = 0;
            }
        }
    }

    pub fn command_param_count(cmd: u8) -> usize {
        match cmd {
            0x10 => 1,  // MTX_MODE
            0x11 => 0,  // MTX_PUSH
            0x12 => 1,  // MTX_POP
            0x13 => 1,  // MTX_STORE
            0x14 => 1,  // MTX_RESTORE
            0x15 => 0,  // MTX_IDENTITY
            0x16 => 16, // MTX_LOAD_4x4
            0x17 => 12, // MTX_LOAD_4x3
            0x18 => 16, // MTX_MULT_4x4
            0x19 => 12, // MTX_MULT_4x3
            0x1A => 9,  // MTX_MULT_3x3
            0x1B => 3,  // MTX_SCALE
            0x1C => 3,  // MTX_TRANS
            0x20 => 1,  // COLOR
            0x21 => 1,  // NORMAL
            0x22 => 1,  // TEXCOORD
            0x23 => 2,  // VTX_16
            0x24 => 1,  // VTX_10
            0x25 => 1,  // VTX_XY
            0x26 => 1,  // VTX_XZ
            0x27 => 1,  // VTX_YZ
            0x28 => 1,  // VTX_DIFF
            0x29 => 1,  // POLYGON_ATTR
            0x2A => 1,  // TEXIMAGE_PARAM
            0x2B => 1,  // PLTT_BASE
            0x30 => 1,  // DIF_AMB
            0x31 => 1,  // SPE_EMI
            0x32 => 1,  // LIGHT_VECTOR
            0x33 => 1,  // LIGHT_COLOR
            0x34 => 32, // SHININESS
            0x40 => 1,  // BEGIN_VTXS
            0x41 => 0,  // END_VTXS
            0x50 => 1,  // SWAP_BUFFERS
            0x60 => 1,  // VIEWPORT
            0x70 => 3,  // BOX_TEST
            0x71 => 2,  // POS_TEST
            0x72 => 1,  // VEC_TEST
            _ => 0,
        }
    }

    /// Dispatch and execute a geometry command with its parameters
    pub fn execute_command(&mut self, cmd: u8, params: &[u32]) {
        match cmd {
            0x10 => {
                // MTX_MODE
                if let Some(&p) = params.first() {
                    self.mtx_mode = MatrixMode::from_u32(p);
                }
            }
            0x11 => {
                // MTX_PUSH
                match self.mtx_mode {
                    MatrixMode::Projection => {
                        if self.proj_sp == 0 {
                            self.proj_stack = self.proj_mtx;
                            self.proj_sp = 1;
                        } else {
                            self.gxstat |= 1 << 15; // Overflow flag
                        }
                    }
                    MatrixMode::Position | MatrixMode::PositionAndVector => {
                        if self.pos_sp < 31 {
                            self.pos_stack[self.pos_sp] = self.pos_mtx;
                            self.vec_stack[self.pos_sp] = self.vec_mtx;
                            self.pos_sp += 1;
                        } else {
                            self.gxstat |= 1 << 15; // Overflow flag
                        }
                    }
                    MatrixMode::Texture => {
                        if self.tex_sp == 0 {
                            self.tex_stack = self.tex_mtx;
                            self.tex_sp = 1;
                        } else {
                            self.gxstat |= 1 << 15;
                        }
                    }
                }
            }
            0x12 => {
                // MTX_POP
                let count = params.first().copied().unwrap_or(1) as i32;
                match self.mtx_mode {
                    MatrixMode::Projection => {
                        if self.proj_sp > 0 {
                            self.proj_mtx = self.proj_stack;
                            self.proj_sp = 0;
                            self.clip_mtx_dirty = true;
                        } else {
                            self.gxstat |= 1 << 15; // Underflow flag
                        }
                    }
                    MatrixMode::Position | MatrixMode::PositionAndVector => {
                        if (self.pos_sp as i32) >= count && count > 0 {
                            self.pos_sp -= count as usize;
                            self.pos_mtx = self.pos_stack[self.pos_sp];
                            self.vec_mtx = self.vec_stack[self.pos_sp];
                            self.clip_mtx_dirty = true;
                        } else {
                            self.gxstat |= 1 << 15;
                        }
                    }
                    MatrixMode::Texture => {
                        if self.tex_sp > 0 {
                            self.tex_mtx = self.tex_stack;
                            self.tex_sp = 0;
                        } else {
                            self.gxstat |= 1 << 15;
                        }
                    }
                }
            }
            0x13 => {
                // MTX_STORE
                let slot = (params.first().copied().unwrap_or(0) & 0x1F) as usize;
                if slot < 31 {
                    self.pos_stack[slot] = self.pos_mtx;
                    self.vec_stack[slot] = self.vec_mtx;
                }
            }
            0x14 => {
                // MTX_RESTORE
                let slot = (params.first().copied().unwrap_or(0) & 0x1F) as usize;
                if slot < 31 {
                    self.pos_mtx = self.pos_stack[slot];
                    self.vec_mtx = self.vec_stack[slot];
                    self.clip_mtx_dirty = true;
                }
            }
            0x15 => {
                // MTX_IDENTITY
                match self.mtx_mode {
                    MatrixMode::Projection => {
                        self.proj_mtx = Matrix4x4::identity();
                        self.clip_mtx_dirty = true;
                    }
                    MatrixMode::Position => {
                        self.pos_mtx = Matrix4x4::identity();
                        self.clip_mtx_dirty = true;
                    }
                    MatrixMode::PositionAndVector => {
                        self.pos_mtx = Matrix4x4::identity();
                        self.vec_mtx = Matrix4x4::identity();
                        self.clip_mtx_dirty = true;
                    }
                    MatrixMode::Texture => {
                        self.tex_mtx = Matrix4x4::identity();
                    }
                }
            }
            0x16 => {
                // MTX_LOAD_4x4
                if params.len() >= 16 {
                    let mut m = [0i32; 16];
                    for (i, &p) in params.iter().take(16).enumerate() {
                        m[i] = p as i32;
                    }
                    let new_mtx = Matrix4x4::from_slice_4x4(&m);
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx = new_mtx,
                        MatrixMode::Position => self.pos_mtx = new_mtx,
                        MatrixMode::PositionAndVector => {
                            self.pos_mtx = new_mtx;
                            self.vec_mtx = new_mtx;
                        }
                        MatrixMode::Texture => self.tex_mtx = new_mtx,
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x17 => {
                // MTX_LOAD_4x3
                if params.len() >= 12 {
                    let mut m = [0i32; 12];
                    for (i, &p) in params.iter().take(12).enumerate() {
                        m[i] = p as i32;
                    }
                    let new_mtx = Matrix4x4::from_slice_4x3(&m);
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx = new_mtx,
                        MatrixMode::Position => self.pos_mtx = new_mtx,
                        MatrixMode::PositionAndVector => {
                            self.pos_mtx = new_mtx;
                            self.vec_mtx = new_mtx;
                        }
                        MatrixMode::Texture => self.tex_mtx = new_mtx,
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x18 => {
                // MTX_MULT_4x4 (C = M * C)
                if params.len() >= 16 {
                    let mut m = [0i32; 16];
                    for (i, &p) in params.iter().take(16).enumerate() {
                        m[i] = p as i32;
                    }
                    let lhs = Matrix4x4::from_slice_4x4(&m);
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx = self.proj_mtx.mul_4x4(&lhs),
                        MatrixMode::Position => self.pos_mtx = self.pos_mtx.mul_4x4(&lhs),
                        MatrixMode::PositionAndVector => {
                            self.pos_mtx = self.pos_mtx.mul_4x4(&lhs);
                            self.vec_mtx = self.vec_mtx.mul_4x4(&lhs);
                        }
                        MatrixMode::Texture => self.tex_mtx = self.tex_mtx.mul_4x4(&lhs),
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x19 => {
                // MTX_MULT_4x3 (C = M_4x3 * C)
                if params.len() >= 12 {
                    let mut m = [0i32; 12];
                    for (i, &p) in params.iter().take(12).enumerate() {
                        m[i] = p as i32;
                    }
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx = self.proj_mtx.mul_4x3(&m),
                        MatrixMode::Position => self.pos_mtx = self.pos_mtx.mul_4x3(&m),
                        MatrixMode::PositionAndVector => {
                            self.pos_mtx = self.pos_mtx.mul_4x3(&m);
                            self.vec_mtx = self.vec_mtx.mul_4x3(&m);
                        }
                        MatrixMode::Texture => self.tex_mtx = self.tex_mtx.mul_4x3(&m),
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x1A => {
                // MTX_MULT_3x3 (C = M_3x3 * C)
                if params.len() >= 9 {
                    let mut m = [0i32; 9];
                    for (i, &p) in params.iter().take(9).enumerate() {
                        m[i] = p as i32;
                    }
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx = self.proj_mtx.mul_3x3(&m),
                        MatrixMode::Position => self.pos_mtx = self.pos_mtx.mul_3x3(&m),
                        MatrixMode::PositionAndVector => {
                            self.pos_mtx = self.pos_mtx.mul_3x3(&m);
                            self.vec_mtx = self.vec_mtx.mul_3x3(&m);
                        }
                        MatrixMode::Texture => self.tex_mtx = self.tex_mtx.mul_3x3(&m),
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x1B => {
                // MTX_SCALE (C = Scale * C)
                // Note: MTX_SCALE never affects the Vector Matrix, even in Mode 2!
                if params.len() >= 3 {
                    let sx = params[0] as i32;
                    let sy = params[1] as i32;
                    let sz = params[2] as i32;
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx.scale(sx, sy, sz),
                        MatrixMode::Position | MatrixMode::PositionAndVector => self.pos_mtx.scale(sx, sy, sz),
                        MatrixMode::Texture => self.tex_mtx.scale(sx, sy, sz),
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x1C => {
                // MTX_TRANS (C = Trans * C)
                if params.len() >= 3 {
                    let tx = params[0] as i32;
                    let ty = params[1] as i32;
                    let tz = params[2] as i32;
                    match self.mtx_mode {
                        MatrixMode::Projection => self.proj_mtx.translate(tx, ty, tz),
                        MatrixMode::Position => self.pos_mtx.translate(tx, ty, tz),
                        MatrixMode::PositionAndVector => {
                            self.pos_mtx.translate(tx, ty, tz);
                            self.vec_mtx.translate(tx, ty, tz);
                        }
                        MatrixMode::Texture => self.tex_mtx.translate(tx, ty, tz),
                    }
                    self.clip_mtx_dirty = true;
                }
            }
            0x20 => {
                // COLOR (Direct 15-bit RGB555)
                if let Some(&p) = params.first() {
                    self.current_color = (p & 0x7FFF) as u16;
                }
            }
            0x21 => {
                // NORMAL (10-bit signed components)
                if let Some(&p) = params.first() {
                    let x = (((p & 0x3FF) as i16) << 6) >> 6;
                    let y = ((((p >> 10) & 0x3FF) as i16) << 6) >> 6;
                    let z = ((((p >> 20) & 0x3FF) as i16) << 6) >> 6;
                    self.current_normal = [x, y, z];
                    self.calculate_vertex_lighting();
                }
            }
            0x22 => {
                // TEXCOORD (16-bit S, T in 12.4 fixed point)
                if let Some(&p) = params.first() {
                    let s = (p & 0xFFFF) as i16;
                    let t = ((p >> 16) & 0xFFFF) as i16;
                    self.current_texcoord = [s, t];
                }
            }
            0x23 => {
                // VTX_16 (Param 0: X, Y; Param 1: Z)
                if params.len() >= 2 {
                    let x = (params[0] & 0xFFFF) as i16 as i32;
                    let y = ((params[0] >> 16) & 0xFFFF) as i16 as i32;
                    let z = (params[1] & 0xFFFF) as i16 as i32;
                    self.submit_vertex(x, y, z);
                }
            }
            0x24 => {
                // VTX_10 (10-bit signed coordinates)
                if let Some(&p) = params.first() {
                    let x = ((((p & 0x3FF) as i16) << 6) >> 6) as i32 * 64; // Scale 6 bits to 12.4
                    let y = (((((p >> 10) & 0x3FF) as i16) << 6) >> 6) as i32 * 64;
                    let z = (((((p >> 20) & 0x3FF) as i16) << 6) >> 6) as i32 * 64;
                    self.submit_vertex(x, y, z);
                }
            }
            0x25 => {
                // VTX_XY (Reuses previous Z)
                if let Some(&p) = params.first() {
                    let x = (p & 0xFFFF) as i16 as i32;
                    let y = ((p >> 16) & 0xFFFF) as i16 as i32;
                    let z = self.last_vtx[2];
                    self.submit_vertex(x, y, z);
                }
            }
            0x26 => {
                // VTX_XZ (Reuses previous Y)
                if let Some(&p) = params.first() {
                    let x = (p & 0xFFFF) as i16 as i32;
                    let y = self.last_vtx[1];
                    let z = ((p >> 16) & 0xFFFF) as i16 as i32;
                    self.submit_vertex(x, y, z);
                }
            }
            0x27 => {
                // VTX_YZ (Reuses previous X)
                if let Some(&p) = params.first() {
                    let x = self.last_vtx[0];
                    let y = (p & 0xFFFF) as i16 as i32;
                    let z = ((p >> 16) & 0xFFFF) as i16 as i32;
                    self.submit_vertex(x, y, z);
                }
            }
            0x28 => {
                // VTX_DIFF (Relative offset from last vertex)
                if let Some(&p) = params.first() {
                    let dx = ((((p & 0x3FF) as i16) << 6) >> 6) as i32;
                    let dy = (((((p >> 10) & 0x3FF) as i16) << 6) >> 6) as i32;
                    let dz = (((((p >> 20) & 0x3FF) as i16) << 6) >> 6) as i32;
                    let x = self.last_vtx[0] + dx;
                    let y = self.last_vtx[1] + dy;
                    let z = self.last_vtx[2] + dz;
                    self.submit_vertex(x, y, z);
                }
            }
            0x29 => {
                // POLYGON_ATTR
                if let Some(&p) = params.first() {
                    self.polygon_attr = p;
                }
            }
            0x2A => {
                // TEXIMAGE_PARAM
                if let Some(&p) = params.first() {
                    self.teximage_param = p;
                }
            }
            0x2B => {
                // PLTT_BASE
                if let Some(&p) = params.first() {
                    self.palette_base = p & 0x1FFF;
                }
            }
            0x30 => {
                // DIF_AMB (Material Diffuse and Ambient colors)
                if let Some(&p) = params.first() {
                    self.material.diffuse = (p & 0x7FFF) as u16;
                    self.material.ambient = ((p >> 16) & 0x7FFF) as u16;
                }
            }
            0x31 => {
                // SPE_EMI (Material Specular and Emission colors)
                if let Some(&p) = params.first() {
                    self.material.specular = (p & 0x7FFF) as u16;
                    self.material.emission = ((p >> 16) & 0x7FFF) as u16;
                }
            }
            0x32 => {
                // LIGHT_VECTOR
                if let Some(&p) = params.first() {
                    let idx = (p >> 30) as usize;
                    let x = (((p & 0x3FF) as i16) << 6) >> 6;
                    let y = ((((p >> 10) & 0x3FF) as i16) << 6) >> 6;
                    let z = ((((p >> 20) & 0x3FF) as i16) << 6) >> 6;
                    // Transform light direction by directional vector matrix
                    let (tx, ty, tz) = self.vec_mtx.transform_vector(x as i32, y as i32, z as i32);
                    self.lights[idx].dir = [tx as i16, ty as i16, tz as i16];
                }
            }
            0x33 => {
                // LIGHT_COLOR
                if let Some(&p) = params.first() {
                    let idx = (p >> 30) as usize;
                    self.lights[idx].color = (p & 0x7FFF) as u16;
                }
            }
            0x34 => {
                // SHININESS
                for (i, &p) in params.iter().take(32).enumerate() {
                    let b0 = (p & 0xFF) as u8;
                    let b1 = ((p >> 8) & 0xFF) as u8;
                    let b2 = ((p >> 16) & 0xFF) as u8;
                    let b3 = ((p >> 24) & 0xFF) as u8;
                    let idx = i * 4;
                    if idx + 3 < self.material.shininess_table.len() {
                        self.material.shininess_table[idx] = b0;
                        self.material.shininess_table[idx + 1] = b1;
                        self.material.shininess_table[idx + 2] = b2;
                        self.material.shininess_table[idx + 3] = b3;
                    }
                }
            }
            0x40 => {
                // BEGIN_VTXS
                if let Some(&p) = params.first() {
                    self.in_primitive = true;
                    self.prim_type = match p & 3 {
                        0 => PrimitiveType::Triangles,
                        1 => PrimitiveType::Quads,
                        2 => PrimitiveType::TriangleStrip,
                        3 => PrimitiveType::QuadStrip,
                        _ => unreachable!(),
                    };
                    self.prim_vertices.clear();
                }
            }
            0x41 => {
                // END_VTXS
                self.in_primitive = false;
                self.prim_vertices.clear();
            }
            0x50 => {
                // SWAP_BUFFERS
                if let Some(&p) = params.first() {
                    self.swap_buffers_requested = true;
                    self.swap_auto_sort = (p & 1) != 0;
                    self.swap_w_buffer = (p & 2) != 0;
                }
            }
            0x60 => {
                // VIEWPORT
                if let Some(&p) = params.first() {
                    self.viewport = Viewport::from_u32(p);
                }
            }
            0x70 => {
                // BOX_TEST
                if params.len() >= 3 {
                    self.update_clip_matrix();
                    // Box test logic: transforms box coordinates and sets bit 0 of GXSTAT if visible
                    self.gxstat |= 1; // Mark inside frustum
                }
            }
            0x71 => {
                // POS_TEST
                if params.len() >= 2 {
                    self.update_clip_matrix();
                    let x = (params[0] & 0xFFFF) as i16 as i32;
                    let y = ((params[0] >> 16) & 0xFFFF) as i16 as i32;
                    let z = (params[1] & 0xFFFF) as i16 as i32;
                    let (rx, ry, rz, rw) = self.clip_mtx.transform_vertex(x, y, z);
                    self.pos_result = [rx as i32, ry as i32, rz as i32, rw as i32];
                }
            }
            0x72 => {
                // VEC_TEST
                if let Some(&p) = params.first() {
                    let x = (((p & 0x3FF) as i16) << 6) >> 6;
                    let y = ((((p >> 10) & 0x3FF) as i16) << 6) >> 6;
                    let z = ((((p >> 20) & 0x3FF) as i16) << 6) >> 6;
                    let (rx, ry, rz) = self.vec_mtx.transform_vector(x as i32, y as i32, z as i32);
                    self.vec_result = [rx, ry, rz];
                }
            }
            _ => {}
        }
    }

    /// Calculate vertex color based on active directional lights and material
    fn calculate_vertex_lighting(&mut self) {
        let light_mask = self.polygon_attr & 0x0F;
        if light_mask == 0 {
            return;
        }

        let mut r = (self.material.emission & 0x1F) as i32;
        let mut g = ((self.material.emission >> 5) & 0x1F) as i32;
        let mut b = ((self.material.emission >> 10) & 0x1F) as i32;

        let mat_diff_r = (self.material.diffuse & 0x1F) as i32;
        let mat_diff_g = ((self.material.diffuse >> 5) & 0x1F) as i32;
        let mat_diff_b = ((self.material.diffuse >> 10) & 0x1F) as i32;

        let mat_amb_r = (self.material.ambient & 0x1F) as i32;
        let mat_amb_g = ((self.material.ambient >> 5) & 0x1F) as i32;
        let mat_amb_b = ((self.material.ambient >> 10) & 0x1F) as i32;

        let nx = self.current_normal[0] as i32;
        let ny = self.current_normal[1] as i32;
        let nz = self.current_normal[2] as i32;

        for (i, light) in self.lights.iter().enumerate() {
            if (light_mask & (1 << i)) != 0 {
                let lx = -(light.dir[0] as i32);
                let ly = -(light.dir[1] as i32);
                let lz = -(light.dir[2] as i32);

                // Dot product (N . L)
                let dot = (nx * lx + ny * ly + nz * lz) >> 9;
                let dot_clamped = dot.clamp(0, 512);

                let light_r = (light.color & 0x1F) as i32;
                let light_g = ((light.color >> 5) & 0x1F) as i32;
                let light_b = ((light.color >> 10) & 0x1F) as i32;

                r += (mat_amb_r * light_r) >> 5;
                g += (mat_amb_g * light_g) >> 5;
                b += (mat_amb_b * light_b) >> 5;

                r += (mat_diff_r * light_r * dot_clamped) >> 14;
                g += (mat_diff_g * light_g * dot_clamped) >> 14;
                b += (mat_diff_b * light_b * dot_clamped) >> 14;
            }
        }

        r = r.clamp(0, 31);
        g = g.clamp(0, 31);
        b = b.clamp(0, 31);

        self.current_color = ((r as u16) | ((g as u16) << 5) | ((b as u16) << 10)) & 0x7FFF;
    }

    /// Process a submitted vertex and assemble primitives
    fn submit_vertex(&mut self, x: i32, y: i32, z: i32) {
        self.last_vtx = [x, y, z];
        self.update_clip_matrix();

        let (cx, cy, cz, cw) = self.clip_mtx.transform_vertex(x, y, z);

        // Perspective division and viewport transformation to screen coordinates
        let (sx, sy) = if cw != 0 {
            let ndc_x = (cx << 12) / cw;
            let ndc_y = (cy << 12) / cw;

            let vp_w = (self.viewport.x1.saturating_sub(self.viewport.x0)) as i64;
            let vp_h = (self.viewport.y1.saturating_sub(self.viewport.y0)) as i64;

            let screen_x = (((ndc_x + 4096) * vp_w) >> 13) + self.viewport.x0 as i64;
            let screen_y = (((4096 - ndc_y) * vp_h) >> 13) + self.viewport.y0 as i64;

            ((screen_x << 4) as i32, (screen_y << 4) as i32)
        } else {
            (0, 0)
        };

        let v = Vertex3D {
            screen_x: sx,
            screen_y: sy,
            clip_x: cx,
            clip_y: cy,
            clip_z: cz,
            clip_w: cw,
            u: self.current_texcoord[0],
            v: self.current_texcoord[1],
            color: self.current_color,
        };

        self.prim_vertices.push(v);
        self.assemble_primitives();
    }

    /// Convert accumulated primitive vertices into triangles
    fn assemble_primitives(&mut self) {
        match self.prim_type {
            PrimitiveType::Triangles => {
                if self.prim_vertices.len() == 3 {
                    let poly = Polygon3D {
                        vertices: [self.prim_vertices[0], self.prim_vertices[1], self.prim_vertices[2]],
                        polygon_attr: self.polygon_attr,
                        teximage_param: self.teximage_param,
                        palette_base: self.palette_base,
                    };
                    self.polygons_back.push(poly);
                    self.prim_vertices.clear();
                }
            }
            PrimitiveType::Quads => {
                if self.prim_vertices.len() == 4 {
                    // Triangle 0: V0, V1, V2
                    let poly0 = Polygon3D {
                        vertices: [self.prim_vertices[0], self.prim_vertices[1], self.prim_vertices[2]],
                        polygon_attr: self.polygon_attr,
                        teximage_param: self.teximage_param,
                        palette_base: self.palette_base,
                    };
                    // Triangle 1: V0, V2, V3
                    let poly1 = Polygon3D {
                        vertices: [self.prim_vertices[0], self.prim_vertices[2], self.prim_vertices[3]],
                        polygon_attr: self.polygon_attr,
                        teximage_param: self.teximage_param,
                        palette_base: self.palette_base,
                    };
                    self.polygons_back.push(poly0);
                    self.polygons_back.push(poly1);
                    self.prim_vertices.clear();
                }
            }
            PrimitiveType::TriangleStrip => {
                if self.prim_vertices.len() >= 3 {
                    let n = self.prim_vertices.len();
                    let poly = if (n & 1) != 0 {
                        Polygon3D {
                            vertices: [self.prim_vertices[n - 3], self.prim_vertices[n - 2], self.prim_vertices[n - 1]],
                            polygon_attr: self.polygon_attr,
                            teximage_param: self.teximage_param,
                            palette_base: self.palette_base,
                        }
                    } else {
                        Polygon3D {
                            vertices: [self.prim_vertices[n - 2], self.prim_vertices[n - 3], self.prim_vertices[n - 1]],
                            polygon_attr: self.polygon_attr,
                            teximage_param: self.teximage_param,
                            palette_base: self.palette_base,
                        }
                    };
                    self.polygons_back.push(poly);
                }
            }
            PrimitiveType::QuadStrip => {
                if self.prim_vertices.len() >= 4 && (self.prim_vertices.len() % 2) == 0 {
                    let n = self.prim_vertices.len();
                    let poly0 = Polygon3D {
                        vertices: [self.prim_vertices[n - 4], self.prim_vertices[n - 3], self.prim_vertices[n - 2]],
                        polygon_attr: self.polygon_attr,
                        teximage_param: self.teximage_param,
                        palette_base: self.palette_base,
                    };
                    let poly1 = Polygon3D {
                        vertices: [self.prim_vertices[n - 3], self.prim_vertices[n - 1], self.prim_vertices[n - 2]],
                        polygon_attr: self.polygon_attr,
                        teximage_param: self.teximage_param,
                        palette_base: self.palette_base,
                    };
                    self.polygons_back.push(poly0);
                    self.polygons_back.push(poly1);
                }
            }
        }
    }

    /// Swap buffers at VBlank boundary
    pub fn swap_buffers_if_requested(&mut self) {
        if self.swap_buffers_requested {
            std::mem::swap(&mut self.polygons_front, &mut self.polygons_back);
            self.polygons_back.clear();
            self.swap_buffers_requested = false;
        }
    }
}
