#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_tag, get_world_from_local, mesh_position_local_to_world},
    view_transformations::{position_world_to_clip, depth_ndc_to_view_z},
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif

struct ParticleParams {
    color: vec4<f32>,
    uv_offset: vec2<f32>,
    has_texture: u32,
    soft: f32,
};
struct ParticlePage {
    values: array<ParticleParams, 128>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> page: ParticlePage;

struct ParticleOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) has_texture: u32,
    @location(3) @interpolate(flat) soft: f32,
};

@vertex
fn vertex(in: Vertex) -> ParticleOutput {
    let params = page.values[get_tag(in.instance_index)];
    let world = mesh_position_local_to_world(get_world_from_local(in.instance_index), vec4<f32>(in.position, 1.0));
    var out: ParticleOutput;
    out.position = position_world_to_clip(world.xyz);
    out.uv = in.uv;
    out.color = params.color;
    out.has_texture = params.has_texture;
    out.soft = params.soft;
    return out;
}

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
fn fragment(in: ParticleOutput) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if ((in.has_texture & 1u) == 1u) {
        texel = textureSample(base_color_texture, base_color_sampler, in.uv);
    }
    let soft = soft_factor(in.position, in.soft);
    var fade = vec4<f32>(soft, soft, soft, 1.0);
    if ((in.has_texture & 2u) == 2u) {
        fade = vec4<f32>(1.0, 1.0, 1.0, soft);
    }
    return texel * in.color * fade;
}
