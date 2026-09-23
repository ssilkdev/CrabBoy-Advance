//! Binary save-state encoding helpers (ROADMAP M2).
//!
//! Each component implements [`Snapshot`] next to its own struct (so it can
//! reach private fields); `Gba::save_state` stitches them together in a
//! fixed order after the original v1/v2 layout. All values are
//! little-endian, fixed width, with no padding, so a state is a pure
//! function of emulator state: save -> load -> save is byte-identical.
//!
//! Decoding never panics on a short or corrupt buffer: every read returns
//! `None` past the end, and the caller rejects the whole state.

pub struct StateWriter {
    pub buf: Vec<u8>,
}

impl StateWriter {
    pub fn new(buf: Vec<u8>) -> Self {
        Self { buf }
    }
    #[inline]
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    #[inline]
    pub fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }
    #[inline]
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    #[inline]
    pub fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }
    #[inline]
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    /// Length-prefixed byte slice.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
    }
    pub fn opt_u64(&mut self, v: Option<u64>) {
        self.bool(v.is_some());
        self.u64(v.unwrap_or(0));
    }
}

pub struct StateReader<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> StateReader<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }
    #[inline]
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }
    pub fn i64(&mut self) -> Option<i64> {
        Some(self.u64()? as i64)
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    /// Length-prefixed bytes into a fixed buffer; the length must match.
    pub fn bytes_into(&mut self, dst: &mut [u8]) -> Option<()> {
        let src = self.bytes()?;
        (src.len() == dst.len()).then(|| dst.copy_from_slice(src))
    }
    pub fn opt_u64(&mut self) -> Option<Option<u64>> {
        let some = self.bool()?;
        let v = self.u64()?;
        Some(some.then_some(v))
    }
}

/// A component that can be written to and restored from a save state.
pub trait Snapshot {
    fn save(&self, w: &mut StateWriter);
    /// Restore; `None` means the data was malformed (the caller rejects the
    /// state).
    fn load(&mut self, r: &mut StateReader) -> Option<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_short_reads() {
        let mut w = StateWriter::new(Vec::new());
        w.u8(1);
        w.u16(0x2345);
        w.u32(0x6789_ABCD);
        w.i64(-5);
        w.f32(1.5);
        w.bytes(b"hi");
        w.opt_u64(Some(9));
        let mut r = StateReader::new(&w.buf, 0);
        assert_eq!(r.u8(), Some(1));
        assert_eq!(r.u16(), Some(0x2345));
        assert_eq!(r.u32(), Some(0x6789_ABCD));
        assert_eq!(r.i64(), Some(-5));
        assert_eq!(r.f32(), Some(1.5));
        assert_eq!(r.bytes(), Some(&b"hi"[..]));
        assert_eq!(r.opt_u64(), Some(Some(9)));
        assert_eq!(r.u8(), None);
        // A length prefix pointing past the end is rejected, not a panic.
        let mut r = StateReader::new(&[0xFF, 0xFF, 0xFF, 0x7F], 0);
        assert_eq!(r.bytes(), None);
    }
}
