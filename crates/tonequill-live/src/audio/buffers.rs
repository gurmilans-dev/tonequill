//! Fixed-size SPSC messages: no allocation, deallocation, logging or locking in
//! ordinary callbacks. Only the worker may wait for capacity or run the modem.
use cpal::{FromSample, Sample};
use rtrb::{Consumer, Producer};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub const BLOCK: usize = 1024;
pub const CAPACITY: usize = 256; // 5.46 seconds of mono PCM, about 1 MiB per ring.
pub const PREFILL: usize = 48; // 1.024 seconds before starting playback.

#[derive(Clone, Copy)]
pub struct Block {
    pub samples: [f32; BLOCK],
    pub len: usize,
    pub tag: u64, // Capture epoch, or nonzero output completion marker.
}
impl Default for Block {
    fn default() -> Self {
        Self {
            samples: [0.0; BLOCK],
            len: 0,
            tag: 0,
        }
    }
}

#[derive(Default)]
pub struct Shared {
    pub epoch: AtomicU64, // Odd: listen. Even: local TX/settling; discard capture.
    /// Blocks dropped because the worker did not drain the bounded queue.
    pub overruns: AtomicU64,
    /// Backend discontinuity flags. WASAPI still delivers the flagged buffer,
    /// so these are reported without throwing already captured PCM away.
    pub discontinuities: AtomicU64,
    pub active: AtomicBool,
    pub underflow: AtomicBool,
    pub stream_error: AtomicBool,
    pub backend_error: OnceLock<(&'static str, cpal::Error)>,
    pub played: AtomicU64,
    pub finished_at_us: AtomicU64,
    pub input_at_ms: AtomicU64,
}

pub struct Capture {
    pub queue: Producer<Block>,
    pub shared: Arc<Shared>,
    pub channel: usize,
}
impl Capture {
    pub fn process<T>(&mut self, data: &[T], channels: usize)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        if epoch.is_multiple_of(2) {
            return;
        }
        for part in data.chunks(BLOCK * channels) {
            let mut block = Block {
                tag: epoch,
                ..Default::default()
            };
            for frame in part.chunks_exact(channels) {
                let value = f32::from_sample(frame[self.channel]);
                block.samples[block.len] = if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                block.len += 1;
            }
            if self.queue.push(block).is_err() {
                self.shared.overruns.fetch_add(1, Ordering::Release);
            }
        }
    }
}

pub struct Playback {
    pub queue: Consumer<Block>,
    pub shared: Arc<Shared>,
    current: Block,
    index: usize,
}
impl Playback {
    pub fn new(queue: Consumer<Block>, shared: Arc<Shared>) -> Self {
        Self {
            queue,
            shared,
            current: Block::default(),
            index: 0,
        }
    }
    /// Returns (ticket, mono sample offset) when an explicit end marker is heard.
    /// An empty active queue is a detectable discontinuity, never silent success.
    pub fn process<T: Sample + FromSample<f32>>(
        &mut self,
        output: &mut [T],
        channels: usize,
    ) -> Option<(u64, usize)> {
        let mut completed = None;
        for (offset, frame) in output.chunks_mut(channels).enumerate() {
            let mut value = 0.0;
            if self.shared.active.load(Ordering::Acquire) {
                if self.index == self.current.len {
                    match self.queue.pop() {
                        Ok(block) if block.tag != 0 => {
                            completed = Some((block.tag, offset));
                            self.shared.active.store(false, Ordering::Release);
                        }
                        Ok(block) => {
                            self.current = block;
                            self.index = 0;
                        }
                        Err(_) => {
                            self.shared.underflow.store(true, Ordering::Release);
                            self.shared.active.store(false, Ordering::Release);
                        }
                    }
                }
                if self.shared.active.load(Ordering::Acquire) && self.index < self.current.len {
                    value = self.current.samples[self.index];
                    self.index += 1;
                }
            }
            frame.fill(T::from_sample(value));
        }
        completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;
    #[test]
    fn channel_selection_pcm_conversion_epoch_gating_and_overrun_are_explicit() {
        let shared = Arc::new(Shared::default());
        shared.epoch.store(1, Ordering::Release);
        let (queue, mut rx) = RingBuffer::new(1);
        let mut capture = Capture {
            queue,
            shared: shared.clone(),
            channel: 1,
        };
        capture.process(&[0_i16, i16::MIN, 0, 16384, 0, i16::MAX], 2);
        capture.process(&[0_i16, 42], 2);
        assert_eq!(shared.overruns.load(Ordering::Acquire), 1);
        let block = rx.pop().unwrap();
        assert_eq!(block.len, 3);
        assert_eq!(block.tag, 1);
        assert_eq!(&block.samples[..2], &[-1.0, 0.5]);
        assert!(block.samples[2] > 0.999);
        shared.epoch.store(2, Ordering::Release);
        capture.process(&[0_i16, 42], 2);
        assert!(rx.pop().is_err());
        shared.epoch.store(3, Ordering::Release);
        capture.process(&[0_u16, 32768], 2);
        let block = rx.pop().unwrap();
        assert_eq!(block.tag, 3);
        assert_eq!(block.samples[0], 0.0);
        capture.process(&[0_f32, f32::NAN, 0.0, 2.0], 2);
        assert_eq!(&rx.pop().unwrap().samples[..2], &[0.0, 1.0]);
    }

    #[test]
    fn playback_spans_callbacks_fills_all_channels_and_marks_end_without_underflow() {
        let shared = Arc::new(Shared::default());
        let (mut tx, rx) = RingBuffer::new(3);
        let mut block = Block {
            len: 3,
            ..Default::default()
        };
        block.samples[..3].copy_from_slice(&[-1.0, 0.0, 0.5]);
        tx.push(block).ok().unwrap();
        tx.push(Block {
            tag: 1,
            ..Default::default()
        })
        .ok()
        .unwrap();
        let mut playback = Playback::new(rx, shared.clone());
        shared.active.store(true, Ordering::Release);
        let mut a = [0_i16; 4];
        assert_eq!(playback.process(&mut a, 2), None);
        assert_eq!(a, [i16::MIN, i16::MIN, 0, 0]);
        let mut b = [0_u16; 6];
        assert_eq!(playback.process(&mut b, 2), Some((1, 1)));
        assert_eq!(b, [49152, 49152, 32768, 32768, 32768, 32768]);
        assert!(!shared.underflow.load(Ordering::Acquire));
        shared.active.store(true, Ordering::Release);
        playback.process(&mut b, 2);
        assert!(shared.underflow.load(Ordering::Acquire));
    }
}
