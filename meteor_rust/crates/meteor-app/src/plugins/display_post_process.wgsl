#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;

struct ViewportPostProcessSettings {
    brightness: f32,
    contrast: f32,
    sharpness: f32,
    grayscale: f32,
    input_size: vec2<f32>,
    output_size: vec2<f32>,
    underwater: vec4<f32>,
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

fn sample_input(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(screen_texture, screen_sampler, uv, 0.0);
}

fn cas_rcas(center: vec3<f32>, uv: vec2<f32>, sharpness: f32) -> vec3<f32> {
    if sharpness <= 0.0 {
        return center;
    }
    let tex_size = max(settings.output_size, vec2<f32>(1.0));

    let pixel = 1.0 / tex_size;
    let b = sample_sharpen_neighbor(uv + vec2<f32>(0.0, -pixel.y));
    let d = sample_sharpen_neighbor(uv + vec2<f32>(-pixel.x, 0.0));
    let f = sample_sharpen_neighbor(uv + vec2<f32>(pixel.x, 0.0));
    let h = sample_sharpen_neighbor(uv + vec2<f32>(0.0, pixel.y));

    let mn = min(min(min(d, center), min(f, b)), h);
    let mx = max(max(max(d, center), max(f, b)), h);

    let amp = clamp(min(mn, 2.0 - mx) / max(mx, vec3<f32>(1e-5)), vec3<f32>(0.0), vec3<f32>(1.0));
    let weight = -sqrt(amp) * mix(0.0, 0.22, sharpness);
    let sharpened = (b + d + f + h) * weight + center;
    return clamp(sharpened / (1.0 + 4.0 * weight), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn sample_sharpen_neighbor(uv: vec2<f32>) -> vec3<f32> {
    return sample_input(uv).rgb;
}

fn sample_resolved(uv: vec2<f32>) -> vec4<f32> {
    return sample_input(uv);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    var uv = in.uv;
    if settings.underwater.x > 0.0 {
        let time = settings.underwater.z;
        let depth = settings.underwater.y;
        let warp = min(depth * 0.5, 1.0) * 0.0015;
        uv += vec2<f32>(
            sin(uv.y * 37.0 + time * 1.3) * warp,
            sin(uv.x * 29.0 - time * 0.9) * warp * 0.65,
        );
    }
    let source = sample_resolved(uv);
    var rgb = source.rgb;

    if settings.sharpness > 0.0 {
        rgb = cas_rcas(rgb, uv, settings.sharpness);
    }

    if settings.underwater.x > 0.0 {
        let depth = settings.underwater.y;
        let amount = smoothstep(0.05, 0.30, depth) * min(0.45 + depth * 0.08, 0.72);
        let poison = settings.underwater.x > 1.5;
        let tint = select(vec3<f32>(0.025, 0.16, 0.14), vec3<f32>(0.045, 0.17, 0.035), poison);
        rgb = mix(rgb, rgb * 0.62 + tint * 0.38, amount);
    }

    if settings.grayscale > 0.0 {
        let luminance = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(rgb, vec3<f32>(luminance), settings.grayscale);
    }

    if settings.brightness == 1.0 && settings.contrast == 1.0 {
        return vec4<f32>(rgb, source.a);
    }
    var srgb = linear_to_srgb(rgb);
    srgb = srgb * settings.brightness;
    srgb = (srgb - vec3<f32>(0.5)) * settings.contrast + vec3<f32>(0.5);
    return vec4<f32>(srgb_to_linear(clamp(srgb, vec3<f32>(0.0), vec3<f32>(1.0))), source.a);
}
