pub mod audio;
pub mod oscilloscope;

/// A single beam position sample.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
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
    /// Push a sample into the channel. Returns `true` if successful,
    /// `false` if the buffer is full (sample is dropped).
    pub fn push(&mut self, sample: BeamSample) -> bool {
        self.inner.push(sample).is_ok()
    }
}

impl SampleConsumer {
    /// Drain all pending samples from the channel.
    pub fn drain(&mut self) -> Vec<BeamSample> {
        let count = self.inner.slots();
        if count == 0 {
            return Vec::new();
        }
        let chunk = self.inner.read_chunk(count).unwrap();
        let samples: Vec<BeamSample> = chunk.into_iter().collect();
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
}
