use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use atomic_float::AtomicF32;

/// Decoded audio ready to be shared.
pub struct DecodedAudio {
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Shared playback state coordinating cpal callback, sim thread, and UI.
pub struct SharedAudioPlayback {
    pub samples: Arc<[f32]>,
    pub channels: u16,
    pub sample_rate: u32,
    pub position: AtomicUsize,
    pub seek_target: AtomicUsize,
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub speed: AtomicF32,
    pub volume: AtomicF32,
}

impl SharedAudioPlayback {
    pub fn new(decoded: DecodedAudio) -> Self {
        Self {
            samples: decoded.samples,
            channels: decoded.channels,
            sample_rate: decoded.sample_rate,
            position: AtomicUsize::new(0),
            seek_target: AtomicUsize::new(usize::MAX),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            speed: AtomicF32::new(1.0),
            volume: AtomicF32::new(1.0),
        }
    }

    pub fn total_frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_secs(&self) -> f32 {
        self.total_frames() as f32 / self.sample_rate as f32
    }

    pub fn position_secs(&self) -> f32 {
        self.position.load(Ordering::Relaxed) as f32 / self.sample_rate as f32
    }

    pub fn seek_to_frame(&self, frame: usize) {
        self.seek_target
            .store(frame.min(self.total_frames()), Ordering::Relaxed);
    }

    pub fn seek_to_secs(&self, secs: f32) {
        let frame = (secs * self.sample_rate as f32) as usize;
        self.seek_to_frame(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shared(num_frames: usize, sample_rate: u32) -> SharedAudioPlayback {
        let channels: u16 = 2;
        let samples: Vec<f32> = vec![0.0; num_frames * channels as usize];
        SharedAudioPlayback::new(DecodedAudio {
            samples: samples.into(),
            sample_rate,
            channels,
        })
    }

    #[test]
    fn total_frames_stereo() {
        let shared = make_shared(1000, 44100);
        assert_eq!(shared.total_frames(), 1000);
    }

    #[test]
    fn duration_secs() {
        let shared = make_shared(44100, 44100);
        assert!((shared.duration_secs() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn position_secs_tracks_position() {
        let shared = make_shared(44100, 44100);
        shared.position.store(22050, Ordering::Relaxed);
        assert!((shared.position_secs() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn seek_to_frame_clamps() {
        let shared = make_shared(1000, 44100);
        shared.seek_to_frame(5000);
        assert_eq!(shared.seek_target.load(Ordering::Relaxed), 1000);
    }

    #[test]
    fn seek_to_secs() {
        let shared = make_shared(44100, 44100);
        shared.seek_to_secs(0.5);
        assert_eq!(shared.seek_target.load(Ordering::Relaxed), 22050);
    }
}
