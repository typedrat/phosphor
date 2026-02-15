// Faceplate Scatter Gaussian Blur Shader
//
// Separable Gaussian blur. Run twice: once with direction=(1,0) for horizontal,
// once with direction=(0,1) for vertical. Kernel width derived from sigma.

struct BlurParams {
    direction: vec2<f32>,
    sigma: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> params: BlurParams;
@group(1) @binding(0) var src_texture: texture_2d<f32>;

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
    let coord = vec2<i32>(in.position.xy);
    let tex_size = vec2<i32>(textureDimensions(src_texture));

    if params.sigma <= 0.0 {
        return textureLoad(src_texture, coord, 0);
    }

    let half_width = i32(ceil(3.0 * params.sigma));
    let inv_2sigma2 = -0.5 / (params.sigma * params.sigma);
    let dir = vec2<i32>(params.direction);

    var color = vec4<f32>(0.0);
    var weight_sum = 0.0;

    for (var i = -half_width; i <= half_width; i++) {
        let sample_coord = clamp(
            coord + dir * i,
            vec2<i32>(0),
            tex_size - vec2<i32>(1),
        );

        let w = exp(f32(i * i) * inv_2sigma2);
        color += textureLoad(src_texture, sample_coord, 0) * w;
        weight_sum += w;
    }

    return color / weight_sum;
}
