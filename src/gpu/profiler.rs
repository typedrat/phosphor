use bytemuck::cast_slice;
use strum::{Display, EnumCount, EnumIter, IntoEnumIterator, VariantNames};

// ------------------ GpuQuery ------------------

#[derive(Debug, Clone, Copy, EnumCount, EnumIter, Display, VariantNames, PartialEq, Eq)]
#[repr(u32)]
pub enum GpuQuery {
    #[strum(serialize = "Frame Start")]
    FrameStart = 0,
    #[strum(serialize = "Beam Write")]
    AfterBeamWrite = 1,
    #[strum(serialize = "Spectral Resolve")]
    AfterSpectralResolve = 2,
    #[strum(serialize = "Decay")]
    AfterDecay = 3,
    #[strum(serialize = "Faceplate Scatter")]
    AfterFaceplateScatter = 4,
    #[strum(serialize = "Composite")]
    AfterComposite = 5,
}

const QUERY_COUNT: u32 = GpuQuery::COUNT as u32;
const RESULT_SIZE: u64 = QUERY_COUNT as u64 * 8;

/// Number of timed segments (one between each consecutive pair of timestamps).
pub const NUM_SEGMENTS: usize = GpuQuery::COUNT - 1;

/// Segment names derived from `GpuQuery::VARIANTS` (excludes FrameStart).
pub const SEGMENT_NAMES: &[&str] = {
    let v = GpuQuery::VARIANTS;
    v.split_at(1).1
};

// ------------------ SoA timing history ------------------

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const HISTORY_CAP: usize = 512; // power of 2
const CAP: usize = HISTORY_CAP;
const CAP_MASK: usize = CAP - 1;

/// Number of f32 tracks: one per segment + one for totals.
const F32_TRACKS: usize = NUM_SEGMENTS + 1;
const TOTAL_TRACK: usize = NUM_SEGMENTS;

/// Single-producer lock-free ring buffer of per-frame GPU timings in SoA layout.
///
/// A single flat allocation holds all tracks contiguously:
///   `[seg0 × CAP | seg1 × CAP | ... | total × CAP]`
///
/// Each track's CAP-element region shares a common atomic write cursor (`tail`),
/// so iterating one track across all frames is a contiguous memory read
/// (at most two slices for the ring wrap).
///
/// # Thread safety
/// - **Single writer**: `push()` takes `&self` via `UnsafeCell`; the caller
///   (`GpuProfiler::read_back`) holds `&mut GpuProfiler`, guaranteeing
///   exclusive writer access.
/// - **Multiple readers**: `len()`, `track_slices()`, `segment_iter()`, etc.
///   load `tail` with `Acquire` before reading data.
/// - Data writes complete before `tail` is incremented (`Release` ordering),
///   so readers never observe a partially-written frame.
///
/// # Invariants
/// - `CAP` is a power of 2; `CAP_MASK = CAP - 1` for fast modulo.
/// - Write index = `tail & CAP_MASK`.
/// - `len = min(tail, CAP)`.
/// - When `tail > CAP` the ring has wrapped: oldest frame is at
///   `tail & CAP_MASK`, newest at `(tail - 1) & CAP_MASK`.
pub struct TimingHistory {
    buf: UnsafeCell<Box<[f32]>>,      // F32_TRACKS * CAP
    beam_buf: UnsafeCell<Box<[u32]>>, // CAP
    tail: AtomicUsize,                // monotonically increasing write cursor
}

// Safety: single writer via &mut GpuProfiler; readers use Acquire on tail.
// The only possible torn read is the slot currently being overwritten when
// the ring wraps — acceptable for a diagnostic display.
unsafe impl Send for TimingHistory {}
unsafe impl Sync for TimingHistory {}

impl TimingHistory {
    pub fn new() -> Self {
        Self {
            buf: UnsafeCell::new(vec![0.0f32; F32_TRACKS * CAP].into_boxed_slice()),
            beam_buf: UnsafeCell::new(vec![0u32; CAP].into_boxed_slice()),
            tail: AtomicUsize::new(0),
        }
    }

    /// Push a new frame of timing data. **Single writer only.**
    pub fn push(&self, segments: [f32; NUM_SEGMENTS], total: f32, beam_samples: u32) {
        let idx = self.tail.load(Ordering::Relaxed) & CAP_MASK;

        // Safety: single writer guaranteed by &mut GpuProfiler in call chain.
        let buf = unsafe { &mut *self.buf.get() };
        for (i, &val) in segments.iter().enumerate() {
            buf[i * CAP + idx] = val;
        }
        buf[TOTAL_TRACK * CAP + idx] = total;

        let beam_buf = unsafe { &mut *self.beam_buf.get() };
        beam_buf[idx] = beam_samples;

        // Release: all writes above are visible before readers see the new tail.
        self.tail.fetch_add(1, Ordering::Release);
    }

    pub fn len(&self) -> usize {
        self.tail.load(Ordering::Acquire).min(CAP)
    }

    /// Returns the two ordered slices for a track's ring data `(older, newer)`.
    /// When the ring hasn't wrapped yet, returns `(data, &[])`.
    fn track_slices(&self, track: usize) -> (&[f32], &[f32]) {
        let tail_val = self.tail.load(Ordering::Acquire);
        let len = tail_val.min(CAP);
        let base = track * CAP;
        // Safety: read-only access; tail Acquire ensures data is visible.
        let buf = unsafe { &*self.buf.get() };

        if tail_val <= CAP {
            (&buf[base..base + len], &[])
        } else {
            let head = tail_val & CAP_MASK;
            (&buf[base + head..base + CAP], &buf[base..base + head])
        }
    }

