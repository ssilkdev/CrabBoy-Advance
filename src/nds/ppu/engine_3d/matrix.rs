//! Fixed-point 4x4 Matrix Mathematics for the Nintendo DS Geometry Engine
//!
//! The Nintendo DS represents 3D coordinates and matrices in fixed-point numbers
//! with 12 fractional bits (1.0 = 4096 = 1 << 12). All geometry transformations
//! (rotation, translation, scaling, projection) operate in this format.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Matrix4x4 {
    /// 4x4 elements in column-major order:
    /// [m0,  m1,  m2,  m3,   <- Column 0
    ///  m4,  m5,  m6,  m7,   <- Column 1
    ///  m8,  m9,  m10, m11,  <- Column 2
    ///  m12, m13, m14, m15]  <- Column 3
    pub m: [i32; 16],
}

impl Default for Matrix4x4 {
    fn default() -> Self {
        Self::identity()
    }
}

impl Matrix4x4 {
    pub const FIXED_ONE: i32 = 4096; // 1.0 in 12-bit fixed point (1 << 12)

    /// Returns a 4x4 identity matrix
    pub const fn identity() -> Self {
        let mut m = [0i32; 16];
        m[0] = Self::FIXED_ONE;
        m[5] = Self::FIXED_ONE;
        m[10] = Self::FIXED_ONE;
        m[15] = Self::FIXED_ONE;
        Self { m }
    }

    /// Load from 16 words (full 4x4 matrix)
    pub fn from_slice_4x4(slice: &[i32; 16]) -> Self {
        Self { m: *slice }
    }

    /// Load from 12 words (4x3 affine matrix)
    /// Row 3 is set to [0, 0, 0, 1.0]
    pub fn from_slice_4x3(slice: &[i32; 12]) -> Self {
        let mut m = [0i32; 16];
        // Col 0
        m[0] = slice[0];
        m[1] = slice[1];
        m[2] = slice[2];
        m[3] = 0;
        // Col 1
        m[4] = slice[3];
        m[5] = slice[4];
        m[6] = slice[5];
        m[7] = 0;
        // Col 2
        m[8] = slice[6];
        m[9] = slice[7];
        m[10] = slice[8];
        m[11] = 0;
        // Col 3 (Translation)
        m[12] = slice[9];
        m[13] = slice[10];
        m[14] = slice[11];
        m[15] = Self::FIXED_ONE;
        Self { m }
    }

    /// Fixed-point matrix multiplication: C = M * C (self = rhs * self)
    /// Performs standard 4x4 matrix multiplication with 12-bit right shift.
    pub fn mul_4x4(&self, lhs: &Matrix4x4) -> Matrix4x4 {
        let mut res = [0i32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut sum: i64 = 0;
                for k in 0..4 {
                    let a = lhs.m[k * 4 + row] as i64;
                    let b = self.m[col * 4 + k] as i64;
                    sum += a * b;
                }
                res[col * 4 + row] = (sum >> 12) as i32;
            }
        }
        Matrix4x4 { m: res }
    }

    /// Multiply by 4x3 matrix: C = M_4x3 * C
    pub fn mul_4x3(&self, lhs_slice: &[i32; 12]) -> Matrix4x4 {
        let lhs = Matrix4x4::from_slice_4x3(lhs_slice);
        self.mul_4x4(&lhs)
    }

    /// Multiply by 3x3 matrix: C = M_3x3 * C
    pub fn mul_3x3(&self, lhs_slice: &[i32; 9]) -> Matrix4x4 {
        let mut lhs = [0i32; 16];
        lhs[0] = lhs_slice[0];
        lhs[1] = lhs_slice[1];
        lhs[2] = lhs_slice[2];
        lhs[4] = lhs_slice[3];
        lhs[5] = lhs_slice[4];
        lhs[6] = lhs_slice[5];
        lhs[8] = lhs_slice[6];
        lhs[9] = lhs_slice[7];
        lhs[10] = lhs_slice[8];
        lhs[15] = Self::FIXED_ONE;

        self.mul_4x4(&Matrix4x4 { m: lhs })
    }

    /// Multiply by scale factors: C = Scale(sx, sy, sz) * C
    pub fn scale(&mut self, sx: i32, sy: i32, sz: i32) {
        let mut s = Matrix4x4::identity();
        s.m[0] = sx;
        s.m[5] = sy;
        s.m[10] = sz;
        *self = self.mul_4x4(&s);
    }

    /// Multiply by translation vector: C = Trans(tx, ty, tz) * C
    pub fn translate(&mut self, tx: i32, ty: i32, tz: i32) {
        let mut t = Matrix4x4::identity();
        t.m[12] = tx;
        t.m[13] = ty;
        t.m[14] = tz;
        *self = self.mul_4x4(&t);
    }

    /// Transform a 3D vertex (x, y, z, 1.0) into clip coordinates (X, Y, Z, W)
    /// Coordinates are in 12-bit fixed point.
    #[inline(always)]
    pub fn transform_vertex(&self, x: i32, y: i32, z: i32) -> (i64, i64, i64, i64) {
        let vx = x as i64;
        let vy = y as i64;
        let vz = z as i64;
        let vw = Self::FIXED_ONE as i64;

        let rx = (vx * self.m[0] as i64 + vy * self.m[4] as i64 + vz * self.m[8] as i64 + vw * self.m[12] as i64) >> 12;
        let ry = (vx * self.m[1] as i64 + vy * self.m[5] as i64 + vz * self.m[9] as i64 + vw * self.m[13] as i64) >> 12;
        let rz = (vx * self.m[2] as i64 + vy * self.m[6] as i64 + vz * self.m[10] as i64 + vw * self.m[14] as i64) >> 12;
        let rw = (vx * self.m[3] as i64 + vy * self.m[7] as i64 + vz * self.m[11] as i64 + vw * self.m[15] as i64) >> 12;

        (rx, ry, rz, rw)
    }

    /// Transform directional vector (x, y, z) by upper 3x3 of matrix
    #[inline(always)]
    pub fn transform_vector(&self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        let vx = x as i64;
        let vy = y as i64;
        let vz = z as i64;

        let rx = (vx * self.m[0] as i64 + vy * self.m[4] as i64 + vz * self.m[8] as i64) >> 12;
        let ry = (vx * self.m[1] as i64 + vy * self.m[5] as i64 + vz * self.m[9] as i64) >> 12;
        let rz = (vx * self.m[2] as i64 + vy * self.m[6] as i64 + vz * self.m[10] as i64) >> 12;

        (rx as i32, ry as i32, rz as i32)
    }
}
