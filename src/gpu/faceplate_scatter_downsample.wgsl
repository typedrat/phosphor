// Faceplate Scatter Downsample Shader
//
// 2x downsample of the HDR texture with brightness threshold.
// Hardware bilinear sampling at 2x2 block centers gives a free box filter.

struct DownsampleParams {
    threshold: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> params: DownsampleParams;
@group(1) @binding(0) var hdr_texture: texture_2d<f32>;
@group(1) @binding(1) var hdr_sampler: sampler;

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
    // Each output texel corresponds to a 2x2 block in the source.
    // Sample at block center — bilinear filter averages the 4 texels.
    let src_size = vec2<f32>(textureDimensions(hdr_texture));
    let uv = in.position.xy / src_size * 2.0;
    let avg = textureSample(hdr_texture, hdr_sampler, uv);

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
