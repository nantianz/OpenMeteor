#import bevy_pbr::forward_io::VertexOutput

struct TrailParams {

    misc: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: TrailParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var trail_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var trail_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let age = clamp(in.uv.x, 0.0, 1.0);
    let span = clamp(in.uv.y, 0.0, 1.0);
    let softness = max(in.uv_b.x, 0.001);
    let core_strength = in.uv_b.y;

    let edge = smoothstep(0.0, softness * 1.5, span) * smoothstep(0.0, softness, 1.0 - span);

    let core_pos = 1.0 - softness * 1.2;
    let core_width = 0.05 + softness * 0.15;
    let core = core_strength * 2.5 * exp(-pow((span - core_pos) / core_width, 2.0)) * (1.0 - age);

    var texel = vec4<f32>(1.0);
    if (params.misc.y > 0.5) {
        texel = textureSample(trail_texture, trail_sampler, vec2<f32>(age + params.misc.x, span));
    }

    let envelope = in.color.a * 2.0;
    let rgb = in.color.rgb * texel.rgb * (1.0 + core) * edge * texel.a * envelope;
    return vec4<f32>(rgb, 1.0);
}
