#import bevy_pbr::prepass_io::VertexOutput;
#ifdef PREPASS_FRAGMENT
#import bevy_pbr::prepass_io::FragmentOutput;
#endif
#import bevy_pbr::mesh_view_bindings::view;
#import bevy_pbr::prepass_bindings;

struct GmdlMaterialParams {
    opacity: f32,
    alpha_cutoff: f32,
    mode: u32,
    has_texture: u32,
    uv_offset: vec2<f32>,
};
struct GmdlGpuParams { base: GmdlMaterialParams, scroll: vec4<f32>, detail: vec4<f32>, body_glow: vec4<f32>, body_style: vec4<f32>, water: vec4<f32>, water_bounds: vec4<f32>, first_person_clip: vec4<f32>, water_dynamics_bounds: vec4<f32>, };
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: GmdlGpuParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;
struct CameraOcclusionParams {
    eye: vec4<f32>, player: vec4<f32>, watched: vec4<f32>, self_eye: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> occlusion: CameraOcclusionParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> simulation_time: vec4<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> lighting: vec4<f32>;

@fragment
fn fragment(in: VertexOutput)
#ifdef PREPASS_FRAGMENT
    -> FragmentOutput
#endif
{
    let params = material.base;
    if (params.mode == 3u && lighting.w > 0.0) { discard; }
    if (params.alpha_cutoff > 0.0) {
    var alpha = params.opacity;
#ifdef VERTEX_COLORS
    alpha *= in.color.a;
#endif
#ifdef VERTEX_UVS_A
        if (params.has_texture != 0u) {
    let elapsed = simulation_time.xy - material.scroll.zw;
    let offset = fract(params.uv_offset + fract(material.scroll.xy * elapsed.x) + fract(material.scroll.xy * elapsed.y));
        alpha *= textureSample(base_color_texture, base_color_sampler, in.uv + offset).a;
    }
#endif
        if (alpha < params.alpha_cutoff) { discard; }
    }
    if (material.first_person_clip.w > 0.0 && occlusion.self_eye.w > 0.0
        && distance(view.world_position, occlusion.self_eye.xyz) < 0.01
        && distance(in.world_position.xyz, occlusion.self_eye.xyz) < material.first_person_clip.w) {
        discard;
    }
    #ifdef PREPASS_FRAGMENT
    var out: FragmentOutput;
#ifdef NORMAL_PREPASS
    out.normal = vec4(in.world_normal * 0.5 + vec3(0.5), 1.0);
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
#ifdef MOTION_VECTOR_PREPASS
    let current = view.unjittered_clip_from_world * in.world_position;
    let previous = prepass_bindings::previous_view_uniforms.clip_from_world * in.previous_world_position;
    out.motion_vector = (current.xy / current.w - previous.xy / previous.w) * vec2(0.5, -0.5);
#endif
    return out;
    #endif
}
