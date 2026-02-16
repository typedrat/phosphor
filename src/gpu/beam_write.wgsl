// Beam Write Compute Shader
//
// For each BeamSample, splats a Gaussian spot profile into the spectral
// accumulation buffer. One workgroup per sample, threads cooperatively
// cover the spot footprint tile.
//
// Uses atomic CAS-loop float addition to correctly accumulate overlapping
// spots that write to the same pixel from concurrent workgroups.

override SPECTRAL_BANDS: u32 = 16u;

struct BeamSample {
    x: f32,
    y: f32,
    intensity: f32,
    dt: f32,
}

struct BeamParams {
    sigma_core: f32,
    sigma_halo: f32,
    halo_fraction: f32,
    // Number of samples this frame
    sample_count: u32,
    // Accumulation buffer dimensions
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

// Per-band emission weight and fast/slow amplitude split.
struct EmissionParams {
    weights: array<vec4<f32>, 4>,  // 16 bands packed as 4 vec4s
    fast_fraction: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

struct AccumDims {
    width: u32,
    height: u32,
    layers: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> samples: array<BeamSample>;
@group(0) @binding(1) var<uniform> params: BeamParams;
@group(0) @binding(2) var<uniform> emission: EmissionParams;

@group(1) @binding(0) var<storage, read_write> accum: array<atomic<u32>>;
@group(1) @binding(1) var<uniform> accum_dims: AccumDims;

fn get_emission_weight(band: u32) -> f32 {
    let vec_idx = band / 4u;
    let comp_idx = band % 4u;
    return emission.weights[vec_idx][comp_idx];
}

fn accum_index(x: i32, y: i32, layer: u32) -> u32 {
    return layer * (accum_dims.width * accum_dims.height) + u32(y) * accum_dims.width + u32(x);
}

fn atomic_add_f32(idx: u32, delta: f32) {
    if delta == 0.0 { return; }
    loop {
        let old = atomicLoad(&accum[idx]);
        let new_val = bitcast<u32>(bitcast<f32>(old) + delta);
        let result = atomicCompareExchangeWeak(&accum[idx], old, new_val);
        if result.exchanged { break; }
    }
}

// Evaluate Gaussian core + halo spot profile at distance r
fn spot_profile(r_sq: f32) -> f32 {
    let h = params.halo_fraction;
    let inv_2_sigma_core_sq = 0.5 / (params.sigma_core * params.sigma_core);
    let inv_2_sigma_halo_sq = 0.5 / (params.sigma_halo * params.sigma_halo);
    return (1.0 - h) * exp(-r_sq * inv_2_sigma_core_sq)
         + h * exp(-r_sq * inv_2_sigma_halo_sq);
}

// Each workgroup handles one beam sample. Threads tile the spot footprint.
@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
) {
    let sample_idx = wg_id.x;
    if sample_idx >= params.sample_count {
        return;
    }

    let sample = samples[sample_idx];
    if sample.intensity <= 0.0 {
        return;
    }

    // Beam position in pixel coordinates
    let beam_x = sample.x * f32(params.width);
    let beam_y = sample.y * f32(params.height);

    // 4-sigma radius for the spot footprint (use the larger of core/halo)
    let sigma_max = max(params.sigma_core, params.sigma_halo);
    let radius = ceil(4.0 * sigma_max);
    let radius_i = i32(radius);

    // This thread's offset within the spot tile
    let tile_size = 16;
    let ox = i32(local_id.x) - radius_i;
    let oy = i32(local_id.y) - radius_i;

    // If the spot is larger than 16x16, we need to loop over tiles
    let steps = i32(ceil(f32(2 * radius_i + 1) / f32(tile_size)));

    let a_fast = emission.fast_fraction;
    let a_slow = 1.0 - a_fast;

    for (var ty = 0; ty < steps; ty++) {
        for (var tx = 0; tx < steps; tx++) {
            let px_offset_x = ox + tx * tile_size;
            let px_offset_y = oy + ty * tile_size;

            let px_x = i32(beam_x) + px_offset_x;
            let px_y = i32(beam_y) + px_offset_y;

            // Bounds check
            if px_x < 0 || px_x >= i32(params.width) || px_y < 0 || px_y >= i32(params.height) {
                continue;
            }

            // Distance squared from beam center (in pixels)
            let dx = f32(px_x) + 0.5 - beam_x;
            let dy = f32(px_y) + 0.5 - beam_y;
            let r_sq = dx * dx + dy * dy;

            // Skip pixels outside 4-sigma
            if r_sq > radius * radius {
                continue;
            }

            let profile = spot_profile(r_sq);
            let base_energy = sample.intensity * profile * sample.dt;

            // Deposit into each spectral band layer
            for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                let energy = base_energy * get_emission_weight(band);

                // Fast decay layer
                atomic_add_f32(accum_index(px_x, px_y, band), energy * a_fast);

                // Slow decay layer
                atomic_add_f32(accum_index(px_x, px_y, SPECTRAL_BANDS + band), energy * a_slow);
            }
        }
    }
}
