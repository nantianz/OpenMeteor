#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var<uniform> params: vec4<f32>;
@group(1) @binding(1) var background: texture_2d<f32>;
@group(1) @binding(2) var background_sampler: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {

    let weights = array<f32, 5>(0.054488685, 0.24420134, 0.40261996, 0.24420134, 0.054488685);
    let step = params.xy * params.z;
    var color = vec4<f32>(0.0);
    for (var y = 0; y < 5; y++) {
        for (var x = 0; x < 5; x++) {
            let offset = vec2<f32>(f32(x) - 2.0, f32(y) - 2.0) * step;
            color += textureSampleLevel(background, background_sampler, in.uv + offset, 0.0) * weights[x] * weights[y];
        }
    }
    return color;
}
