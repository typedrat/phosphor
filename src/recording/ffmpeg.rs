use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// EncodingPreset
// ---------------------------------------------------------------------------

/// Available encoding presets for video output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodingPreset {
    /// H.265 (HEVC) with HDR10 metadata — default, highest quality
    #[default]
    H265Hdr10,
    /// H.265 (HEVC) SDR with zscale tonemapping
    H265Sdr,
    /// AV1 (SVT-AV1) with HDR10 metadata
    Av1Hdr10,
    /// Apple ProRes 4444 — lossless-quality, large files
    ProRes4444,
}

impl EncodingPreset {
    pub const ALL: &'static [Self] = &[
        Self::H265Hdr10,
        Self::H265Sdr,
        Self::Av1Hdr10,
        Self::ProRes4444,
    ];

    /// Short identifier used in CLI flags and file-name hints.
    pub fn slug(&self) -> &'static str {
        match self {
            Self::H265Hdr10 => "h265-hdr10",
            Self::H265Sdr => "h265-sdr",
            Self::Av1Hdr10 => "av1-hdr10",
            Self::ProRes4444 => "prores-4444",
        }
    }

    /// Human-readable description shown in the UI.
    pub fn description(&self) -> &'static str {
        match self {
            Self::H265Hdr10 => "H.265 HDR10 (HEVC, BT.2020 PQ)",
            Self::H265Sdr => "H.265 SDR (HEVC, tonemapped BT.709)",
            Self::Av1Hdr10 => "AV1 HDR10 (SVT-AV1, BT.2020 PQ)",
            Self::ProRes4444 => "ProRes 4444 (lossless-quality, large)",
        }
    }

    /// Default file extension (without leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            Self::H265Hdr10 | Self::H265Sdr => "mp4",
            Self::Av1Hdr10 => "mkv",
            Self::ProRes4444 => "mov",
        }
    }

    /// Codec and color-metadata arguments for ffmpeg (inserted after `-i` / before output).
    ///
    /// The pixel format passed on the command line is `yuv420p10le` for HDR
    /// presets and `yuv420p` for SDR — callers should note that we set the
    /// pixel format here as well.
    pub fn video_args(&self) -> Vec<&'static str> {
        match self {
            Self::H265Hdr10 => vec![
                "-c:v",
                "libx265",
                "-pix_fmt",
                "yuv420p10le",
                "-colorspace",
                "bt2020nc",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
                "-x265-params",
                "hdr-opt=1:repeat-headers=1:\
                 master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):\
                 max-cll=1000,400:\
                 colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc",
                "-crf",
                "18",
                "-preset",
                "slow",
            ],
            Self::H265Sdr => vec![
                "-vf",
                "zscale=t=linear:npl=100,format=gbrpf32le,\
                 zscale=p=bt709,tonemap=tonemap=hable:desat=0,\
                 zscale=t=bt709:m=bt709:r=tv,format=yuv420p",
                "-c:v",
                "libx265",
                "-pix_fmt",
                "yuv420p",
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-crf",
                "18",
                "-preset",
                "slow",
            ],
            Self::Av1Hdr10 => vec![
                "-c:v",
                "libsvtav1",
                "-pix_fmt",
                "yuv420p10le",
                "-colorspace",
                "bt2020nc",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
                "-svtav1-params",
                "hdr=1:mastering-display=G(0.265,0.690)B(0.150,0.060)R(0.680,0.320)WP(0.3127,0.3290)L(1000,0.0001):content-light=1000:400",
                "-crf",
                "28",
                "-preset",
                "6",
            ],
            Self::ProRes4444 => vec![
                "-c:v",
                "prores_ks",
                "-profile:v",
                "4444",
                "-pix_fmt",
                "yuva444p10le",
                "-vendor",
                "apl0",
                "-bits_per_mb",
                "8000",
            ],
        }
    }

    /// Parse a preset from its slug string.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.slug() == slug)
    }
}

// ---------------------------------------------------------------------------
// ffmpeg availability check
// ---------------------------------------------------------------------------

