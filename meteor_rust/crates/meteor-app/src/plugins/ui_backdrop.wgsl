#import bevy_ui::ui_vertex_output::UiVertexOutput

struct BackdropUniform {
    uv_rect: vec4<f32>,
    params: vec4<f32>,
    sweep: vec2<f32>,
};

@group(1) @binding(0) var backdrop_texture: texture_2d<f32>;
@group(1) @binding(1) var backdrop_sampler: sampler;
@group(1) @binding(2) var<uniform> backdrop: BackdropUniform;

fn sample_clamped(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(backdrop_texture, backdrop_sampler, clamp(uv, vec2<f32>(0.002), vec2<f32>(0.998))).rgb;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let t = backdrop.params.x;
    let opacity = clamp(backdrop.params.y, 0.0, 1.0);
    let blur = backdrop.params.z;
    let aspect = max(backdrop.params.w, 0.25);

    let drift = vec2<f32>(sin(t * 0.11) * 0.0035, cos(t * 0.08) * 0.0025);
    let uv = backdrop.uv_rect.xy + in.uv * backdrop.uv_rect.zw + drift;

    let r = vec2<f32>(blur / aspect, blur);
    let d = r * 0.70710678;
    var color = sample_clamped(uv) * 0.24;
    color += (
        sample_clamped(uv + vec2<f32>(r.x, 0.0)) +
        sample_clamped(uv - vec2<f32>(r.x, 0.0)) +
        sample_clamped(uv + vec2<f32>(0.0, r.y)) +
        sample_clamped(uv - vec2<f32>(0.0, r.y))
    ) * 0.11;
    color += (
        sample_clamped(uv + d) +
        sample_clamped(uv - d) +
        sample_clamped(uv + vec2<f32>(d.x, -d.y)) +
        sample_clamped(uv + vec2<f32>(-d.x, d.y))
    ) * 0.08;

    let scenic_mask = smoothstep(0.18, 0.82, in.uv.x) * (1.0 - smoothstep(0.72, 1.0, in.uv.y) * 0.55);
    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));

    let diag = in.uv.x * 0.78 + in.uv.y * 0.34;
    let distance = diag - backdrop.sweep.x;
    let distance_sq = distance * distance;
    let sweep = (0.25 * exp(-144.0 * distance_sq) + 0.12 * exp(-24.0 * distance_sq))
        * scenic_mask * backdrop.sweep.y;

    let wave_a = 0.5 + 0.5 * sin(in.uv.x * 5.4 - t * 0.32 + sin(in.uv.y * 3.8 + t * 0.19) * 0.9);
    let wave_b = 0.5 + 0.5 * sin(in.uv.x * 3.1 + in.uv.y * 4.2 + t * 0.23);
    let mist = smoothstep(0.52, 0.98, wave_a * 0.6 + wave_b * 0.4)
        * smoothstep(0.12, 0.65, in.uv.y)
        * (1.0 - smoothstep(0.68, 0.98, in.uv.y))
        * scenic_mask;

    let breathe = 0.5 + 0.5 * sin(t * 0.65 + in.uv.x * 2.4);
    let ridge_glow = smoothstep(0.18, 0.62, luma) * breathe * scenic_mask;

    let warm_gold = vec3<f32>(0.96, 0.64, 0.34);
    let cool_mist = vec3<f32>(0.48, 0.64, 0.74);

    color = mix(color, vec3<f32>(1.0, 0.77, 0.48), sweep);
    color += warm_gold * (ridge_glow * 0.045);
    color += cool_mist * (mist * 0.065);

    return vec4<f32>(color, opacity);
}
