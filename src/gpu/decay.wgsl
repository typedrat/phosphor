// Decay Compute Shader
//
// Applies exponential decay to all accumulation texture layers each frame.
// Each texel: value *= exp(-dt / tau)
// Values below threshold are zeroed to prevent floating-point drift
// from accumulating imperceptible energy over thousands of frames.

override SPECTRAL_BANDS: u32 = 16u;

struct DecayParams {
    dt: f32,
    threshold: f32,
    tau_fast: f32,
    tau_slow: f32,
}

@group(0) @binding(0) var<uniform> params: DecayParams;

// Single 2D array texture: layers 0..N-1 = fast, N..2N-1 = slow.
@group(1) @binding(0) var accum: texture_storage_2d_array<r32float, read_write>;

fn decay_value(value: f32, factor: f32, threshold: f32) -> f32 {
    let decayed = value * factor;
    return select(decayed, 0.0, decayed < threshold);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = vec2<i32>(global_id.xy);
    let dims = textureDimensions(accum);

    if coord.x >= i32(dims.x) || coord.y >= i32(dims.y) {
        return;
    }

    let fast_factor = exp(-params.dt / params.tau_fast);
    let slow_factor = exp(-params.dt / params.tau_slow);
    let threshold = params.threshold;

    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        // Fast decay layers
        let fast_val = textureLoad(accum, coord, band).r;
        textureStore(accum, coord, band, vec4<f32>(decay_value(fast_val, fast_factor, threshold), 0.0, 0.0, 0.0));

        // Slow decay layers
        let slow_layer = SPECTRAL_BANDS + band;
        let slow_val = textureLoad(accum, coord, slow_layer).r;
        textureStore(accum, coord, slow_layer, vec4<f32>(decay_value(slow_val, slow_factor, threshold), 0.0, 0.0, 0.0));
    }
}
