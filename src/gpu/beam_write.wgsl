// Beam Write Compute Shader
//
// For each BeamSample, splats a Gaussian spot profile into the spectral
// accumulation textures. One workgroup per sample, threads cooperatively
// cover the spot footprint tile.

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
// emission[i].x = emission weight for band i
// emission[i].y = fast decay amplitude fraction (a_fast)
struct EmissionParams {
    weights: array<vec4<f32>, 4>,  // 16 bands packed as 4 vec4s
    fast_fraction: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<storage, read> samples: array<BeamSample>;
@group(0) @binding(1) var<uniform> params: BeamParams;
@group(0) @binding(2) var<uniform> emission: EmissionParams;

// Accumulation textures: 4 per component, 2 components (fast/slow) per layer.
// For single-layer: textures 0-3 = fast decay, 4-7 = slow decay.
@group(1) @binding(0) var accum_0: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(1) var accum_1: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(2) var accum_2: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(3) var accum_3: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(4) var accum_4: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(5) var accum_5: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(6) var accum_6: texture_storage_2d<rgba32float, read_write>;
@group(1) @binding(7) var accum_7: texture_storage_2d<rgba32float, read_write>;

fn get_emission_weight(band: u32) -> f32 {
    let vec_idx = band / 4u;
    let comp_idx = band % 4u;
    return emission.weights[vec_idx][comp_idx];
}

// Evaluate Gaussian core + halo spot profile at distance r
fn spot_profile(r_sq: f32) -> f32 {
    let h = params.halo_fraction;
    let inv_2_sigma_core_sq = 0.5 / (params.sigma_core * params.sigma_core);
    let inv_2_sigma_halo_sq = 0.5 / (params.sigma_halo * params.sigma_halo);
    return (1.0 - h) * exp(-r_sq * inv_2_sigma_core_sq)
         + h * exp(-r_sq * inv_2_sigma_halo_sq);
}

// Deposit energy into a single accumulation texture at the given texel.
fn deposit_energy(tex_idx: u32, coord: vec2<i32>, energy: vec4<f32>) {
    switch tex_idx {
        case 0u: {
            let prev = textureLoad(accum_0, coord);
            textureStore(accum_0, coord, prev + energy);
        }
        case 1u: {
            let prev = textureLoad(accum_1, coord);
            textureStore(accum_1, coord, prev + energy);
        }
        case 2u: {
            let prev = textureLoad(accum_2, coord);
            textureStore(accum_2, coord, prev + energy);
        }
        case 3u: {
            let prev = textureLoad(accum_3, coord);
            textureStore(accum_3, coord, prev + energy);
        }
        case 4u: {
            let prev = textureLoad(accum_4, coord);
            textureStore(accum_4, coord, prev + energy);
        }
        case 5u: {
            let prev = textureLoad(accum_5, coord);
            textureStore(accum_5, coord, prev + energy);
        }
        case 6u: {
            let prev = textureLoad(accum_6, coord);
            textureStore(accum_6, coord, prev + energy);
        }
        case 7u: {
            let prev = textureLoad(accum_7, coord);
            textureStore(accum_7, coord, prev + energy);
        }
        default: {}
    }
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

            let coord = vec2<i32>(px_x, px_y);
            let a_fast = emission.fast_fraction;
            let a_slow = 1.0 - a_fast;

            // Deposit into each spectral band texture (4 bands per texture)
            for (var tex = 0u; tex < 4u; tex++) {
                let b0 = tex * 4u;
                let e = vec4<f32>(
                    base_energy * get_emission_weight(b0),
                    base_energy * get_emission_weight(b0 + 1u),
                    base_energy * get_emission_weight(b0 + 2u),
                    base_energy * get_emission_weight(b0 + 3u),
                );

                // Fast decay textures: indices 0..3
                deposit_energy(tex, coord, e * a_fast);
                // Slow decay textures: indices 4..7
                deposit_energy(tex + 4u, coord, e * a_slow);
            }
        }
    }
}
