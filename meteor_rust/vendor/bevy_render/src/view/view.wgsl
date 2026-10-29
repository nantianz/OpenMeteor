#define_import_path bevy_render::view

struct ColorGrading {
    balance: mat3x3<f32>,
    saturation: vec3<f32>,
    contrast: vec3<f32>,
    gamma: vec3<f32>,
    gain: vec3<f32>,
    lift: vec3<f32>,
    midtone_range: vec2<f32>,
    exposure: f32,
    hue: f32,
    post_saturation: f32,
}

struct View {
    clip_from_world: mat4x4<f32>,
    unjittered_clip_from_world: mat4x4<f32>,
    world_from_clip: mat4x4<f32>,
    world_from_view: mat4x4<f32>,
    view_from_world: mat4x4<f32>,

    clip_from_view: mat4x4<f32>,
    view_from_clip: mat4x4<f32>,
    world_position: vec3<f32>,
    exposure: f32,

    viewport: vec4<f32>,
    main_pass_viewport: vec4<f32>,

    frustum: array<vec4<f32>, 6>,

    lod_view_world_position: vec3<f32>,
    color_grading: ColorGrading,
    mip_bias: f32,
    frame_count: u32,
};

fn position_view_to_world(view_pos: vec3<f32>, world_from_view: mat4x4<f32>) -> vec3<f32> {
    let world_pos = world_from_view * vec4(view_pos, 1.0);
    return world_pos.xyz;
}

fn position_clip_to_world(clip_pos: vec4<f32>, world_from_clip: mat4x4<f32>) -> vec3<f32> {
    let world_pos = world_from_clip * clip_pos;
    return world_pos.xyz;
}

fn position_ndc_to_world(ndc_pos: vec3<f32>, world_from_clip: mat4x4<f32>) -> vec3<f32> {
    let world_pos = world_from_clip * vec4(ndc_pos, 1.0);
    return world_pos.xyz / world_pos.w;
}

fn direction_view_to_world(view_dir: vec3<f32>, world_from_view: mat4x4<f32>) -> vec3<f32> {
    let world_dir = world_from_view * vec4(view_dir, 0.0);
    return world_dir.xyz;
}

fn direction_clip_to_world(clip_dir: vec4<f32>, world_from_clip: mat4x4<f32>) -> vec3<f32> {
    let world_dir = world_from_clip * clip_dir;
    return world_dir.xyz;
}

fn position_world_to_view(world_pos: vec3<f32>, view_from_world: mat4x4<f32>) -> vec3<f32> {
    let view_pos = view_from_world * vec4(world_pos, 1.0);
    return view_pos.xyz;
}

fn position_clip_to_view(clip_pos: vec4<f32>, view_from_clip: mat4x4<f32>) -> vec3<f32> {
    let view_pos = view_from_clip * clip_pos;
    return view_pos.xyz;
}

fn position_ndc_to_view(ndc_pos: vec3<f32>, view_from_clip: mat4x4<f32>) -> vec3<f32> {
    let view_pos = view_from_clip * vec4(ndc_pos, 1.0);
    return view_pos.xyz / view_pos.w;
}

fn direction_world_to_view(world_dir: vec3<f32>, view_from_world: mat4x4<f32>) -> vec3<f32> {
    let view_dir = view_from_world * vec4(world_dir, 0.0);
    return view_dir.xyz;
}

fn direction_clip_to_view(clip_dir: vec4<f32>, view_from_clip: mat4x4<f32>) -> vec3<f32> {
    let view_dir = view_from_clip * clip_dir;
    return view_dir.xyz;
}

fn position_world_to_clip(world_pos: vec3<f32>, clip_from_world: mat4x4<f32>) -> vec4<f32> {
    let clip_pos = clip_from_world * vec4(world_pos, 1.0);
    return clip_pos;
}

fn position_view_to_clip(view_pos: vec3<f32>, clip_from_view: mat4x4<f32>) -> vec4<f32> {
    let clip_pos = clip_from_view * vec4(view_pos, 1.0);
    return clip_pos;
}

fn direction_world_to_clip(world_dir: vec3<f32>, clip_from_world: mat4x4<f32>) -> vec4<f32> {
    let clip_dir = clip_from_world * vec4(world_dir, 0.0);
    return clip_dir;
}

fn direction_view_to_clip(view_dir: vec3<f32>, clip_from_view: mat4x4<f32>) -> vec4<f32> {
    let clip_dir = clip_from_view * vec4(view_dir, 0.0);
    return clip_dir;
}

fn position_world_to_ndc(world_pos: vec3<f32>, clip_from_world: mat4x4<f32>) -> vec3<f32> {
    let ndc_pos = clip_from_world * vec4(world_pos, 1.0);
    return ndc_pos.xyz / ndc_pos.w;
}

fn position_view_to_ndc(view_pos: vec3<f32>, clip_from_view: mat4x4<f32>) -> vec3<f32> {
    let ndc_pos = clip_from_view * vec4(view_pos, 1.0);
    return ndc_pos.xyz / ndc_pos.w;
}

fn perspective_camera_near(clip_from_view: mat4x4<f32>) -> f32 {
    return clip_from_view[3][2];
}

fn depth_ndc_to_view_z(ndc_depth: f32, clip_from_view: mat4x4<f32>, view_from_clip: mat4x4<f32>) -> f32 {
#ifdef VIEW_PROJECTION_PERSPECTIVE
    return -perspective_camera_near(clip_from_view) / ndc_depth;
#else ifdef VIEW_PROJECTION_ORTHOGRAPHIC
    return -(clip_from_view[3][2] - ndc_depth) / clip_from_view[2][2];
#else
    let view_pos = view_from_clip * vec4(0.0, 0.0, ndc_depth, 1.0);
    return view_pos.z / view_pos.w;
#endif
}

fn view_z_to_depth_ndc(view_z: f32, clip_from_view: mat4x4<f32>) -> f32 {
#ifdef VIEW_PROJECTION_PERSPECTIVE
    return -perspective_camera_near(clip_from_view) / view_z;
#else ifdef VIEW_PROJECTION_ORTHOGRAPHIC
    return clip_from_view[3][2] + view_z * clip_from_view[2][2];
#else
    let ndc_pos = clip_from_view * vec4(0.0, 0.0, view_z, 1.0);
    return ndc_pos.z / ndc_pos.w;
#endif
}

fn ndc_to_uv(ndc: vec2<f32>) -> vec2<f32> {
    return ndc * vec2(0.5, -0.5) + vec2(0.5);
}

fn uv_to_ndc(uv: vec2<f32>) -> vec2<f32> {
    return uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
}

fn frag_coord_to_uv(frag_coord: vec2<f32>, viewport: vec4<f32>) -> vec2<f32> {
    return (frag_coord - viewport.xy) / viewport.zw;
}

fn frag_coord_to_ndc(frag_coord: vec4<f32>, viewport: vec4<f32>) -> vec3<f32> {
    return vec3(uv_to_ndc(frag_coord_to_uv(frag_coord.xy, viewport)), frag_coord.z);
}

fn ndc_to_frag_coord(ndc: vec2<f32>, viewport: vec4<f32>) -> vec2<f32> {
    return ndc_to_uv(ndc) * viewport.zw;
}
