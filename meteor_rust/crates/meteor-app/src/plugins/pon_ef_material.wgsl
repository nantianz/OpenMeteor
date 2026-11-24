#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::view_transformations::depth_ndc_to_view_z
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> param_slot: vec4<u32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> param_page: PonEfParamsPage;

struct PonEfMaterialParams {
    color: vec4<f32>,
    uv_offset: vec2<f32>,

    has_texture: u32,

    soft: f32,
};

struct PonEfParamsPage {
    values: array<PonEfMaterialParams, 128>,
};

fn soft_factor(frag: vec4<f32>, soft: f32) -> f32 {
#ifdef DEPTH_PREPASS
    if (soft > 0.0) {
        let scene_z = depth_ndc_to_view_z(prepass_depth(frag, 0u));
        let frag_z = depth_ndc_to_view_z(frag.z);
        return clamp((frag_z - scene_z) / soft, 0.0, 1.0);
    }
#endif
    return 1.0;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let params = param_page.values[param_slot.x];
    var texel = vec4<f32>(1.0);
    if ((params.has_texture & 1u) == 1u) {
        texel = textureSample(base_color_texture, base_color_sampler, in.uv + params.uv_offset);
    }
    let soft = soft_factor(in.position, params.soft);
    var fade = vec4<f32>(soft, soft, soft, 1.0);
    if ((params.has_texture & 2u) == 2u) {
        fade = vec4<f32>(1.0, 1.0, 1.0, soft);
    }
    return texel * params.color * fade;
}
