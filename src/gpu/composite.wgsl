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
    faceplate_scatter_intensity: f32,
    curvature: f32,
    glass_tint: vec3<f32>,
    edge_falloff: f32,
    viewport_size: vec2<f32>,
    viewport_offset: vec2<f32>,
}

@group(0) @binding(0) var<uniform> params: CompositeParams;

// HDR texture from spectral resolve pass (linear sRGB, unbounded).
// Alpha channel carries CIE Y luminance for luminance-based tonemapping.
@group(1) @binding(0) var hdr_texture: texture_2d<f32>;
@group(1) @binding(1) var hdr_sampler: sampler;

// Faceplate scatter (blurred bright areas) at half resolution.
@group(2) @binding(0) var faceplate_scatter_texture: texture_2d<f32>;
@group(2) @binding(1) var scatter_sampler: sampler;

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

// Barrel distortion: remap UV from screen center.
// k = curvature strength (0 = flat, 0.1-0.5 = typical CRT range).
fn barrel_distort(uv: vec2<f32>, k: f32) -> vec2<f32> {
    let centered = uv - vec2<f32>(0.5);
    let r2 = dot(centered, centered);
    let scale = 1.0 + k * r2;
    return centered * scale + vec2<f32>(0.5);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = in.position.xy - params.viewport_offset;
    let uv = pixel / params.viewport_size;

    // Screen curvature — remap UV through barrel distortion
    let distorted_uv = barrel_distort(uv, params.curvature);

    // Pixels outside the curved screen area render as black (bezel)
    if distorted_uv.x < 0.0 || distorted_uv.x > 1.0 || distorted_uv.y < 0.0 || distorted_uv.y > 1.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // Bilinear-filtered sampling — smooth under curvature distortion
    let hdr = textureSample(hdr_texture, hdr_sampler, distorted_uv);

    var rgb = hdr.rgb;
    let Y = hdr.a; // CIE Y luminance from spectral resolve

    // Faceplate scatter — half-res texture, hardware bilinear upscale
    let scatter = textureSample(faceplate_scatter_texture, scatter_sampler, distorted_uv).rgb;
    rgb += scatter * params.faceplate_scatter_intensity;

    // Glass faceplate tint — multiplicative color filter
    rgb *= params.glass_tint;

    // Edge darkening — cosine falloff from screen normal
    let centered = distorted_uv - vec2<f32>(0.5);
    let r2 = dot(centered, centered);
    let edge_dim = mix(1.0, 1.0 - r2 * 4.0, params.edge_falloff);
    rgb *= max(edge_dim, 0.0);

    // Exposure
    rgb *= params.exposure;
    let luminance = Y * params.exposure;

    // Tonemapping (mode selected via uniform)
    rgb = apply_tonemap(rgb, luminance, params.tonemap_mode);

    // Output linear RGB — the sRGB render target applies gamma encoding
    return vec4<f32>(rgb, 1.0);
}
