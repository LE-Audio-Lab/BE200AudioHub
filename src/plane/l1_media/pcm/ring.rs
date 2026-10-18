//! Lock-free bounded stereo Float32 ring.
//!
//! This is the Rust ownership equivalent of LE-Audio-Relay's SpscPcmRing.
//! It is intended for one producer (Windows capture) and one consumer
//! (Media/StreamClock). Capacity is measured in stereo PCM frames.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Debug)]
struct StereoSlot {
    left: AtomicU32,
    right: AtomicU32,
}

impl StereoSlot {
    fn zero() -> Self {
        Self {
            left: AtomicU32::new(0.0f32.to_bits()),
            right: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn store(&self, frame: [f32; 2]) {
        self.left.store(frame[0].to_bits(), Ordering::Relaxed);
        self.right.store(frame[1].to_bits(), Ordering::Relaxed);
    }

    fn load(&self) -> [f32; 2] {
        [
            f32::from_bits(self.left.load(Ordering::Relaxed)),
            f32::from_bits(self.right.load(Ordering::Relaxed)),
        ]
    }

    fn clear(&self) {
        self.left.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.right.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

/// Bounded SPSC ring measured in stereo PCM frames.
///
/// The type is memory-safe even if misused, but the ordering contract assumes
/// exactly one writer and one reader. Producer publication is Release/Acquire
/// fenced by write_frames; consumer retirement is fenced by read_frames.
#[derive(Debug)]
pub struct SpscStereoPcmRing {
    slots: Box<[StereoSlot]>,
    capacity_frames: usize,
    write_frames: AtomicU64,
    read_frames: AtomicU64,
}

impl SpscStereoPcmRing {
    pub fn new(capacity_frames: usize) -> Result<Self, &'static str> {
        if capacity_frames == 0 {
            return Err("PCM ring capacity must be nonzero");
        }

        let slots = (0..capacity_frames)
            .map(|_| StereoSlot::zero())
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Ok(Self {
            slots,
            capacity_frames,
            write_frames: AtomicU64::new(0),
            read_frames: AtomicU64::new(0),
        })
    }

    pub const fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    pub fn fill_frames(&self) -> usize {
        let write = self.write_frames.load(Ordering::Acquire);
        let read = self.read_frames.load(Ordering::Acquire);
        usize::try_from(write.saturating_sub(read))
            .unwrap_or(usize::MAX)
            .min(self.capacity_frames)
    }

    pub fn write(
        &self,
        source: &[[f32; 2]],
        max_fill_frames: usize,
    ) -> usize {
        if source.is_empty() {
            return 0;
        }

        let max_fill = max_fill_frames.clamp(1, self.capacity_frames);
        let write = self.write_frames.load(Ordering::Relaxed);
        let read = self.read_frames.load(Ordering::Acquire);
        let used = usize::try_from(write.saturating_sub(read))
            .unwrap_or(usize::MAX);

        let free = max_fill.saturating_sub(used.min(max_fill));
        let frames_to_write = source.len().min(free);
        if frames_to_write == 0 {
            return 0;
        }

        for (offset, frame) in source[..frames_to_write].iter().enumerate() {
            let slot = usize::try_from(
                (write + offset as u64) % self.capacity_frames as u64,
            )
            .expect("ring slot fits usize");
            self.slots[slot].store(*frame);
        }

        self.write_frames.store(
            write + frames_to_write as u64,
            Ordering::Release,
        );
        frames_to_write
    }

    pub fn write_silence(
        &self,
        input_frames: usize,
        max_fill_frames: usize,
    ) -> usize {
        if input_frames == 0 {
            return 0;
        }

        let max_fill = max_fill_frames.clamp(1, self.capacity_frames);
        let write = self.write_frames.load(Ordering::Relaxed);
        let read = self.read_frames.load(Ordering::Acquire);
        let used = usize::try_from(write.saturating_sub(read))
            .unwrap_or(usize::MAX);

        let free = max_fill.saturating_sub(used.min(max_fill));
        let frames_to_write = input_frames.min(free);
        if frames_to_write == 0 {
            return 0;
        }

        for offset in 0..frames_to_write {
            let slot = usize::try_from(
                (write + offset as u64) % self.capacity_frames as u64,
            )
            .expect("ring slot fits usize");
            self.slots[slot].clear();
        }

        self.write_frames.store(
            write + frames_to_write as u64,
            Ordering::Release,
        );
        frames_to_write
    }

    pub fn read(&self, destination: &mut [[f32; 2]]) -> usize {
        if destination.is_empty() {
            return 0;
        }

        let read = self.read_frames.load(Ordering::Relaxed);
        let write = self.write_frames.load(Ordering::Acquire);
        let available = usize::try_from(write.saturating_sub(read))
            .unwrap_or(usize::MAX);
        let frames_to_read = destination.len().min(available);
        if frames_to_read == 0 {
            return 0;
        }

        for (offset, output) in
            destination[..frames_to_read].iter_mut().enumerate()
        {
            let slot = usize::try_from(
                (read + offset as u64) % self.capacity_frames as u64,
            )
            .expect("ring slot fits usize");
            *output = self.slots[slot].load();
        }

        self.read_frames.store(
            read + frames_to_read as u64,
            Ordering::Release,
        );
        frames_to_read
    }

    pub fn discard(&self, requested_frames: usize) -> usize {
        if requested_frames == 0 {
            return 0;
        }

        let read = self.read_frames.load(Ordering::Relaxed);
        let write = self.write_frames.load(Ordering::Acquire);
        let available = usize::try_from(write.saturating_sub(read))
            .unwrap_or(usize::MAX);
        let frames = requested_frames.min(available);

        if frames != 0 {
            self.read_frames.store(
                read + frames as u64,
                Ordering::Release,
            );
        }

        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: f32) -> [f32; 2] {
        [value, -value]
    }

    #[test]
    fn rejects_zero_capacity() {
        assert!(SpscStereoPcmRing::new(0).is_err());
    }

    #[test]
    fn write_read_preserves_stereo_frames_and_fill() {
        let ring = SpscStereoPcmRing::new(4).unwrap();
        assert_eq!(ring.write(&[frame(1.0), frame(2.0)], 4), 2);
        assert_eq!(ring.fill_frames(), 2);

        let mut out = [[0.0; 2]; 2];
        assert_eq!(ring.read(&mut out), 2);
        assert_eq!(out, [frame(1.0), frame(2.0)]);
        assert_eq!(ring.fill_frames(), 0);
    }

    #[test]
    fn wraparound_preserves_fifo_order() {
        let ring = SpscStereoPcmRing::new(3).unwrap();
        assert_eq!(
            ring.write(&[frame(1.0), frame(2.0), frame(3.0)], 3),
            3
        );

        let mut first = [[0.0; 2]; 2];
        assert_eq!(ring.read(&mut first), 2);
        assert_eq!(first, [frame(1.0), frame(2.0)]);

        assert_eq!(ring.write(&[frame(4.0), frame(5.0)], 3), 2);

        let mut rest = [[0.0; 2]; 3];
        assert_eq!(ring.read(&mut rest), 3);
        assert_eq!(rest, [frame(3.0), frame(4.0), frame(5.0)]);
    }

    #[test]
    fn max_fill_bounds_startup_accumulation() {
        let ring = SpscStereoPcmRing::new(8).unwrap();
        assert_eq!(
            ring.write(
                &[frame(1.0), frame(2.0), frame(3.0), frame(4.0)],
                2,
            ),
            2
        );
        assert_eq!(ring.fill_frames(), 2);
        assert_eq!(ring.write(&[frame(5.0)], 2), 0);
    }

    #[test]
    fn silence_and_discard_match_relay_semantics() {
        let ring = SpscStereoPcmRing::new(6).unwrap();
        assert_eq!(ring.write(&[frame(1.0), frame(2.0)], 6), 2);
        assert_eq!(ring.write_silence(2, 6), 2);
        assert_eq!(ring.discard(1), 1);

        let mut out = [[9.0; 2]; 3];
        assert_eq!(ring.read(&mut out), 3);
        assert_eq!(out[0], frame(2.0));
        assert_eq!(out[1], [0.0, 0.0]);
        assert_eq!(out[2], [0.0, 0.0]);
    }
}
