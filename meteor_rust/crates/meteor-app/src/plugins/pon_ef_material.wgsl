#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> param_slot: vec4<u32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> param_page: PonEfParamsPage;

struct PonEfMaterialParams {
    color: vec4<f32>,
    has_texture: u32,
};

struct PonEfParamsPage {
    values: array<PonEfMaterialParams, 128>,
};

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let params = param_page.values[param_slot.x];
    var texel = vec4<f32>(1.0);
    if (params.has_texture == 1u) {
        texel = textureSample(base_color_texture, base_color_sampler, in.uv);
    }
    return texel * params.color;
}
