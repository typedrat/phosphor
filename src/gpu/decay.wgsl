// Decay Compute Shader — Three-Tier Model
//
// Tier 1: Instantaneous exponentials (tau << frame dt) — handled at beam
//         write time as an energy boost, not stored in the accumulation buffer.
// Tier 2: Slow exponentials — multiplicative decay: value *= exp(-dt / tau)
// Tier 3: Power-law — elapsed time tracking: I(t) = peak * (alpha/(t+alpha))^beta

override SPECTRAL_BANDS: u32 = 16u;

struct DecayTermGpu {
    amplitude: f32,
    param1: f32,    // tau (exp) or alpha (power_law)
    param2: f32,    // 0.0 (exp) or beta (power_law)
    type_flag: f32, // 0.0 = exponential, 1.0 = power_law
}

struct DecayParams {
    dt: f32,
    threshold: f32,
    tau_cutoff: f32,
    term_count: u32,
    terms: array<DecayTermGpu, 8>,
    slow_exp_count: u32,
    has_power_law: u32,
    _pad0: u32,
    _pad1: u32,
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

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = vec2<i32>(global_id.xy);

    if coord.x >= i32(accum_dims.width) || coord.y >= i32(accum_dims.height) {
        return;
    }

    let threshold = params.threshold;

    // Tier 2: slow exponentials — multiplicative decay
    for (var term = 0u; term < params.slow_exp_count; term++) {
        let tau = params.terms[term].param1;
        let factor = exp(-params.dt / tau);
        for (var band = 0u; band < SPECTRAL_BANDS; band++) {
            let layer = term * SPECTRAL_BANDS + band;
            let val = load_accum(coord.x, coord.y, layer);
            let decayed = val * factor;
            store_accum(coord.x, coord.y, layer,
                select(decayed, 0.0, decayed < threshold));
        }
    }

    // Tier 3: power-law — elapsed time tracking
    if params.has_power_law == 1u {
        let base = params.slow_exp_count * SPECTRAL_BANDS;
        let time_layer = base + SPECTRAL_BANDS;

        var elapsed = load_accum(coord.x, coord.y, time_layer);
        elapsed += params.dt;
        store_accum(coord.x, coord.y, time_layer, elapsed);

        // Find the power-law term (first one with type_flag == 1.0)
        for (var i = 0u; i < params.term_count; i++) {
            if params.terms[i].type_flag == 1.0 {
                let alpha = params.terms[i].param1;
                let beta = params.terms[i].param2;

                // Threshold dead texels to save compute
                for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                    let peak = load_accum(coord.x, coord.y, base + band);
                    if peak > 0.0 {
                        let value = peak
                            * pow(alpha / (elapsed + alpha), beta);
                        if value < threshold {
                            store_accum(coord.x, coord.y, base + band, 0.0);
                        }
                    }
                }
                break;
            }
        }
    }
}
