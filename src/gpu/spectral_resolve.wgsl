// Spectral Resolve Fragment Shader
//
// Reads scalar energy from the accumulation buffer (one value per decay term),
// distributes across spectral bands using shared emission weights per group,
// integrates against CIE 1931 color matching functions, and converts to linear
// sRGB with gamut mapping.

override SPECTRAL_BANDS: u32 = 16u;

struct EmissionGroupGpu {
    weights: array<vec4<f32>, 4>,
    slow_exp_start: u32,
    slow_exp_count: u32,
    has_power_law: u32,
    power_law_layer: u32,
    elapsed_layer: u32,
    has_instant: u32,
    instant_layer: u32,
    _pad: u32,
}

struct SpectralResolveParams {
    cie_x: array<vec4<f32>, 4>,
    cie_y: array<vec4<f32>, 4>,
    cie_z: array<vec4<f32>, 4>,
    group_count: u32,
    power_law_alpha: f32,
    power_law_beta: f32,
    _pad: u32,
    groups: array<EmissionGroupGpu, 2>,
}

struct AccumDims {
    width: u32,
    height: u32,
    layers: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: SpectralResolveParams;

@group(1) @binding(0) var<storage, read> accum: array<u32>;
@group(1) @binding(1) var<uniform> accum_dims: AccumDims;

fn accum_index(x: i32, y: i32, layer: u32) -> u32 {
    return layer * (accum_dims.width * accum_dims.height) + u32(y) * accum_dims.width + u32(x);
}

fn load_accum(x: i32, y: i32, layer: u32) -> f32 {
    return bitcast<f32>(accum[accum_index(x, y, layer)]);
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(vi & 1u) * 4.0 - 1.0;
    let y = f32((vi >> 1u) & 1u) * 4.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn get_cie_weight(channel: u32, band: u32) -> f32 {
    let vec_idx = band / 4u;
    let comp_idx = band % 4u;
    switch channel {
        case 0u: { return params.cie_x[vec_idx][comp_idx]; }
        case 1u: { return params.cie_y[vec_idx][comp_idx]; }
        default: { return params.cie_z[vec_idx][comp_idx]; }
    }
}

fn get_group_weight(group_idx: u32, band: u32) -> f32 {
    let vec_idx = band / 4u;
    let comp_idx = band % 4u;
    return params.groups[group_idx].weights[vec_idx][comp_idx];
}

// Luminance-preserving desaturation for out-of-gamut colors.
fn gamut_map(rgb: vec3<f32>, luminance: f32) -> vec3<f32> {
    if luminance <= 0.0 {
        return vec3<f32>(0.0);
    }
    let min_c = min(rgb.r, min(rgb.g, rgb.b));
    if min_c >= 0.0 {
        return rgb;
    }
    let t = luminance / (luminance - min_c);
    return mix(vec3<f32>(luminance), rgb, t);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(in.position.xy);

    var X = 0.0;
    var Y = 0.0;
    var Z = 0.0;

    for (var g = 0u; g < params.group_count; g++) {
        let group = params.groups[g];

        // Sum scalar energies across all tiers for this emission group
        var group_energy = 0.0;

        // Tier 2: slow exponential terms (one scalar each)
        for (var i = 0u; i < group.slow_exp_count; i++) {
            group_energy += load_accum(coord.x, coord.y, group.slow_exp_start + i);
        }

        // Tier 3: power-law from scalar peak and elapsed time
        if group.has_power_law == 1u {
            let peak = load_accum(coord.x, coord.y, group.power_law_layer);
            if peak > 0.0 {
                let elapsed = load_accum(coord.x, coord.y, group.elapsed_layer);
                group_energy += peak * pow(
                    params.power_law_alpha / (elapsed + params.power_law_alpha),
                    params.power_law_beta);
            }
        }

        // Tier 1: instantaneous emission (one-frame scalar)
        if group.has_instant == 1u {
            group_energy += load_accum(coord.x, coord.y, group.instant_layer);
        }

        // Distribute scalar energy across spectral bands using shared emission
        // weights, then integrate against CIE color matching functions.
        for (var band = 0u; band < SPECTRAL_BANDS; band++) {
            let spectral_energy = group_energy * get_group_weight(g, band);
            X += spectral_energy * get_cie_weight(0u, band);
            Y += spectral_energy * get_cie_weight(1u, band);
            Z += spectral_energy * get_cie_weight(2u, band);
        }
    }

    // XYZ -> linear sRGB (IEC 61966-2-1)
    var rgb = vec3<f32>(
         3.2406 * X - 1.5372 * Y - 0.4986 * Z,
        -0.9689 * X + 1.8758 * Y + 0.0415 * Z,
         0.0557 * X - 0.2040 * Y + 1.0570 * Z,
    );

    // Gamut mapping for phosphor colors outside sRGB
    rgb = gamut_map(rgb, Y);

    // Output unbounded linear HDR RGB + luminance in alpha for downstream passes
    return vec4<f32>(rgb, Y);
}
