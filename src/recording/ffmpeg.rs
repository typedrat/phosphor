use std::io::{BufReader, Read as _, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// FfmpegProgress
// ---------------------------------------------------------------------------

/// Parsed ffmpeg encoding progress.
#[derive(Clone, Default)]
pub struct FfmpegProgress {
    pub encode_fps: f64,
    pub bitrate_kbps: f64,
    /// Output size in bytes.
    pub output_size_bytes: u64,
    pub speed: f64,
}

/// Parse ffmpeg's size string (e.g. "256KiB", "1024KiB", "10MiB") to bytes.
fn parse_size_to_bytes(s: &str) -> u64 {
    if let Some(v) = s.strip_suffix("GiB") {
        v.trim().parse::<f64>().unwrap_or(0.0) as u64 * 1024 * 1024 * 1024
    } else if let Some(v) = s.strip_suffix("MiB") {
        v.trim().parse::<f64>().unwrap_or(0.0) as u64 * 1024 * 1024
    } else if let Some(v) = s.strip_suffix("KiB") {
        v.trim().parse::<f64>().unwrap_or(0.0) as u64 * 1024
    } else if let Some(v) = s.strip_suffix("kB") {
        v.trim().parse::<f64>().unwrap_or(0.0) as u64 * 1000
    } else {
        s.trim().parse::<u64>().unwrap_or(0)
    }
}

/// Parse a single ffmpeg progress line into an `FfmpegProgress`.
///
/// ffmpeg progress lines look like:
/// `frame=  123 fps= 45 q=25.4 size=     256KiB time=00:00:02.05 bitrate=1024.0kbits/s speed=0.9x`
fn parse_ffmpeg_progress(line: &str) -> Option<FfmpegProgress> {
    if !line.contains("frame=") {
        return None;
    }

    let mut progress = FfmpegProgress::default();

    // Helper: find "key=" then grab the next non-whitespace token.
    let extract = |key: &str| -> Option<&str> {
        let idx = line.find(key)?;
        let after = &line[idx + key.len()..];
        let trimmed = after.trim_start();
        trimmed.split_whitespace().next()
    };

    if let Some(v) = extract("fps=") {
        progress.encode_fps = v.parse().unwrap_or(0.0);
    }
    if let Some(v) = extract("size=") {
        progress.output_size_bytes = parse_size_to_bytes(v);
    }
    if let Some(v) = extract("bitrate=") {
        // e.g. "1024.0kbits/s" or "N/A"
        if let Some(stripped) = v.strip_suffix("kbits/s") {
            progress.bitrate_kbps = stripped.parse().unwrap_or(0.0);
        }
    }
    if let Some(v) = extract("speed=") {
        // e.g. "0.9x" or "N/A"
        if let Some(stripped) = v.strip_suffix('x') {
            progress.speed = stripped.parse().unwrap_or(0.0);
        }
    }

    Some(progress)
}

/// Read ffmpeg stderr byte-by-byte, splitting on `\r` or `\n`, and update
/// the shared progress. Header lines (before any `frame=`) are logged.
fn stderr_reader_loop(stderr: impl std::io::Read, progress: Arc<Mutex<FfmpegProgress>>) {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::with_capacity(512);

    // Read byte-by-byte since ffmpeg uses \r for progress lines.
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte) {
            Ok(0) => break, // EOF
            Ok(_) => {
                if byte[0] == b'\r' || byte[0] == b'\n' {
                    if buf.is_empty() {
                        continue;
                    }
                    let line = String::from_utf8_lossy(&buf).to_string();
                    if let Some(p) = parse_ffmpeg_progress(&line) {
                        if let Ok(mut guard) = progress.lock() {
                            *guard = p;
                        }
                    } else {
                        // Header / info line — log it
                        info!(target: "ffmpeg", "{}", line.trim());
                    }
                    buf.clear();
                } else {
                    buf.push(byte[0]);
                }
            }
            Err(e) => {
                warn!("ffmpeg stderr read error: {e}");
                break;
            }
        }
    }

    // Flush any remaining partial line
    if !buf.is_empty() {
        let line = String::from_utf8_lossy(&buf).to_string();
        if let Some(p) = parse_ffmpeg_progress(&line) {
            if let Ok(mut guard) = progress.lock() {
                *guard = p;
            }
        } else {
            info!(target: "ffmpeg", "{}", line.trim());
        }
    }
}

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
    /// H.265 via NVENC with HDR10 — fast GPU encoding (NVIDIA only)
    H265NvencHdr10,
    /// AV1 via NVENC with HDR10 — fast GPU encoding (NVIDIA 50-series+)
    Av1NvencHdr10,
}

