#ifdef MULTISAMPLED
@group(0) @binding(0) var source: texture_depth_multisampled_2d;
#else
@group(0) @binding(0) var source: texture_depth_2d;
#endif
@group(0) @binding(1) var mip0: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var mip1: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var mip2: texture_storage_2d<r32float, write>;
@group(0) @binding(4) var mip3: texture_storage_2d<r32float, write>;

var<workgroup> nearest_depth: array<f32, 64>;

fn reduce_2x2(index: u32, step: u32) -> f32 {
    let row = 8u * step;
    return max(max(nearest_depth[index], nearest_depth[index + step]),
               max(nearest_depth[index + row], nearest_depth[index + row + step]));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>,
        @builtin(local_invocation_index) index: u32) {
    var nearest = 0.0;
    let size = textureDimensions(source);
    for (var y = 0u; y < 4u; y += 1u) {
        for (var x = 0u; x < 4u; x += 1u) {
            let p = id.xy * 4u + vec2<u32>(x, y);
            if (all(p < size)) {
                nearest = max(nearest, textureLoad(source, vec2<i32>(p), 0));
            }
        }
    }

    nearest += 1.0;
    nearest_depth[index] = nearest;
    textureStore(mip0, vec2<i32>(id.xy), vec4<f32>(nearest));
    workgroupBarrier();
    if (all(local.xy % 2u == vec2<u32>(0u))) {
        let value = reduce_2x2(index, 1u);
        nearest_depth[index] = value;
        textureStore(mip1, vec2<i32>(id.xy / 2u), vec4<f32>(value));
    }
    workgroupBarrier();
    if (all(local.xy % 4u == vec2<u32>(0u))) {
        let value = reduce_2x2(index, 2u);
        nearest_depth[index] = value;
        textureStore(mip2, vec2<i32>(id.xy / 4u), vec4<f32>(value));
    }
    workgroupBarrier();
    if (index == 0u) {
        textureStore(mip3, vec2<i32>(id.xy / 8u), vec4<f32>(reduce_2x2(0u, 4u)));
    }
}
