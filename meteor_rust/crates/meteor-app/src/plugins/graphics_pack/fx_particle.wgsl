#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::view_transformations::depth_ndc_to_view_z
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif

struct FxParams {

    misc: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: FxParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var fx_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var fx_sampler: sampler;

fn plume_hash(p: vec2<f32>) -> f32 {
    let q = fract(vec3<f32>(p.xyx) * vec3<f32>(0.1031, 0.1030, 0.0973));
    let r = q + dot(q, q.yzx + vec3<f32>(33.33));
    return fract((r.x + r.y) * r.z);
}
fn plume_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(plume_hash(i), plume_hash(i + vec2<f32>(1.0, 0.0)), s.x),
        mix(plume_hash(i + vec2<f32>(0.0, 1.0)), plume_hash(i + vec2<f32>(1.0, 1.0)), s.x),
        s.y,
    );
}
fn flame_shape(p: vec2<f32>, phase: f32) -> vec4<f32> {
    let y = 1.0 - p.y;
    let noise = plume_noise(vec2<f32>(p.x * 4.0 + phase, y * 4.5 - phase * 1.7));
    let sway = (sin(phase * 3.1 + y * 8.0) * 0.19 + (noise - 0.5) * 0.42) * y;
    let width = mix(0.92, 0.16, smoothstep(0.18, 0.97, y));
    let distance = abs((p.x * 2.0 - 1.0) - sway) / width;
    let tongue = (1.0 - smoothstep(0.21, 0.99, distance))
        * smoothstep(0.0, 0.23, y) * (1.0 - smoothstep(0.68, 0.99, y));
    let core = (1.0 - smoothstep(0.0, 0.65, distance)) * (1.0 - smoothstep(0.19, 0.74, y));
    let color = mix(vec3<f32>(1.0, 0.62, 0.30), vec3<f32>(2.15, 2.05, 1.62), core);
    return vec4<f32>(color, tongue * (0.59 + 0.41 * noise));
}
fn smoke_shape(p: vec2<f32>, phase: f32) -> vec4<f32> {
    let flow = vec2<f32>(phase, -phase * 0.81);
    let coarse = plume_noise(p * 4.0 + flow);
    let fine = plume_noise(p * 9.0 - flow * 1.6);
    let cloud = smoothstep(0.22, 0.83, coarse * 0.76 + fine * 0.24);
    let edge = 1.0 - smoothstep(0.30, 0.99, length(p * 2.0 - 1.0));
    return vec4<f32>(vec3<f32>(0.7 + 0.54 * coarse), cloud * edge);
}
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if (params.misc.x > 0.5) {
        texel = textureSample(fx_texture, fx_sampler, in.uv);
    } else if (params.misc.z > 1.5) {
        texel = smoke_shape(in.uv, in.uv_b.y);
    } else if (params.misc.z > 0.5) {
        texel = flame_shape(in.uv, in.uv_b.y);
    } else {

        let r = length(in.uv * 2.0 - vec2<f32>(1.0));
        let disc = 1.0 - smoothstep(0.55, 1.0, r);
        texel = vec4<f32>(vec3<f32>(1.0 + 0.6 * (1.0 - smoothstep(0.0, 0.5, r))), disc);
    }

    var soft = 1.0;
#ifdef DEPTH_PREPASS
    let soft_distance = in.uv_b.x;
    if (soft_distance > 0.0) {
        let scene_z = depth_ndc_to_view_z(prepass_depth(in.position, 0u));
        let frag_z = depth_ndc_to_view_z(in.position.z);

        soft = clamp((frag_z - scene_z) / soft_distance, 0.0, 1.0);
    }
#endif

    let alpha = texel.a * in.color.a * soft;
    let rgb = texel.rgb * in.color.rgb;
    if (params.misc.y > 0.5) {

        return vec4<f32>(rgb * alpha, 0.0);
    }
    return vec4<f32>(rgb, alpha);
}
