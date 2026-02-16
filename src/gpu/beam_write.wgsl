// Beam Write Compute Shader
//
// For each BeamSample, deposits energy into the spectral accumulation buffer
// by analytically integrating the Gaussian beam profile along the line segment
// from the previous sample to the current one. This produces smooth continuous
// traces even at coarse sample rates, avoiding the "beaded necklace" artifact
// of per-point splatting.
//
// Falls back to a point splat when the segment is shorter than half a pixel
// (first sample in a frame, or after a blanked retrace).
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
    sample_count: u32,
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

struct EmissionParams {
    weights: array<vec4<f32>, 4>,
    slow_exp_count: u32,
    has_power_law: u32,
    instant_energy_total: f32,
    has_instant: u32,
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

// --- Profile functions ---

// Abramowitz & Stegun 7.1.26, |ε| ≤ 2.5×10⁻⁵
fn erf_approx(x: f32) -> f32 {
    let ax = abs(x);
    let t = 1.0 / (1.0 + 0.47047 * ax);
    let t2 = t * t;
    let t3 = t2 * t;
    let y = 1.0 - (0.3480242 * t - 0.0958798 * t2 + 0.7478556 * t3) * exp(-ax * ax);
    return select(-y, y, x >= 0.0);
}

const SQRT_2: f32 = 1.4142136;
const SQRT_HALF_PI: f32 = 1.2533141; // √(π/2)

// Analytical integral of a 1D Gaussian with width σ over [0, seg_len],
// evaluated at parallel offset t_par, weighted by perpendicular Gaussian.
// Returns the average profile intensity along the segment at this pixel.
fn line_gaussian(d_perp_sq: f32, t_par: f32, seg_len: f32, sigma: f32) -> f32 {
    let inv_sqrt2_sigma = 1.0 / (SQRT_2 * sigma);
    let erf_a = erf_approx((seg_len - t_par) * inv_sqrt2_sigma);
    let erf_b = erf_approx(t_par * inv_sqrt2_sigma);
    return (sigma / seg_len) * SQRT_HALF_PI
         * exp(-d_perp_sq / (2.0 * sigma * sigma))
         * (erf_a + erf_b);
}

// Line-integrated core + halo profile. Converges to spot_profile as seg_len → 0.
fn line_profile(d_perp_sq: f32, t_par: f32, seg_len: f32) -> f32 {
    let h = params.halo_fraction;
    let core = line_gaussian(d_perp_sq, t_par, seg_len, params.sigma_core);
    let halo = line_gaussian(d_perp_sq, t_par, seg_len, params.sigma_halo);
    return (1.0 - h) * core + h * halo;
}

// Point-splat Gaussian core + halo at distance² r_sq.
fn spot_profile(r_sq: f32) -> f32 {
    let h = params.halo_fraction;
    let inv_2_sigma_core_sq = 0.5 / (params.sigma_core * params.sigma_core);
    let inv_2_sigma_halo_sq = 0.5 / (params.sigma_halo * params.sigma_halo);
    return (1.0 - h) * exp(-r_sq * inv_2_sigma_core_sq)
         + h * exp(-r_sq * inv_2_sigma_halo_sq);
}

// --- Main ---

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

    // Current beam position in pixels
    let bx = sample.x * f32(params.width);
    let by = sample.y * f32(params.height);

    // Previous beam position — forms a line segment for integration.
    // Falls back to current position (point splat) for the first sample
    // or after a blanked retrace.
    var ax = bx;
    var ay = by;
    if sample_idx > 0u {
        let prev = samples[sample_idx - 1u];
        if prev.intensity > 0.0 {
            ax = prev.x * f32(params.width);
            ay = prev.y * f32(params.height);
        }
    }

    // Segment geometry
    let seg_dx = bx - ax;
    let seg_dy = by - ay;
    let seg_len = sqrt(seg_dx * seg_dx + seg_dy * seg_dy);
    let use_line = seg_len > 0.5;

    // Precompute unit direction for line mode
    var dir_x = 0.0;
    var dir_y = 0.0;
    if use_line {
        let inv_len = 1.0 / seg_len;
        dir_x = seg_dx * inv_len;
        dir_y = seg_dy * inv_len;
    }

