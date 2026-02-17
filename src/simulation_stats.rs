use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;

/// Statistics shared between the simulation thread (writer) and
/// the render/UI thread (reader). All fields use relaxed atomics —
/// individual reads may be slightly stale but that's fine for display.
pub struct SimStats {
    /// Current adaptive batch interval in seconds.
    pub batch_interval: AtomicF32,
    /// Post-resample samples pushed per second (updated ~once per second).
    pub throughput: AtomicF32,
    /// Pre-resample samples generated per second (updated ~once per second).
    pub samples_generated: AtomicF32,
    /// Cumulative count of samples dropped due to full ring buffer.
    pub samples_dropped: AtomicU32,
    /// Ring buffer capacity.
    pub buffer_capacity: AtomicU32,
}

impl SimStats {
    pub fn new(buffer_capacity: u32) -> Arc<Self> {
        Arc::new(Self {
            batch_interval: AtomicF32::new(0.001),
            throughput: AtomicF32::new(0.0),
            samples_generated: AtomicF32::new(0.0),
            samples_dropped: AtomicU32::new(0),
            buffer_capacity: AtomicU32::new(buffer_capacity),
        })
    }
}