impl EncodingPreset {
    pub const ALL: &'static [Self] = &[
        Self::H265Hdr10,
        Self::H265Sdr,
        Self::Av1Hdr10,
        Self::ProRes4444,
        Self::H265NvencHdr10,
        Self::Av1NvencHdr10,
    ];

    /// Short identifier used in CLI flags and file-name hints.
    pub fn slug(&self) -> &'static str {
        match self {
            Self::H265Hdr10 => "h265-hdr10",
            Self::H265Sdr => "h265-sdr",
            Self::Av1Hdr10 => "av1-hdr10",
            Self::ProRes4444 => "prores-4444",
            Self::H265NvencHdr10 => "h265-nvenc-hdr10",
            Self::Av1NvencHdr10 => "av1-nvenc-hdr10",
        }
    }

    /// Human-readable description shown in the UI.
    pub fn description(&self) -> &'static str {
        match self {
            Self::H265Hdr10 => "H.265 HDR10 (HEVC, BT.2020 PQ)",
            Self::H265Sdr => "H.265 SDR (HEVC, tonemapped BT.709)",
            Self::Av1Hdr10 => "AV1 HDR10 (SVT-AV1, BT.2020 PQ)",
            Self::ProRes4444 => "ProRes 4444 (lossless-quality, large)",
            Self::H265NvencHdr10 => "H.265 HDR10 NVENC (GPU, fast)",
            Self::Av1NvencHdr10 => "AV1 HDR10 NVENC (GPU, fast)",
        }
    }

    /// Default file extension (without leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            Self::H265Hdr10 | Self::H265Sdr | Self::H265NvencHdr10 => "mp4",
            Self::Av1Hdr10 | Self::Av1NvencHdr10 => "mkv",
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
            Self::H265NvencHdr10 => vec![
                "-c:v",
                "hevc_nvenc",
                "-pix_fmt",
                "p010le",
                "-colorspace",
                "bt2020nc",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
                "-rc",
                "constqp",
                "-qp",
                "18",
                "-preset",
                "p7",
                "-tier",
                "high",
                "-profile:v",
                "main10",
            ],
            Self::Av1NvencHdr10 => vec![
                "-c:v",
                "av1_nvenc",
                "-pix_fmt",
                "p010le",
                "-colorspace",
                "bt2020nc",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
                "-rc",
                "constqp",
                "-qp",
                "18",
                "-preset",
                "p7",
                "-highbitdepth",
                "1",
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

/// Verify that `ffmpeg` is on `PATH` and supports the `rgbf16le` pixel format.
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

    // Check that rgbf16le is a supported pixel format.
    let pix_output = Command::new("ffmpeg")
        .args(["-pix_fmts"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to query ffmpeg pixel formats: {e}"))?;

    let pix_list = String::from_utf8_lossy(&pix_output.stdout);
    if !pix_list.contains("rgbf16le") {
        return Err("ffmpeg build does not support rgbf16le pixel format — \
             a recent build with full codec support is required"
            .to_string());
    }

    info!("ffmpeg found and supports rgbf16le");
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

/// Bytes per pixel in the GPU staging buffer (Rgba16Float = 4 × f16).
const GPU_BYTES_PER_PIXEL: u32 = 8;
/// Bytes per pixel piped to ffmpeg (RGB f16, alpha stripped = 3 × f16).
const PIPE_BYTES_PER_PIXEL: u32 = 6;

/// Row stride in bytes for a single `Rgba16Float` (8 bytes per pixel) row
/// padded to wgpu's copy-row-alignment requirement.
fn padded_row_bytes(width: u32) -> u32 {
    let unpadded = width * GPU_BYTES_PER_PIXEL;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(align) * align
}

/// Active ffmpeg child process receiving raw frames on stdin.
///
/// The GPU produces `Rgba16Float` (8 bytes/pixel) but we strip the alpha
/// channel and pipe `rgbf16le` (6 bytes/pixel) to ffmpeg, saving 25% bandwidth.
pub struct FfmpegPipe {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Width in pixels.
    width: u32,
    /// Padded bytes per row in the GPU staging buffer.
    pub padded_bytes_per_row: u32,
    /// Pre-allocated row buffer for alpha stripping (width * 6 bytes).
    row_buf: Vec<u8>,
    /// Shared encoding progress parsed from ffmpeg stderr.
    pub progress: Arc<Mutex<FfmpegProgress>>,
    /// Handle for the stderr reader thread.
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl FfmpegPipe {
    /// Spawn an ffmpeg child process configured from `config`.
    pub fn spawn(config: &FfmpegConfig) -> Result<Self, String> {
        let padded_bytes_per_row = padded_row_bytes(config.width);

        // Base input arguments: read raw video from stdin.
        // We pipe rgbf16le (RGB, no alpha) — alpha is stripped in write_frame().
        let fps_str = format!("{}", config.fps);
        let size_str = format!("{}x{}", config.width, config.height);

        let mut args: Vec<String> = vec![
            "-y".into(),
            // Raw video input from stdin
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgbf16le".into(),
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
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;

        let stdin = child.stdin.take();

        // Spawn a thread to read stderr and parse progress lines.
        let progress = Arc::new(Mutex::new(FfmpegProgress::default()));
        let stderr_thread = {
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| "failed to capture ffmpeg stderr".to_string())?;
            let progress = Arc::clone(&progress);
            Some(
                std::thread::Builder::new()
                    .name("ffmpeg-stderr".into())
                    .spawn(move || stderr_reader_loop(stderr, progress))
                    .map_err(|e| format!("failed to spawn stderr reader: {e}"))?,
            )
        };

        let row_buf = vec![0u8; config.width as usize * PIPE_BYTES_PER_PIXEL as usize];

        Ok(Self {
            child,
            stdin,
            width: config.width,
            padded_bytes_per_row,
            row_buf,
            progress,
            stderr_thread,
        })
    }

    /// Write one frame to ffmpeg's stdin, stripping row padding AND alpha.
    ///
    /// The GPU staging buffer contains `Rgba16Float` (8 bytes/pixel) with
    /// potential row padding. We write `rgbf16le` (6 bytes/pixel, no alpha,
    /// no padding) to ffmpeg — saving 25% pipe bandwidth.
    pub fn write_frame(&mut self, frame_data: &[u8], height: u32) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "ffmpeg stdin is closed".to_string())?;

        let pbpr = self.padded_bytes_per_row as usize;
        let width = self.width as usize;
        let gpu_bpp = GPU_BYTES_PER_PIXEL as usize;
        let pipe_bpp = PIPE_BYTES_PER_PIXEL as usize;

        let row_buf = &mut self.row_buf;

        for row in 0..height as usize {
            let row_start = row * pbpr;
            for px in 0..width {
                let src = row_start + px * gpu_bpp;
                let dst = px * pipe_bpp;
                row_buf[dst..dst + pipe_bpp].copy_from_slice(&frame_data[src..src + pipe_bpp]);
            }
            stdin
                .write_all(row_buf)
                .map_err(|e| format!("write row {row} to ffmpeg failed: {e}"))?;
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

        // Join the stderr reader thread.
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }

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

// ---------------------------------------------------------------------------
// PipeWriterThread
// ---------------------------------------------------------------------------

/// Background thread that writes frame data to ffmpeg stdin.
///
/// Receives `Vec<u8>` frame buffers over a bounded channel and writes them
/// sequentially, decoupling GPU readback from I/O latency.
pub struct PipeWriterThread {
    sender: crossbeam_channel::Sender<Vec<u8>>,
    handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl PipeWriterThread {
    /// Spawn the background writer thread. Ownership of the `FfmpegPipe` moves
    /// into the thread; it will be finished when the channel closes.
    ///
    /// `buffer_frames`: number of frames to buffer. Use -1 for unbounded.
    pub fn spawn(mut pipe: FfmpegPipe, height: u32, buffer_frames: i32) -> Self {
        let (sender, receiver) = if buffer_frames < 0 {
            crossbeam_channel::unbounded::<Vec<u8>>()
        } else {
            crossbeam_channel::bounded::<Vec<u8>>(buffer_frames.max(1) as usize)
        };

        let handle = std::thread::Builder::new()
            .name("pipe-writer".into())
            .spawn(move || {
                for frame_data in receiver {
                    pipe.write_frame(&frame_data, height)
                        .map_err(|e| format!("pipe writer: {e}"))?;
                }
                pipe.finish()
            })
            .expect("failed to spawn pipe-writer thread");

        Self {
            sender,
            handle: Some(handle),
        }
    }

    /// Send frame data to the writer thread (may block if the channel is full).
    pub fn send(&self, data: Vec<u8>) -> Result<(), String> {
        self.sender
            .send(data)
            .map_err(|_| "pipe writer thread has exited".to_string())
    }

    /// Drop the sender (signals EOF to the thread), join, and return any error.
    pub fn finish(mut self) -> Result<(), String> {
        // Drop sender so the receiver loop exits.
        drop(std::mem::replace(
            &mut self.sender,
            crossbeam_channel::bounded(0).0,
        ));

        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| "pipe writer thread panicked".to_string())?
        } else {
            Ok(())
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

        // Join the stderr reader thread.
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}
