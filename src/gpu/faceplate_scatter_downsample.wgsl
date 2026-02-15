// Faceplate Scatter Downsample Shader
//
// 2x downsample of the HDR texture with brightness threshold.
// Uses textureLoad (Rgba32Float is not filterable without a feature flag).

struct DownsampleParams {
    threshold: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> params: DownsampleParams;
@group(1) @binding(0) var hdr_texture: texture_2d<f32>;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Each output texel corresponds to a 2x2 block in the source
    let dst_coord = vec2<i32>(in.position.xy);
    let src_base = dst_coord * 2;

    // Manual 2x2 average
    let s00 = textureLoad(hdr_texture, src_base + vec2<i32>(0, 0), 0);
    let s10 = textureLoad(hdr_texture, src_base + vec2<i32>(1, 0), 0);
    let s01 = textureLoad(hdr_texture, src_base + vec2<i32>(0, 1), 0);
    let s11 = textureLoad(hdr_texture, src_base + vec2<i32>(1, 1), 0);
    let avg = (s00 + s10 + s01 + s11) * 0.25;

    let rgb = avg.rgb;
    let luminance = avg.a; // CIE Y from spectral resolve

    // Soft threshold: extract only the bright portion
    if luminance <= 0.0 {
        return vec4<f32>(0.0);
    }
    let contribution = max(luminance - params.threshold, 0.0);
    let scale = contribution / luminance;

    return vec4<f32>(rgb * scale, 0.0);
}
