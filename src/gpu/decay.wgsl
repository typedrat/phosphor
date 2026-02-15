// Decay Compute Shader
//
// Applies exponential decay to all accumulation textures each frame.
// Each texel: value *= exp(-dt / tau)
// Values below threshold are zeroed to prevent floating-point drift
// from accumulating imperceptible energy over thousands of frames.

struct DecayParams {
    dt: f32,
    threshold: f32,
    tau_fast: f32,
    tau_slow: f32,
}

@group(0) @binding(0) var<uniform> params: DecayParams;

// Accumulation textures: 0-3 = fast decay, 4-7 = slow decay.
@group(1) @binding(0) var accum_0: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(1) var accum_1: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(2) var accum_2: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(3) var accum_3: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(4) var accum_4: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(5) var accum_5: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(6) var accum_6: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(7) var accum_7: texture_storage_2d<rgba32float, read_write>;

fn decay_texel(value: vec4<f32>, factor: f32, threshold: f32) -> vec4<f32> {
    let decayed = value * factor;
    // Zero out components below threshold to prevent drift
    return select(decayed, vec4<f32>(0.0), decayed < vec4<f32>(threshold));
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = vec2<i32>(global_id.xy);
    let dims = textureDimensions(accum_0);

    if coord.x >= i32(dims.x) || coord.y >= i32(dims.y) {
        return;
    }

    let fast_factor = exp(-params.dt / params.tau_fast);
    let slow_factor = exp(-params.dt / params.tau_slow);
    let threshold = params.threshold;

    // Fast decay textures (0-3)
    textureStore(accum_0, coord, decay_texel(textureLoad(accum_0, coord), fast_factor, threshold));
    textureStore(accum_1, coord, decay_texel(textureLoad(accum_1, coord), fast_factor, threshold));
    textureStore(accum_2, coord, decay_texel(textureLoad(accum_2, coord), fast_factor, threshold));
    textureStore(accum_3, coord, decay_texel(textureLoad(accum_3, coord), fast_factor, threshold));

    // Slow decay textures (4-7)
    textureStore(accum_4, coord, decay_texel(textureLoad(accum_4, coord), slow_factor, threshold));
    textureStore(accum_5, coord, decay_texel(textureLoad(accum_5, coord), slow_factor, threshold));
    textureStore(accum_6, coord, decay_texel(textureLoad(accum_6, coord), slow_factor, threshold));
    textureStore(accum_7, coord, decay_texel(textureLoad(accum_7, coord), slow_factor, threshold));
}
