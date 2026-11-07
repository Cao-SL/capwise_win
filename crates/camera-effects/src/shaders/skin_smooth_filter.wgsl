struct FilterUniforms {
    texel_size: vec2<f32>,
    direction: vec2<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: FilterUniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );

    var out: VertexOutput;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fs_downsample(in: VertexOutput) -> @location(0) vec4<f32> {
    let offset = uniforms.texel_size * 0.25;
    let corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    var sum = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i = i + 1u) {
        let rgb = textureSample(input_tex, input_sampler, in.uv + corners[i] * offset).rgb;
        let y = luma(rgb);
        sum += vec4<f32>(rgb, y * y);
    }
    return sum * 0.25;
}

// Averages rather than recomputes, so alpha stays the mean of luma squared.
@fragment
fn fs_reduce(in: VertexOutput) -> @location(0) vec4<f32> {
    let offset = uniforms.texel_size * 0.25;
    let corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    var sum = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i = i + 1u) {
        sum += textureSample(input_tex, input_sampler, in.uv + corners[i] * offset);
    }
    return sum * 0.25;
}

// RGB carries the local mean and alpha the local mean of luma squared, which
// together give the composite pass a per-pixel variance.
@fragment
fn fs_blur(in: VertexOutput) -> @location(0) vec4<f32> {
    let weights = array<f32, 7>(0.1964, 0.1747, 0.1227, 0.0681, 0.0299, 0.0104, 0.0028);
    let weight_sum = 1.0136;
    let step = uniforms.direction * uniforms.texel_size;
    var sum = textureSample(input_tex, input_sampler, in.uv) * weights[0];
    for (var i = 1u; i < 7u; i = i + 1u) {
        let offset = step * f32(i);
        sum += textureSample(input_tex, input_sampler, in.uv + offset) * weights[i];
        sum += textureSample(input_tex, input_sampler, in.uv - offset) * weights[i];
    }
    return sum / weight_sum;
}
