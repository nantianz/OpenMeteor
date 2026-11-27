@group(0) @binding(0) var static_depth: texture_depth_2d;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(static_depth, vec2<i32>(position.xy), 0);
}
