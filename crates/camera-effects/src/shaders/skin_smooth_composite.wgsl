struct CompositeUniforms {
    amount: f32,
    edge_sigma: f32,
    texture_keep: f32,
    contour_threshold: f32,
    mid_sigma: f32,
    brighten: f32,
    yellow_cut: f32,
    red_cut: f32,
};

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var base_tex: texture_2d<f32>;
@group(0) @binding(2) var mid_tex: texture_2d<f32>;
@group(0) @binding(3) var person_tex: texture_2d<f32>;
@group(0) @binding(4) var input_sampler: sampler;
@group(0) @binding(5) var<uniform> uniforms: CompositeUniforms;

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

// Read from the low resolution mean, not the source, so sensor chroma noise
// cannot make the mask shimmer from frame to frame.
fn skin_mask(mean: vec3<f32>, mean_y: f32) -> f32 {
    let cb = (mean.b - mean_y) * 0.564 + 0.5;
    let cr = (mean.r - mean_y) * 0.713 + 0.5;
    let distance = length(vec2<f32>((cb - 0.43) / 0.10, (cr - 0.60) / 0.095));
    let chroma = 1.0 - smoothstep(0.8, 1.3, distance);
    return chroma * smoothstep(0.08, 0.2, mean_y);
}

// Raising luma alone leaves skin grey, so chroma scales with it. Deep tones get
// a smaller lift to keep them from going chalky, and the lift stops at the
// highlight headroom so a sunlit forehead keeps its detail.
fn brightened(rgb: vec3<f32>) -> vec3<f32> {
    let y = luma(rgb);
    let lift = uniforms.brighten * (0.35 + 0.65 * smoothstep(0.1, 0.4, y));
    let lifted_y = y + lift * 4.0 * y * (1.0 - y);
    let chroma_gain = sqrt(max(lifted_y, 0.001) / max(y, 0.001));
    let r = lifted_y + (rgb.r - y) * chroma_gain * (1.0 - uniforms.red_cut);
    let b = lifted_y + (rgb.b - y) * chroma_gain * (1.0 - uniforms.yellow_cut);
    let g = (lifted_y - 0.299 * r - 0.114 * b) / 0.587;
    let lifted = vec3<f32>(r, g, b);
    let peak = max(rgb.r, max(rgb.g, rgb.b));
    let lifted_peak = max(r, max(g, b));
    let headroom = clamp((0.97 - peak) / max(lifted_peak - peak, 0.001), 0.0, 1.0);
    return mix(rgb, lifted, headroom);
}

@fragment
fn fs_composite(in: VertexOutput) -> @location(0) vec4<f32> {
    let source = textureSample(source_tex, input_sampler, in.uv);
    let base = textureSample(base_tex, input_sampler, in.uv);
    let mean = base.rgb;
    let mean_y = luma(mean);

    // Guided filter coefficient: near 1 on eyes, brows and jawlines whose
    // local variance dwarfs the sigma, near 0 across skin texture.
    let variance = max(base.a - mean_y * mean_y, 0.0);
    let sigma_squared = uniforms.edge_sigma * uniforms.edge_sigma;
    var coefficient = variance / (variance + sigma_squared);

    // Pores and spots vanish from the low resolution mean while jawlines, lip
    // lines and stray hairs survive it, so its gradient protects the long, low
    // contrast contours that variance alone would blur away.
    let texel = 2.0 / vec2<f32>(textureDimensions(base_tex));
    let gradient_x = textureSample(base_tex, input_sampler, in.uv + vec2<f32>(texel.x, 0.0)).rgb
        - textureSample(base_tex, input_sampler, in.uv - vec2<f32>(texel.x, 0.0)).rgb;
    let gradient_y = textureSample(base_tex, input_sampler, in.uv + vec2<f32>(0.0, texel.y)).rgb
        - textureSample(base_tex, input_sampler, in.uv - vec2<f32>(0.0, texel.y)).rgb;
    let contour = smoothstep(
        uniforms.contour_threshold,
        uniforms.contour_threshold * 3.0,
        length(gradient_x) + length(gradient_y),
    );
    coefficient = max(coefficient, contour);

    let darkness = smoothstep(0.0, 0.05, mean_y - luma(source.rgb));
    coefficient *= 1.0 - 0.5 * (1.0 - coefficient) * darkness;

    // A floor on the coefficient keeps some pores, which is what separates
    // retouched skin from the plastic look of a plain blur.
    coefficient = uniforms.texture_keep + (1.0 - uniforms.texture_keep) * coefficient;

    // Blotches and creases are wider than the fine window and survive H.264 and
    // the camera's own denoise, which already took the pores. Only this wider
    // level visibly changes such footage, and it still shows in a small bubble.
    let wide = textureSample(mid_tex, input_sampler, in.uv);
    let wide_y = luma(wide.rgb);
    let wide_variance = max(wide.a - wide_y * wide_y, 0.0);
    let wide_sigma = uniforms.mid_sigma * 2.5;
    let mid_sigma_squared = wide_sigma * wide_sigma;
    // A local mean far from the wide mean has an edge in its window. Levelling
    // it would paint a halo around noses, lips and the jaw, so it is left alone.
    let structure = smoothstep(
        uniforms.mid_sigma,
        uniforms.mid_sigma * 3.0,
        length(mean - wide.rgb),
    );
    let wide_coefficient = max(
        max(wide_variance / (wide_variance + mid_sigma_squared), contour),
        structure,
    );

    let levelled = wide.rgb + wide_coefficient * (mean - wide.rgb);
    let mask = skin_mask(mean, mean_y);
    // Cream walls pass the fine mask, which is harmless there, but levelling them
    // draws a visible contour where the jaw guard ends.
    let wide_cr = (mean.r - mean_y) * 0.713 + 0.5;
    let wide_mask = smoothstep(0.535, 0.575, wide_cr);
    let levelled_masked = mix(mean, levelled, wide_mask);
    let smoothed = levelled_masked + coefficient * (source.rgb - mean);
    let retouched = mix(source.rgb, smoothed, uniforms.amount * mask);

    // Strongly lit skin leaves the fine mask's ellipse, which would leave an
    // untouched patch here. The person mask already excludes the room, so any
    // warm hue on the person counts.
    let cb = (mean.b - mean_y) * 0.564 + 0.5;
    let warm = smoothstep(0.50, 0.53, wide_cr)
        * (1.0 - smoothstep(0.495, 0.515, cb))
        * smoothstep(0.08, 0.2, mean_y);
    let person = textureSample(person_tex, input_sampler, in.uv).r;
    return vec4<f32>(mix(retouched, brightened(retouched), person * warm), source.a);
}
