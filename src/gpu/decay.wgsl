// Decay Compute Shader
//
// Applies exponential decay to all accumulation buffer layers each frame.
// Each element: value *= exp(-dt / tau)
// Values below threshold are zeroed to prevent floating-point drift
// from accumulating imperceptible energy over thousands of frames.

override SPECTRAL_BANDS: u32 = 16u;

struct DecayParams {
    dt: f32,
    threshold: f32,
    tau_fast: f32,
    tau_slow: f32,
}

struct AccumDims {
    width: u32,
    height: u32,
    layers: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: DecayParams;

@group(1) @binding(0) var<storage, read_write> accum: array<u32>;
@group(1) @binding(1) var<uniform> accum_dims: AccumDims;

fn accum_index(x: i32, y: i32, layer: u32) -> u32 {
    return layer * (accum_dims.width * accum_dims.height) + u32(y) * accum_dims.width + u32(x);
}

fn load_accum(x: i32, y: i32, layer: u32) -> f32 {
    return bitcast<f32>(accum[accum_index(x, y, layer)]);
}

fn store_accum(x: i32, y: i32, layer: u32, val: f32) {
    accum[accum_index(x, y, layer)] = bitcast<u32>(val);
}

fn decay_value(value: f32, factor: f32, threshold: f32) -> f32 {
    let decayed = value * factor;
    return select(decayed, 0.0, decayed < threshold);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = vec2<i32>(global_id.xy);

    if coord.x >= i32(accum_dims.width) || coord.y >= i32(accum_dims.height) {
        return;
    }

    let fast_factor = exp(-params.dt / params.tau_fast);
    let slow_factor = exp(-params.dt / params.tau_slow);
    let threshold = params.threshold;

    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        // Fast decay layers
        let fast_val = load_accum(coord.x, coord.y, band);
        store_accum(coord.x, coord.y, band, decay_value(fast_val, fast_factor, threshold));

        // Slow decay layers
        let slow_layer = SPECTRAL_BANDS + band;
        let slow_val = load_accum(coord.x, coord.y, slow_layer);
        store_accum(coord.x, coord.y, slow_layer, decay_value(slow_val, slow_factor, threshold));
    }
}
