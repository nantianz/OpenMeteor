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

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if (params.misc.x > 0.5) {
        texel = textureSample(fx_texture, fx_sampler, in.uv);
    } else {

        let r = length(in.uv * 2.0 - vec2<f32>(1.0));
        let disc = smoothstep(1.0, 0.55, r);
        texel = vec4<f32>(vec3<f32>(1.0 + 0.6 * smoothstep(0.5, 0.0, r)), disc);
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
