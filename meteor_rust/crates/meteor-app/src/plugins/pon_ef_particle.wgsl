

#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_tag, get_world_from_local, mesh_position_local_to_world},
    view_transformations::position_world_to_clip,
}

struct ParticleParams {
    color: vec4<f32>,
    has_texture: u32,
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
    return out;
}

@fragment
fn fragment(in: ParticleOutput) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if (in.has_texture == 1u) {
        texel = textureSample(base_color_texture, base_color_sampler, in.uv);
    }
    return texel * in.color;
}
