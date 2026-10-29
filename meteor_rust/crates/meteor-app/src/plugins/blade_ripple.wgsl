#import bevy_pbr::forward_io::VertexOutput;
#import bevy_pbr::mesh_view_bindings::{view_transmission_texture, view_transmission_sampler};

struct BladeRippleParams {
    time: f32,
    impact: f32,
    color: vec3<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: BladeRippleParams;

fn wind_hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn wind_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    return mix(mix(wind_hash(cell), wind_hash(cell + vec2<f32>(1.0, 0.0)), w.x),
        mix(wind_hash(cell + vec2<f32>(0.0, 1.0)), wind_hash(cell + vec2<f32>(1.0, 1.0)), w.x), w.y);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let age = clamp(in.uv.x, 0.0, 1.0);
    let along = clamp(in.uv.y, 0.0, 1.0);

    let border = smoothstep(0.0, 0.10, along) * (1.0 - smoothstep(0.90, 1.0, along));
    let fade = smoothstep(0.0, 0.04, age) * (1.0 - smoothstep(0.45, 1.0, age)) * border;
    var strength = 1.0;
    var life = 1.0;
#ifdef VERTEX_COLORS
    strength = in.color.r;
    life = in.color.g;
#endif

    strength = clamp(strength / max(life, 0.0001), 0.0, 1.0);
    let lifetime_alpha = clamp(life, 0.0, 1.0);

    let warped = along + 0.065 * (wind_noise(vec2<f32>(age * 3.0, along * 4.0)) - 0.5)
        + 0.05 * age * age;
    let grain = wind_noise(vec2<f32>(age * 3.4 + 7.3, warped * 14.0));
    let detail = wind_noise(vec2<f32>(age * 7.0 + 19.0, warped * 29.0));
    let texture = grain * 0.72 + detail * 0.28;
    let erosion = age * age * 0.24 - 0.10;
    let coverage = smoothstep(erosion - 0.065, erosion + 0.065, texture);

    let age_gradient = vec2<f32>(dpdx(in.uv.x), dpdy(in.uv.x));
    let blade_gradient = vec2<f32>(dpdx(in.uv.y), dpdy(in.uv.y));
    let determinant = age_gradient.x * blade_gradient.y - age_gradient.y * blade_gradient.x;
    let sweep = vec2<f32>(-blade_gradient.y, blade_gradient.x) * sign(determinant);
    let direction = sweep / max(length(sweep), 0.00001);
    let dimensions = vec2<f32>(textureDimensions(view_transmission_texture));

    let pixels = (24.0 * (1.0 - age * 0.65) * (0.7 + along * 0.3)) * sqrt(fade * strength * coverage)
        * (1.0 + params.impact * 0.35) * dimensions.y / 720.0;

    let uv = clamp((in.position.xy - direction * pixels) / dimensions,
        vec2<f32>(0.5) / dimensions, vec2<f32>(1.0) - vec2<f32>(0.5) / dimensions);
    let background = textureSampleLevel(view_transmission_texture, view_transmission_sampler, uv, 0.0).rgb;

    let envelope = smoothstep(0.0, 0.06, age)
        * (1.0 - smoothstep(0.25, 1.0, age)) * clamp(strength, 0.0, 1.0);

    let folds = smoothstep(0.28, 0.78, texture);
    let sheet = border * smoothstep(0.05, 0.42, along) * (0.035 + folds * 0.22);

    let width = max(0.009, min(fwidth(in.uv.y) * 0.75, 0.025));
    let rim_noise = wind_noise(vec2<f32>(age * 8.0 + 3.0, 2.7));
    let tip = 1.0 - width * (1.0 + rim_noise * 0.9);
    let distance_to_tip = abs(along - tip);
    let line = 1.0 - smoothstep(width * 0.35, width * 1.8, distance_to_tip);
    let halo = 1.0 - smoothstep(width, width * 3.5, distance_to_tip);
    let broken_rim = smoothstep(0.22, 0.62, rim_noise) * (1.0 - age * 0.65);
    let sheen = (sheet + (line * 0.38 + halo * 0.025) * broken_rim)
        * envelope * coverage;

    let tinted = background + max(params.color - background, vec3<f32>(0.0)) * min(sheen, 0.30);
    let alpha = clamp(sqrt(fade * strength * coverage) * 1.4, 0.0, 0.96) * lifetime_alpha;
    return vec4<f32>(tinted, alpha);
}
