use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::{BeamSample, BeamSource, BeamState};

pub struct AudioSource {
    samples: Vec<(f32, f32)>,
    sample_rate: u32,
    position: usize,
}

impl AudioSource {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?;

        let mut format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| anyhow::anyhow!("no audio track found"))?;

        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| anyhow::anyhow!("unknown sample rate"))?;
        let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(2);
        let track_id = track.id;

        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;

        let mut interleaved = Vec::new();

        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)?;
            let spec = *decoded.spec();
            let num_frames = decoded.capacity();

            let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
            sample_buf.copy_interleaved_ref(decoded);

            interleaved.extend_from_slice(sample_buf.samples());
        }

        // De-interleave into (left, right) pairs
        let samples: Vec<(f32, f32)> = match channels {
            1 => interleaved.iter().map(|&s| (s, s)).collect(),
            2 => interleaved.chunks_exact(2).map(|c| (c[0], c[1])).collect(),
            n => {
                // Take first two channels, skip the rest
                interleaved.chunks_exact(n).map(|c| (c[0], c[1])).collect()
            }
        };

        Ok(Self {
            samples,
            sample_rate,
            position: 0,
        })
    }

    pub fn seek(&mut self, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        self.position = (fraction * self.samples.len() as f32) as usize;
        self.position = self.position.min(self.samples.len());
    }

    pub fn is_finished(&self) -> bool {
        self.position >= self.samples.len()
    }

    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }

    pub fn position_secs(&self) -> f32 {
        self.position as f32 / self.sample_rate as f32
    }
}

impl BeamSource for AudioSource {
    fn generate(&mut self, count: usize, _beam: &BeamState) -> Vec<BeamSample> {
        let dt = 1.0 / self.sample_rate as f32;
        let remaining = self.samples.len().saturating_sub(self.position);
        let n = count.min(remaining);

        let result = self.samples[self.position..self.position + n]
            .iter()
            .map(|&(l, r)| BeamSample {
                x: (l + 1.0) / 2.0,
                y: (r + 1.0) / 2.0,
                intensity: 1.0,
                dt,
            })
            .collect();

        self.position += n;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BEAM: BeamState = BeamState { spot_radius: 0.001 };

    /// Create a minimal WAV file with known content (IEEE float, stereo).
    fn make_test_wav(samples: &[(f32, f32)], sample_rate: u32) -> Vec<u8> {
        let num_samples = samples.len() as u32;
        let data_size = num_samples * 2 * 4; // 2 channels, 4 bytes per f32
        let file_size = 36 + data_size;

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        buf.extend_from_slice(&2u16.to_le_bytes()); // stereo
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2 * 4).to_le_bytes());
        buf.extend_from_slice(&8u16.to_le_bytes());
        buf.extend_from_slice(&32u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for (l, r) in samples {
            buf.extend_from_slice(&l.to_le_bytes());
            buf.extend_from_slice(&r.to_le_bytes());
        }
        buf
    }

    #[test]
    fn audio_source_maps_lr_to_xy() {
        let test_samples = vec![(0.0, 0.0), (1.0, -1.0), (-1.0, 1.0)];
        let wav = make_test_wav(&test_samples, 44100);
        let tmp = std::env::temp_dir().join("phosphor_test_audio.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let mut src = AudioSource::load(&tmp).unwrap();
        let beams = src.generate(3, &TEST_BEAM);

        assert!((beams[0].x - 0.5).abs() < 0.01); // (0,0) -> (0.5, 0.5)
        assert!((beams[0].y - 0.5).abs() < 0.01);
        assert!((beams[1].x - 1.0).abs() < 0.01); // (1,-1) -> (1.0, 0.0)
        assert!((beams[1].y - 0.0).abs() < 0.01);
        assert!((beams[2].x - 0.0).abs() < 0.01); // (-1,1) -> (0.0, 1.0)
        assert!((beams[2].y - 1.0).abs() < 0.01);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn audio_source_dt_matches_sample_rate() {
        let silence = vec![(0.0, 0.0); 100];
        let wav = make_test_wav(&silence, 48000);
        let tmp = std::env::temp_dir().join("phosphor_test_dt.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let mut src = AudioSource::load(&tmp).unwrap();
        let beams = src.generate(10, &TEST_BEAM);
        for b in &beams {
            assert!((b.dt - 1.0 / 48000.0).abs() < 1e-9);
        }

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn audio_source_seek() {
        let silence = vec![(0.0, 0.0); 1000];
        let wav = make_test_wav(&silence, 44100);
        let tmp = std::env::temp_dir().join("phosphor_test_seek.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let mut src = AudioSource::load(&tmp).unwrap();
        src.seek(0.5);
        assert!((src.position_secs() - src.duration_secs() * 0.5).abs() < 0.01);

        std::fs::remove_file(&tmp).ok();
    }
}
