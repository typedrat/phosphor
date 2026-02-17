pub mod audio;
pub mod external;
pub mod oscilloscope;
pub mod resample;
pub mod vector;

/// Current beam physics parameters, shared with input sources that need
/// them for sample generation (e.g. vector subdivision density).
#[derive(Clone, Debug)]
pub struct BeamState {
    /// Minimum spot radius in normalized screen coords.
    /// Used to determine subdivision density — consecutive samples should
    /// be within this distance to guarantee Gaussian overlap.
    pub spot_radius: f32,
}

/// Common interface for all beam input sources.
///
/// `count` is a request — sources return up to that many samples.
/// Sources that produce a fixed batch (e.g. vector display lists) may
/// ignore `count` and return their natural output size.
pub trait BeamSource {
    fn generate(&mut self, count: usize, beam: &BeamState) -> Vec<BeamSample>;
}

/// A single beam position sample.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BeamSample {
    pub x: f32,
    pub y: f32,
    pub intensity: f32,
    pub dt: f32,
}

/// Producer half of the sample channel. Lives on the input thread.
pub struct SampleProducer {
    inner: rtrb::Producer<BeamSample>,
}

/// Consumer half of the sample channel. Lives on the render thread.
pub struct SampleConsumer {
    inner: rtrb::Consumer<BeamSample>,
}

/// Create a bounded SPSC sample channel.
pub fn sample_channel(capacity: usize) -> (SampleProducer, SampleConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    (
        SampleProducer { inner: producer },
        SampleConsumer { inner: consumer },
    )
}

impl SampleProducer {
    /// Push a single sample. Returns `true` if successful, `false` if full.
    pub fn push(&mut self, sample: BeamSample) -> bool {
        self.inner.push(sample).is_ok()
    }

    /// Bulk-push samples using zero-copy write_chunk. Returns the number
    /// of samples actually written (may be less than `samples.len()` if
    /// the buffer doesn't have enough free slots).
    pub fn push_bulk(&mut self, samples: &[BeamSample]) -> usize {
        let available = self.inner.slots();
        let n = samples.len().min(available);
        if n == 0 {
            return 0;
        }
        if let Ok(mut chunk) = self.inner.write_chunk(n) {
            let (first, second) = chunk.as_mut_slices();
            let first_len = first.len();
            first.copy_from_slice(&samples[..first_len]);
            if !second.is_empty() {
                second.copy_from_slice(&samples[first_len..n]);
            }
            chunk.commit_all();
            n
        } else {
            0
        }
    }
}

impl SampleConsumer {
    /// Number of samples currently waiting in the buffer.
    pub fn pending(&self) -> usize {
        self.inner.slots()
    }

    /// Drain all pending samples using zero-copy read_chunk.
    pub fn drain(&mut self) -> Vec<BeamSample> {
        self.drain_up_to(usize::MAX)
    }

    /// Drain up to `max` pending samples using zero-copy read_chunk.
    /// Any samples beyond `max` remain in the buffer for the next call.
    pub fn drain_up_to(&mut self, max: usize) -> Vec<BeamSample> {
        let available = self.inner.slots();
        let count = available.min(max);
        if count == 0 {
            return Vec::new();
        }
        let chunk = self.inner.read_chunk(count).unwrap();
        let (first, second) = chunk.as_slices();
        let mut samples = Vec::with_capacity(count);
        samples.extend_from_slice(first);
        samples.extend_from_slice(second);
        chunk.commit_all();
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_sample_is_pod() {
        let sample = BeamSample {
            x: 0.5,
            y: 0.5,
            intensity: 1.0,
            dt: 0.001,
        };
        let bytes = bytemuck::bytes_of(&sample);
        assert_eq!(bytes.len(), 16); // 4 x f32
    }

    #[test]
    fn channel_push_and_drain() {
        let (mut tx, mut rx) = sample_channel(64);
        tx.push(BeamSample {
            x: 0.1,
            y: 0.2,
            intensity: 1.0,
            dt: 0.001,
        });
        tx.push(BeamSample {
            x: 0.3,
            y: 0.4,
            intensity: 0.5,
            dt: 0.001,
        });

        let drained = rx.drain();
        assert_eq!(drained.len(), 2);
        assert!((drained[0].x - 0.1).abs() < f32::EPSILON);
        assert!((drained[1].x - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn drain_clears_buffer() {
        let (mut tx, mut rx) = sample_channel(64);
        tx.push(BeamSample {
            x: 0.0,
            y: 0.0,
            intensity: 1.0,
            dt: 0.001,
        });

        let first = rx.drain();
        assert_eq!(first.len(), 1);

        let second = rx.drain();
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn full_buffer_drops_samples() {
        let (mut tx, mut rx) = sample_channel(2);
        let s = BeamSample {
            x: 0.0,
            y: 0.0,
            intensity: 1.0,
            dt: 0.001,
        };
        assert!(tx.push(s));
        assert!(tx.push(s));
        assert!(!tx.push(s)); // buffer full

        assert_eq!(rx.drain().len(), 2);
    }

    #[test]
    fn producer_and_consumer_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SampleProducer>();
        assert_send::<SampleConsumer>();
    }

    #[test]
    fn bulk_push_and_drain() {
        let (mut tx, mut rx) = sample_channel(128);
        let samples: Vec<BeamSample> = (0..50)
            .map(|i| BeamSample {
                x: i as f32 * 0.01,
                y: 0.5,
                intensity: 1.0,
                dt: 0.001,
            })
            .collect();

        let pushed = tx.push_bulk(&samples);
        assert_eq!(pushed, 50);

        let drained = rx.drain();
        assert_eq!(drained.len(), 50);
        assert!((drained[0].x - 0.0).abs() < f32::EPSILON);
        assert!((drained[49].x - 0.49).abs() < f32::EPSILON);
    }

    #[test]
    fn bulk_push_partial_when_full() {
        let (mut tx, mut rx) = sample_channel(4);
        let samples: Vec<BeamSample> = (0..10)
            .map(|_| BeamSample {
                x: 0.5,
                y: 0.5,
                intensity: 1.0,
                dt: 0.001,
            })
            .collect();

        let pushed = tx.push_bulk(&samples);
        assert_eq!(pushed, 4); // only 4 slots available

        assert_eq!(rx.drain().len(), 4);
    }

    #[test]
    fn drain_up_to_respects_cap() {
        let (mut tx, mut rx) = sample_channel(128);
        let samples: Vec<BeamSample> = (0..100)
            .map(|i| BeamSample {
                x: i as f32 * 0.01,
                y: 0.5,
                intensity: 1.0,
                dt: 0.001,
            })
            .collect();

        tx.push_bulk(&samples);

        // Drain only 30 — 70 should remain
        let first = rx.drain_up_to(30);
        assert_eq!(first.len(), 30);

        // Drain the rest
        let second = rx.drain_up_to(1000);
        assert_eq!(second.len(), 70);

        // Nothing left
        let third = rx.drain_up_to(10);
        assert_eq!(third.len(), 0);
    }
}
