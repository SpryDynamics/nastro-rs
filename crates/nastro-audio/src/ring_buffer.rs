//! Lock-protected ring buffer for interleaved `f32` audio samples.
//!
//! Ported in spirit from the original Swift `AudioRingBuffer`: a capture thread
//! pushes samples as they arrive, and the mixer drains them on its own wall-clock
//! schedule. If the mixer falls behind, the oldest samples are dropped once the
//! buffer is full (bounded memory, graceful degradation under sustained overload
//! rather than unbounded growth).

use std::collections::VecDeque;
use std::sync::Mutex;

pub struct AudioRingBuffer {
    inner: Mutex<VecDeque<f32>>,
    capacity: usize,
}

impl AudioRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Push samples, dropping the oldest ones if the buffer would exceed capacity.
    pub fn push(&self, samples: &[f32]) {
        let mut buf = self.inner.lock().unwrap();
        for &sample in samples {
            if buf.len() == self.capacity {
                buf.pop_front();
            }
            buf.push_back(sample);
        }
    }

    /// Drain up to `max` samples (fewer if not enough are buffered - never blocks,
    /// never panics). Callers that need exactly `max` samples (e.g. the mixer) are
    /// responsible for zero-filling the shortfall.
    pub fn drain(&self, max: usize) -> Vec<f32> {
        let mut buf = self.inner.lock().unwrap();
        let take = max.min(buf.len());
        buf.drain(..take).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_round_trips_within_capacity() {
        let buf = AudioRingBuffer::new(10);
        buf.push(&[1.0, 2.0, 3.0]);

        assert_eq!(buf.drain(10), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn drain_removes_only_what_it_returns() {
        let buf = AudioRingBuffer::new(10);
        buf.push(&[1.0, 2.0, 3.0, 4.0]);

        assert_eq!(buf.drain(2), vec![1.0, 2.0]);
        assert_eq!(buf.drain(10), vec![3.0, 4.0]);
    }

    #[test]
    fn drain_more_than_available_returns_only_whats_there() {
        let buf = AudioRingBuffer::new(10);
        buf.push(&[1.0, 2.0]);

        assert_eq!(buf.drain(100), vec![1.0, 2.0]);
    }

    #[test]
    fn drain_from_empty_buffer_returns_empty() {
        let buf = AudioRingBuffer::new(10);
        assert_eq!(buf.drain(5), Vec::<f32>::new());
    }

    #[test]
    fn exceeding_capacity_drops_oldest_samples() {
        let buf = AudioRingBuffer::new(3);
        buf.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);

        // Only the most recent 3 survive.
        assert_eq!(buf.drain(10), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn len_and_is_empty_reflect_state() {
        let buf = AudioRingBuffer::new(10);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        buf.push(&[1.0, 2.0]);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 2);

        buf.drain(2);
        assert!(buf.is_empty());
    }

    #[test]
    fn multiple_pushes_accumulate_in_order() {
        let buf = AudioRingBuffer::new(10);
        buf.push(&[1.0]);
        buf.push(&[2.0, 3.0]);
        buf.push(&[4.0]);

        assert_eq!(buf.drain(10), vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn concurrent_pushes_from_multiple_threads_preserve_total_count() {
        use std::sync::Arc;
        use std::thread;

        let buf = Arc::new(AudioRingBuffer::new(10_000));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let buf = Arc::clone(&buf);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    buf.push(&[1.0]);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(buf.len(), 800);
    }
}