/// Verify that `ffmpeg` is on `PATH` and supports the `rgbaf16le` pixel format.
///
/// Returns `Ok(())` on success, or an error string describing what is missing.
pub fn check_ffmpeg() -> Result<(), String> {
    // Check that ffmpeg is on PATH at all.
    let version_output = Command::new("ffmpeg")
        .args(["-version"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("ffmpeg not found on PATH: {e}"))?;

    if !version_output.status.success() {
        return Err("ffmpeg -version exited with non-zero status".to_string());
    }

    // Check that rgbaf16le is a supported pixel format.
    let pix_output = Command::new("ffmpeg")
        .args(["-pix_fmts"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to query ffmpeg pixel formats: {e}"))?;

    let pix_list = String::from_utf8_lossy(&pix_output.stdout);
    if !pix_list.contains("rgbaf16le") {
        return Err("ffmpeg build does not support rgbaf16le pixel format — \
             a recent build with full codec support is required"
            .to_string());
    }

    info!("ffmpeg found and supports rgbaf16le");
    Ok(())
}

// ---------------------------------------------------------------------------
// FfmpegConfig
// ---------------------------------------------------------------------------

/// Configuration for a single recording session.
#[derive(Debug, Clone)]
pub struct FfmpegConfig {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: f64,
    /// Destination file path (extension should match the preset).
    pub output_path: PathBuf,
    /// Optional path to an audio file to mux into the output.
    pub audio_path: Option<PathBuf>,
    /// Encoding preset to use (ignored when `custom_args` is `Some`).
    pub preset: EncodingPreset,
    /// Optional fully-custom codec argument list that overrides `preset`.
    pub custom_args: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// FfmpegPipe
// ---------------------------------------------------------------------------

/// Row stride in bytes for a single `Rgba16Float` (8 bytes per pixel) row
/// padded to wgpu's copy-row-alignment requirement.
fn padded_row_bytes(width: u32) -> u32 {
    let unpadded = width * 8; // 4 channels × 2 bytes each
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(align) * align
}

/// Active ffmpeg child process receiving raw `Rgba16Float` frames on stdin.
pub struct FfmpegPipe {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Unpadded bytes per row (width × 8).
    pub bytes_per_row: u32,
    /// Padded bytes per row aligned to `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`.
    pub padded_bytes_per_row: u32,
}

impl FfmpegPipe {
    /// Spawn an ffmpeg child process configured from `config`.
    pub fn spawn(config: &FfmpegConfig) -> Result<Self, String> {
        let bytes_per_row = config.width * 8;
        let padded_bytes_per_row = padded_row_bytes(config.width);

        // Base input arguments: read raw video from stdin.
        let fps_str = format!("{}", config.fps);
        let size_str = format!("{}x{}", config.width, config.height);

        let mut args: Vec<String> = vec![
            "-y".into(),
            // Raw video input from stdin
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgbaf16le".into(),
            "-s".into(),
            size_str,
            "-r".into(),
            fps_str,
            "-i".into(),
            "pipe:0".into(),
        ];

        // Optional audio input.
        let has_audio = config.audio_path.is_some();
        if let Some(ref audio_path) = config.audio_path {
            args.push("-i".into());
            args.push(audio_path.to_string_lossy().into_owned());
        }

        // Codec / preset arguments.
        match &config.custom_args {
            Some(custom) => {
                args.extend(custom.iter().cloned());
            }
            None => {
                args.extend(config.preset.video_args().iter().map(|s| s.to_string()));
            }
        }

        // Audio codec: select based on container.
        if has_audio {
            let ext = config
                .output_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let audio_codec = match ext {
                "mov" => "pcm_s16le",
                "mkv" => "flac",
                _ => "aac", // mp4 default
            };
            args.push("-c:a".into());
            args.push(audio_codec.into());
        }

        // Output path.
        args.push(config.output_path.to_string_lossy().into_owned());

        info!(
            path = %config.output_path.display(),
            width = config.width,
            height = config.height,
            fps = config.fps,
            preset = config.preset.slug(),
            "spawning ffmpeg"
        );

        let mut child = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;

        let stdin = child.stdin.take();

        Ok(Self {
            child,
            stdin,
            bytes_per_row,
            padded_bytes_per_row,
        })
    }

    /// Write one frame to ffmpeg's stdin, stripping row padding if present.
    ///
    /// `frame_data` must be exactly `padded_bytes_per_row * height` bytes.
    pub fn write_frame(&mut self, frame_data: &[u8], height: u32) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "ffmpeg stdin is closed".to_string())?;

        if self.padded_bytes_per_row == self.bytes_per_row {
            // No padding — write the whole buffer in one shot.
            stdin
                .write_all(frame_data)
                .map_err(|e| format!("write to ffmpeg stdin failed: {e}"))?;
        } else {
            // Strip padding row by row.
            let bpr = self.bytes_per_row as usize;
            let pbpr = self.padded_bytes_per_row as usize;
            for row in 0..height as usize {
                let start = row * pbpr;
                let end = start + bpr;
                if end > frame_data.len() {
                    return Err(format!(
                        "frame_data too short: expected at least {end} bytes, got {}",
                        frame_data.len()
                    ));
                }
                stdin
                    .write_all(&frame_data[start..end])
                    .map_err(|e| format!("write row {row} to ffmpeg stdin failed: {e}"))?;
            }
        }

        Ok(())
    }

    /// Close stdin and wait for ffmpeg to exit, returning its exit status.
    pub fn finish(mut self) -> Result<(), String> {
        // Drop stdin to signal EOF.
        drop(self.stdin.take());

        info!("waiting for ffmpeg to finish encoding…");
        let status = self
            .child
            .wait()
            .map_err(|e| format!("failed to wait for ffmpeg: {e}"))?;

        if status.success() {
            info!("ffmpeg finished successfully");
            Ok(())
        } else {
            let msg = format!("ffmpeg exited with status: {status}");
            error!("{msg}");
            Err(msg)
        }
    }
}

impl Drop for FfmpegPipe {
    fn drop(&mut self) {
        // Ensure stdin is closed so ffmpeg isn't left waiting.
        drop(self.stdin.take());

        // Wait for ffmpeg to finish writing the container trailer.
        // Don't kill it — it needs time to flush after stdin closes.
        match self.child.wait() {
            Ok(status) => {
                if !status.success() {
                    warn!("ffmpeg exited with status {status} during drop");
                }
            }
            Err(e) => {
                warn!("could not wait for ffmpeg on drop: {e}");
            }
        }
    }
}