    // Gaussian footprint radius (4σ of the larger component)
    let sigma_max = max(params.sigma_core, params.sigma_halo);
    let radius = ceil(4.0 * sigma_max);
    let radius_sq = radius * radius;

    // Bounding box: segment AABB expanded by gaussian radius.
    // For point splats (seg_len ≈ 0), this reduces to a square around the beam.
    let center_x = (ax + bx) * 0.5;
    let center_y = (ay + by) * 0.5;
    let extent_x = i32(ceil(abs(seg_dx) * 0.5 + radius));
    let extent_y = i32(ceil(abs(seg_dy) * 0.5 + radius));

    let tile_size = 16;
    let steps_x = i32(ceil(f32(2 * extent_x + 1) / f32(tile_size)));
    let steps_y = i32(ceil(f32(2 * extent_y + 1) / f32(tile_size)));

    for (var ty = 0; ty < steps_y; ty++) {
        for (var tx = 0; tx < steps_x; tx++) {
            let px_x = i32(center_x) + i32(local_id.x) - extent_x + tx * tile_size;
            let px_y = i32(center_y) + i32(local_id.y) - extent_y + ty * tile_size;

            if px_x < 0 || px_x >= i32(params.width) || px_y < 0 || px_y >= i32(params.height) {
                continue;
            }

            let px_cx = f32(px_x) + 0.5;
            let px_cy = f32(px_y) + 0.5;

            var profile_val: f32;

            if use_line {
                // Vector from segment start (A) to pixel center
                let vx = px_cx - ax;
                let vy = px_cy - ay;

                // Parallel projection along segment direction
                let t_par = vx * dir_x + vy * dir_y;

                // Perpendicular distance squared
                let perp_x = vx - t_par * dir_x;
                let perp_y = vy - t_par * dir_y;
                let d_perp_sq = perp_x * perp_x + perp_y * perp_y;

                // Early-out: distance from pixel to nearest point on segment
                let ct = clamp(t_par, 0.0, seg_len);
                let near_x = px_cx - (ax + ct * dir_x);
                let near_y = px_cy - (ay + ct * dir_y);
                if near_x * near_x + near_y * near_y > radius_sq {
                    continue;
                }

                profile_val = line_profile(d_perp_sq, t_par, seg_len);
            } else {
                // Point splat fallback
                let dx = px_cx - bx;
                let dy = px_cy - by;
                let r_sq = dx * dx + dy * dy;
                if r_sq > radius_sq {
                    continue;
                }
                profile_val = spot_profile(r_sq);
            }

            let base_energy = sample.intensity * profile_val * sample.dt;

            // Tier 2: deposit into slow exponential layers
            for (var term = 0u; term < emission.slow_exp_count; term++) {
                for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                    let energy = base_energy * get_emission_weight(band);
                    let layer = term * SPECTRAL_BANDS + band;
                    atomic_add_f32(accum_index(px_x, px_y, layer), energy);
                }
            }

            // Tier 3: deposit peak energy into power-law layers, reset elapsed time
            if emission.has_power_law == 1u {
                let pl_base = emission.slow_exp_count * SPECTRAL_BANDS;
                for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                    let energy = base_energy * get_emission_weight(band);
                    atomic_add_f32(accum_index(px_x, px_y, pl_base + band), energy);
                }
                // Reset elapsed time to 0 for this texel
                let time_layer = pl_base + SPECTRAL_BANDS;
                accum[accum_index(px_x, px_y, time_layer)] = bitcast<u32>(0.0);
            }

            // Tier 1: deposit instantaneous spectral emission (one-frame layers).
            // Energy = base × ∑(A·τ) for fast exponentials — the analytically
            // integrated total output of sub-frame decay channels.
            if emission.has_instant == 1u {
                let inst_base = emission.slow_exp_count * SPECTRAL_BANDS
                    + select(0u, SPECTRAL_BANDS + 1u, emission.has_power_law == 1u);
                let inst_energy = base_energy * emission.instant_energy_total;
                for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                    let energy = inst_energy * get_emission_weight(band);
                    atomic_add_f32(accum_index(px_x, px_y, inst_base + band), energy);
                }
            }
        }
    }
}
