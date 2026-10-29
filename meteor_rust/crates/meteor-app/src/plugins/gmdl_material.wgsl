

#import bevy_pbr::forward_io::VertexOutput;
#import bevy_pbr::mesh_view_bindings::view;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: GmdlGpuParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> occlusion: CameraOcclusionParams;

@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> simulation_time: vec4<f32>;

struct GmdlGpuParams {
    base: GmdlMaterialParams,
    scroll: vec4<f32>,
};

struct CameraOcclusionParams {
    eye: vec4<f32>,
    player: vec4<f32>,
    watched: vec4<f32>,
};

fn visibility_tunnel(world: vec3<f32>, body: vec4<f32>) -> f32 {
    let axis = body.xyz - occlusion.eye.xyz;
    let axis_length = length(axis);
    if (axis_length < 0.01 || body.w <= 0.0) { return 0.0; }
    let direction = axis / axis_length;
    let depth = dot(world - occlusion.eye.xyz, direction);
    if (depth <= 0.0 || depth >= axis_length - 0.25) { return 0.0; }
    let radius = mix(0.25, body.w, clamp(depth / axis_length, 0.0, 1.0));
    let radial = length(world - occlusion.eye.xyz - direction * depth);
    return (1.0 - smoothstep(radius * 0.65, radius, radial))
        * smoothstep(0.25, 0.65, axis_length - depth);
}

fn camera_fade(world: vec3<f32>, pixel: vec2<f32>) -> bool {
    let amount = occlusion.eye.w * max(visibility_tunnel(world, occlusion.player), visibility_tunnel(world, occlusion.watched));

    let bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0,
                              3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let xy = vec2<u32>(pixel) % vec2<u32>(4u);
    return amount > (bayer[xy.y * 4u + xy.x] + 0.5) / 16.0;
}

struct GmdlMaterialParams {
    opacity: f32,
    alpha_cutoff: f32,
    mode: u32,
    has_texture: u32,

    uv_offset: vec2<f32>,
};

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let params = material.base;

    let fade_view = distance(view.world_position, occlusion.eye.xyz) < 0.01;
    let elapsed = simulation_time.xy - material.scroll.zw;
    var uv_offset = params.uv_offset;
    if (any(material.scroll.xy != vec2<f32>(0.0))) {
        uv_offset = fract(params.uv_offset + fract(material.scroll.xy * elapsed.x) + fract(material.scroll.xy * elapsed.y));
    }

    var texel = vec4<f32>(1.0);
    if (params.has_texture == 1u) {
        texel = textureSample(base_color_texture, base_color_sampler, in.uv + uv_offset);
    }
    var diffuse = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    diffuse = in.color;
#endif
    var color = texel * diffuse;
    color.a *= params.opacity;

    if (params.mode == 3u) {

        color = vec4<f32>(vec3<f32>(1.0) - color.rgb, 1.0);
    }

    if (params.alpha_cutoff > 0.0 && color.a < params.alpha_cutoff) {
        discard;
    }
    if (fade_view && occlusion.eye.w > 0.0 && camera_fade(in.world_position.xyz, in.position.xy)) {
        discard;
    }
    return color;
}