    /// Iterate `[frame_index, value]` for a single segment — two contiguous reads.
    pub fn segment_iter(&self, seg: usize) -> impl Iterator<Item = [f64; 2]> + '_ {
        let (a, b) = self.track_slices(seg);
        a.iter()
            .chain(b.iter())
            .enumerate()
            .map(|(i, &v)| [i as f64, v as f64])
    }

    /// Average the last `n` values of a track (clamped to `len`).
    fn track_avg(&self, track: usize, n: usize) -> f32 {
        let (a, b) = self.track_slices(track);
        let total_len = a.len() + b.len();
        let n = n.min(total_len);
        if n == 0 {
            return 0.0;
        }
        // Take the last `n` values from the ordered (a, b) pair.
        let skip = total_len - n;
        let sum: f32 = a.iter().chain(b.iter()).skip(skip).sum();
        sum / n as f32
    }

    /// Latest total GPU time in microseconds, or 0 if empty.
    pub fn latest_total(&self) -> f32 {
        let tail_val = self.tail.load(Ordering::Acquire);
        if tail_val == 0 {
            return 0.0;
        }
        let prev = (tail_val - 1) & CAP_MASK;
        let buf = unsafe { &*self.buf.get() };
        buf[TOTAL_TRACK * CAP + prev]
    }

    /// Average total GPU time over the last `n` frames.
    pub fn avg_total(&self, n: usize) -> f32 {
        self.track_avg(TOTAL_TRACK, n)
    }

    /// Average per-segment values over the last `n` frames.
    pub fn avg_segments(&self, n: usize) -> Option<[(&'static str, f32); NUM_SEGMENTS]> {
        if self.len() == 0 {
            return None;
        }
        Some(std::array::from_fn(|i| {
            (SEGMENT_NAMES[i], self.track_avg(i, n))
        }))
    }

    /// Latest per-segment values as `(name, microseconds)` pairs.
    pub fn latest_segments(&self) -> Option<[(&'static str, f32); NUM_SEGMENTS]> {
        let tail_val = self.tail.load(Ordering::Acquire);
        if tail_val == 0 {
            return None;
        }
        let prev = (tail_val - 1) & CAP_MASK;
        let buf = unsafe { &*self.buf.get() };
        Some(std::array::from_fn(|i| {
            (SEGMENT_NAMES[i], buf[i * CAP + prev])
        }))
    }

    pub fn avg_beam_samples(&self, n: usize) -> f32 {
        let tail_val = self.tail.load(Ordering::Acquire);
        let count = tail_val.min(CAP).min(n);
        if count == 0 {
            return 0.0;
        }
        let beam_buf = unsafe { &*self.beam_buf.get() };
        let mut sum = 0u64;
        for i in 0..count {
            let idx = (tail_val.wrapping_sub(1 + i)) & CAP_MASK;
            sum += beam_buf[idx] as u64;
        }
        sum as f32 / count as f32
    }

    pub fn latest_beam_samples(&self) -> u32 {
        let tail_val = self.tail.load(Ordering::Acquire);
        if tail_val == 0 {
            return 0;
        }
        let prev = (tail_val - 1) & CAP_MASK;
        let beam_buf = unsafe { &*self.beam_buf.get() };
        beam_buf[prev]
    }
}

// ------------------ GpuProfiler ------------------

pub struct GpuProfiler {
    query_set: wgpu::QuerySet,
    resolve_buffer: wgpu::Buffer,
    read_buffer: wgpu::Buffer,
    timestamp_period: f32,
    has_data: bool,
    pub history: TimingHistory,
}

impl GpuProfiler {
    pub fn supports_timestamps(adapter: &wgpu::Adapter) -> bool {
        adapter.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        )
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu_profiler"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERY_COUNT,
        });

        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("profiler_resolve"),
            size: RESULT_SIZE,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let read_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("profiler_read"),
            size: RESULT_SIZE,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let timestamp_period = queue.get_timestamp_period();

        Self {
            query_set,
            resolve_buffer,
            read_buffer,
            timestamp_period,
            has_data: false,
            history: TimingHistory::new(),
        }
    }

    pub fn timestamp(&self, encoder: &mut wgpu::CommandEncoder, query: GpuQuery) {
        encoder.write_timestamp(&self.query_set, query as u32);
    }

    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(&self.query_set, 0..QUERY_COUNT, &self.resolve_buffer, 0);
        encoder.copy_buffer_to_buffer(&self.resolve_buffer, 0, &self.read_buffer, 0, RESULT_SIZE);
    }

    pub fn read_back(&mut self, device: &wgpu::Device, beam_samples: u32) {
        if !self.has_data {
            self.has_data = true;
            return;
        }

        self.read_buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());

        {
            let view = self.read_buffer.slice(..).get_mapped_range();
            let ts: &[u64] = cast_slice(&view);

            if ts.len() >= QUERY_COUNT as usize {
                let to_us = |a: usize, b: usize| {
                    ts[b].wrapping_sub(ts[a]) as f32 * self.timestamp_period / 1000.0
                };

                let mut segments = [0.0f32; NUM_SEGMENTS];
                let mut prev = 0;
                for (seg_idx, _variant) in GpuQuery::iter().enumerate().skip(1) {
                    segments[seg_idx - 1] = to_us(prev, seg_idx);
                    prev = seg_idx;
                }

                let total = to_us(0, QUERY_COUNT as usize - 1);
                self.history.push(segments, total, beam_samples);
            }
        }

        self.read_buffer.unmap();
    }
}
