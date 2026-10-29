#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;

struct ViewportPostProcessSettings {
    brightness: f32,
    contrast: f32,
    padding: vec2<f32>,
};

@group(0) @binding(2) var<uniform> settings: ViewportPostProcessSettings;

fn linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let low = linear * 12.92;
    let high = 1.055 * pow(max(linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(low, high, linear > vec3<f32>(0.0031308));
}

fn srgb_to_linear(srgb: vec3<f32>) -> vec3<f32> {
    let low = srgb / 12.92;
    let high = pow(max((srgb + 0.055) / 1.055, vec3<f32>(0.0)), vec3<f32>(2.4));
    return select(low, high, srgb > vec3<f32>(0.04045));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let source = textureSampleLevel(
        screen_texture,
        screen_sampler,
        in.uv,
        0.0,
    );

    if settings.brightness == 1.0 && settings.contrast == 1.0 {
        return source;
    }
    var srgb = linear_to_srgb(source.rgb);
    srgb = srgb * settings.brightness;
    srgb = (srgb - vec3<f32>(0.5)) * settings.contrast + vec3<f32>(0.5);
    return vec4<f32>(srgb_to_linear(clamp(srgb, vec3<f32>(0.0), vec3<f32>(1.0))), source.a);
}
