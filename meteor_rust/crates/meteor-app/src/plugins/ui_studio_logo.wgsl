#import bevy_ui::ui_vertex_output::UiVertexOutput
@group(1) @binding(0) var<uniform> params: vec4<f32>;
@group(1) @binding(1) var logo: texture_2d<f32>;
@group(1) @binding(2) var logo_sampler: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    if params.x <= 0.0 { return vec4<f32>(0.0); }
    if params.y < 0.0001 {
        return vec4<f32>(1.0, 1.0, 1.0, textureSampleLevel(logo, logo_sampler, in.uv, 0.0).a * params.x);
    }

    let radial = in.uv - vec2<f32>(0.5);
    var alpha = 0.0;
    for (var i = 0; i < 24; i += 1) {
        let zoom = 1.0 + params.y * f32(i) / 23.0;
        let uv = vec2<f32>(0.5) + radial / zoom;
        alpha += textureSampleLevel(logo, logo_sampler, uv, 0.0).a;
    }
    return vec4<f32>(1.0, 1.0, 1.0, alpha / 24.0 * params.x);
}
