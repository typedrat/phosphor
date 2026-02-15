// Spectral Resolve Fragment Shader
//
// Stage 1 of the display pipeline. Reads the spectral accumulation texture
// array, integrates energy per band against CIE 1931 color matching functions
// to produce XYZ tristimulus values, converts to linear sRGB, and applies
// gamut mapping. Outputs unbounded linear HDR RGB to an intermediate texture.

override SPECTRAL_BANDS: u32 = 16u;

struct SpectralResolveParams {
    // CIE 1931 color matching function weights per spectral band,
    // packed as 4 vec4s per channel (4 bands per vec4).
    cie_x: array<vec4<f32>, 4>,
    cie_y: array<vec4<f32>, 4>,
    cie_z: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> params: SpectralResolveParams;

// Accumulation texture array bound as sampled texture for reading.
@group(1) @binding(0) var accum: texture_2d_array<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

// Full-screen triangle: 3 vertices covering the entire clip space.
// No vertex buffer needed — positions generated from vertex index.
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

// Luminance-preserving desaturation for out-of-gamut colors.
// Moves toward the achromatic (luminance) axis until all channels are >= 0.
// Uses CIE Y (true photopic luminance from spectral integration).
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

    // Integrate spectral energy to CIE XYZ
    var X = 0.0;
    var Y = 0.0;
    var Z = 0.0;

    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        // Sum fast + slow decay for this band
        let energy = textureLoad(accum, coord, band, 0).r
                   + textureLoad(accum, coord, SPECTRAL_BANDS + band, 0).r;

        X += energy * get_cie_weight(0u, band);
        Y += energy * get_cie_weight(1u, band);
        Z += energy * get_cie_weight(2u, band);
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
