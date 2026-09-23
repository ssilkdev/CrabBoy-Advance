//! Lock-free single-producer / single-consumer sample ring (ROADMAP M1,
//! AI_AGENT_FIX_DESIGN Phase 3 APU items).
//!
//! The emulation thread pushes samples and the real-time audio callback
//! pops them. With the old `Mutex<VecDeque>` the callback could block while
//! the emulator held the lock (over a large drain, for instance), causing
//! underruns, and skipped writing entirely if the lock failed. This ring
//! never blocks either side and is written in safe Rust: samples are stored
//! as `f32` bit patterns in `AtomicU32` slots, and the read/write positions
//! are monotonically increasing counters (`Acquire`/`Release` pairs order
//! the slot writes before the position update).

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub struct SampleRing {
    slots: Box<[AtomicU32]>,
    /// Total samples ever written (producer-owned).
    write: AtomicUsize,
    /// Total samples ever read (consumer-owned).
    read: AtomicUsize,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        let slots = (0..capacity.max(2)).map(|_| AtomicU32::new(0)).collect();
        Self { slots, write: AtomicUsize::new(0), read: AtomicUsize::new(0) }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Samples currently queued.
    pub fn len(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Producer: append as many samples as fit; returns how many were
    /// written.
    pub fn push_slice(&self, samples: &[f32]) -> usize {
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let free = self.capacity() - w.wrapping_sub(r);
        let n = samples.len().min(free);
        for (i, &s) in samples[..n].iter().enumerate() {
            self.slots[(w + i) % self.capacity()].store(s.to_bits(), Ordering::Relaxed);
        }
        self.write.store(w.wrapping_add(n), Ordering::Release);
        n
    }

    /// Consumer: pop one sample.
    #[inline]
    pub fn pop(&self) -> Option<f32> {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let v = f32::from_bits(self.slots[r % self.capacity()].load(Ordering::Relaxed));
        self.read.store(r.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn fifo_order_and_capacity() {
        let r = SampleRing::new(4);
        assert_eq!(r.push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0]), 4);
        assert_eq!(r.len(), 4);
        assert_eq!(r.pop(), Some(1.0));
        assert_eq!(r.push_slice(&[6.0]), 1);
        let out: Vec<f32> = std::iter::from_fn(|| r.pop()).collect();
        assert_eq!(out, vec![2.0, 3.0, 4.0, 6.0]);
        assert!(r.is_empty());
    }

    #[test]
    fn threaded_producer_consumer_keeps_order() {
        let ring = Arc::new(SampleRing::new(64));
        let prod = Arc::clone(&ring);
        let n = 100_000usize;
        let t = std::thread::spawn(move || {
            let mut i = 0;
            while i < n {
                i += prod.push_slice(&[i as f32]);
            }
        });
        let mut expect = 0usize;
        while expect < n {
            if let Some(v) = ring.pop() {
                assert_eq!(v, expect as f32);
                expect += 1;
            }
        }
        t.join().unwrap();
    }
}
