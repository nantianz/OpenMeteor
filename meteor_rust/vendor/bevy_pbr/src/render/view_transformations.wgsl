#define_import_path bevy_pbr::view_transformations

#import bevy_pbr::mesh_view_bindings as view_bindings
#import bevy_pbr::prepass_bindings

fn position_view_to_world(view_pos: vec3<f32>) -> vec3<f32> {
    let world_pos = view_bindings::view.world_from_view * vec4(view_pos, 1.0);
    return world_pos.xyz;
}

fn position_clip_to_world(clip_pos: vec4<f32>) -> vec3<f32> {
    let world_pos = view_bindings::view.world_from_clip * clip_pos;
    return world_pos.xyz;
}

fn position_ndc_to_world(ndc_pos: vec3<f32>) -> vec3<f32> {
    let world_pos = view_bindings::view.world_from_clip * vec4(ndc_pos, 1.0);
    return world_pos.xyz / world_pos.w;
}

fn direction_view_to_world(view_dir: vec3<f32>) -> vec3<f32> {
    let world_dir = view_bindings::view.world_from_view * vec4(view_dir, 0.0);
    return world_dir.xyz;
}

fn direction_clip_to_world(clip_dir: vec4<f32>) -> vec3<f32> {
    let world_dir = view_bindings::view.world_from_clip * clip_dir;
    return world_dir.xyz;
}

fn position_world_to_view(world_pos: vec3<f32>) -> vec3<f32> {
    let view_pos = view_bindings::view.view_from_world * vec4(world_pos, 1.0);
    return view_pos.xyz;
}

fn position_clip_to_view(clip_pos: vec4<f32>) -> vec3<f32> {
    let view_pos = view_bindings::view.view_from_clip * clip_pos;
    return view_pos.xyz;
}

fn position_ndc_to_view(ndc_pos: vec3<f32>) -> vec3<f32> {
    let view_pos = view_bindings::view.view_from_clip * vec4(ndc_pos, 1.0);
    return view_pos.xyz / view_pos.w;
}

fn direction_world_to_view(world_dir: vec3<f32>) -> vec3<f32> {
    let view_dir = view_bindings::view.view_from_world * vec4(world_dir, 0.0);
    return view_dir.xyz;
}

fn direction_clip_to_view(clip_dir: vec4<f32>) -> vec3<f32> {
    let view_dir = view_bindings::view.view_from_clip * clip_dir;
    return view_dir.xyz;
}

fn position_world_to_prev_view(world_pos: vec3<f32>) -> vec3<f32> {
    let view_pos = prepass_bindings::previous_view_uniforms.view_from_world *
        vec4(world_pos, 1.0);
    return view_pos.xyz;
}

fn position_world_to_prev_ndc(world_pos: vec3<f32>) -> vec3<f32> {
    let ndc_pos = prepass_bindings::previous_view_uniforms.clip_from_world *
        vec4(world_pos, 1.0);
    return ndc_pos.xyz / ndc_pos.w;
}

fn position_world_to_clip(world_pos: vec3<f32>) -> vec4<f32> {
    let clip_pos = view_bindings::view.clip_from_world * vec4(world_pos, 1.0);
    return clip_pos;
}

fn position_view_to_clip(view_pos: vec3<f32>) -> vec4<f32> {
    let clip_pos = view_bindings::view.clip_from_view * vec4(view_pos, 1.0);
    return clip_pos;
}

fn direction_world_to_clip(world_dir: vec3<f32>) -> vec4<f32> {
    let clip_dir = view_bindings::view.clip_from_world * vec4(world_dir, 0.0);
    return clip_dir;
}

fn direction_view_to_clip(view_dir: vec3<f32>) -> vec4<f32> {
    let clip_dir = view_bindings::view.clip_from_view * vec4(view_dir, 0.0);
    return clip_dir;
}

fn position_world_to_ndc(world_pos: vec3<f32>) -> vec3<f32> {
    let ndc_pos = view_bindings::view.clip_from_world * vec4(world_pos, 1.0);
    return ndc_pos.xyz / ndc_pos.w;
}

fn position_view_to_ndc(view_pos: vec3<f32>) -> vec3<f32> {
    let ndc_pos = view_bindings::view.clip_from_view * vec4(view_pos, 1.0);
    return ndc_pos.xyz / ndc_pos.w;
}

fn perspective_camera_near() -> f32 {
    return view_bindings::view.clip_from_view[3][2];
}

fn depth_ndc_to_view_z(ndc_depth: f32) -> f32 {
#ifdef VIEW_PROJECTION_PERSPECTIVE
    return -perspective_camera_near() / ndc_depth;
#else ifdef VIEW_PROJECTION_ORTHOGRAPHIC
    return -(view_bindings::view.clip_from_view[3][2] - ndc_depth) / view_bindings::view.clip_from_view[2][2];
#else
    let view_pos = view_bindings::view.view_from_clip * vec4(0.0, 0.0, ndc_depth, 1.0);
    return view_pos.z / view_pos.w;
#endif
}

fn view_z_to_depth_ndc(view_z: f32) -> f32 {
#ifdef VIEW_PROJECTION_PERSPECTIVE
    return -perspective_camera_near() / view_z;
#else ifdef VIEW_PROJECTION_ORTHOGRAPHIC
    return view_bindings::view.clip_from_view[3][2] + view_z * view_bindings::view.clip_from_view[2][2];
#else
    let ndc_pos = view_bindings::view.clip_from_view * vec4(0.0, 0.0, view_z, 1.0);
    return ndc_pos.z / ndc_pos.w;
#endif
}

fn prev_view_z_to_depth_ndc(view_z: f32) -> f32 {
#ifdef VIEW_PROJECTION_PERSPECTIVE
    return -perspective_camera_near() / view_z;
#else ifdef VIEW_PROJECTION_ORTHOGRAPHIC
    return prepass_bindings::previous_view_uniforms.clip_from_view[3][2] +
        view_z * prepass_bindings::previous_view_uniforms.clip_from_view[2][2];
#else
    let ndc_pos = prepass_bindings::previous_view_uniforms.clip_from_view *
        vec4(0.0, 0.0, view_z, 1.0);
    return ndc_pos.z / ndc_pos.w;
#endif
}

fn ndc_to_uv(ndc: vec2<f32>) -> vec2<f32> {
    return ndc * vec2(0.5, -0.5) + vec2(0.5);
}

fn uv_to_ndc(uv: vec2<f32>) -> vec2<f32> {
    return uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
}

fn frag_coord_to_uv(frag_coord: vec2<f32>) -> vec2<f32> {
    return (frag_coord - view_bindings::view.viewport.xy) / view_bindings::view.viewport.zw;
}

fn frag_coord_to_ndc(frag_coord: vec4<f32>) -> vec3<f32> {
    return vec3(uv_to_ndc(frag_coord_to_uv(frag_coord.xy)), frag_coord.z);
}

fn ndc_to_frag_coord(ndc: vec2<f32>) -> vec2<f32> {
    return ndc_to_uv(ndc) * view_bindings::view.viewport.zw;
}
