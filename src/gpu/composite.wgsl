// Composite / Display Mapping Fragment Shader
//
// Stage 2 of the display pipeline. Reads the intermediate HDR texture
// (linear sRGB from spectral resolve), applies exposure and tonemapping,
// and outputs to the swapchain surface. Bloom, glass tint, and curvature
// effects will be added here in later tasks.

// Tonemap modes (selected via params.tonemap_mode)
alias TonemapMode = u32;

const TONEMAP_REINHARD: TonemapMode = 0u;
const TONEMAP_ACES: TonemapMode = 1u;
const TONEMAP_CLAMP: TonemapMode = 2u;
const TONEMAP_NONE: TonemapMode = 3u;

struct CompositeParams {
    exposure: f32,
    tonemap_mode: TonemapMode,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> params: CompositeParams;

// HDR texture from spectral resolve pass (linear sRGB, unbounded).
// Alpha channel carries CIE Y luminance for luminance-based tonemapping.
@group(1) @binding(0) var hdr_texture: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

// Full-screen triangle: 3 vertices covering the entire clip space.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(vi & 1u) * 4.0 - 1.0;
    let y = f32((vi >> 1u) & 1u) * 4.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// Reinhard: L / (1 + L), applied to CIE Y luminance, preserving hue.
fn tonemap_reinhard(rgb: vec3<f32>, luminance: f32) -> vec3<f32> {
    if luminance <= 0.0 {
        return vec3<f32>(0.0);
    }
    let mapped = luminance / (1.0 + luminance);
    return rgb * (mapped / luminance);
}

// ACES filmic approximation (Narkowicz 2015), applied per-channel.
fn tonemap_aces(rgb: vec3<f32>) -> vec3<f32> {
    let v = rgb;
    return clamp(
        (v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

// Linear clamp — no compression, just saturate to [0, 1].
fn tonemap_clamp(rgb: vec3<f32>) -> vec3<f32> {
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn apply_tonemap(rgb: vec3<f32>, luminance: f32, mode: TonemapMode) -> vec3<f32> {
    switch mode {
        case TONEMAP_ACES: {
            return tonemap_aces(rgb);
        }
        case TONEMAP_CLAMP: {
            return tonemap_clamp(rgb);
        }
        case TONEMAP_NONE: {
            // HDR passthrough — exposure only, no compression.
            return rgb;
        }
        default: {
            return tonemap_reinhard(rgb, luminance);
        }
    }
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(in.position.xy);
    let hdr = textureLoad(hdr_texture, coord, 0);

    var rgb = hdr.rgb;
    let Y = hdr.a; // CIE Y luminance from spectral resolve

    // Exposure
    rgb *= params.exposure;
    let luminance = Y * params.exposure;

    // Tonemapping (mode selected via uniform)
    rgb = apply_tonemap(rgb, luminance, params.tonemap_mode);

    // Output linear RGB — the sRGB render target applies gamma encoding
    return vec4<f32>(rgb, 1.0);
}
