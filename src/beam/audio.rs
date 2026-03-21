use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio_output::DecodedAudio;

/// Decode an audio file into interleaved stereo samples.
pub fn decode_audio_file(path: &Path) -> anyhow::Result<DecodedAudio> {
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

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

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

    // Ensure stereo output: mono → duplicate, >2ch → take first two
    let stereo_interleaved: Vec<f32> = match channels {
        1 => interleaved.iter().flat_map(|&s| [s, s]).collect(),
        2 => interleaved,
        n => interleaved
            .chunks_exact(n)
            .flat_map(|c| [c[0], c[1]])
            .collect(),
    };

    Ok(DecodedAudio {
        samples: Arc::from(stereo_interleaved),
        sample_rate,
        channels: 2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn decode_stereo_wav() {
        let test_samples = vec![(0.5, -0.5), (1.0, -1.0), (-1.0, 1.0)];
        let wav = make_test_wav(&test_samples, 44100);
        let tmp = std::env::temp_dir().join("phosphor_test_decode.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let decoded = decode_audio_file(&tmp).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.sample_rate, 44100);
        assert_eq!(decoded.samples.len(), 6);
        assert!((decoded.samples[0] - 0.5).abs() < 0.01);
        assert!((decoded.samples[1] - -0.5).abs() < 0.01);
        assert!((decoded.samples[2] - 1.0).abs() < 0.01);
        assert!((decoded.samples[3] - -1.0).abs() < 0.01);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn decode_sample_rate() {
        let silence = vec![(0.0, 0.0); 100];
        let wav = make_test_wav(&silence, 48000);
        let tmp = std::env::temp_dir().join("phosphor_test_decode_rate.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let decoded = decode_audio_file(&tmp).unwrap();
        assert_eq!(decoded.sample_rate, 48000);

        std::fs::remove_file(&tmp).ok();
    }
}
