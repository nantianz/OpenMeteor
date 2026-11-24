#import bevy_ui::ui_vertex_output::UiVertexOutput

struct Atmosphere {
    ember: vec4<f32>,
    params: vec4<f32>,
};
@group(1) @binding(0) var<uniform> atmo: Atmosphere;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn hash2(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(hash(p), hash(p + vec2<f32>(19.19, 47.31)));
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x),
        u.y,
    );
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + a * noise(q);
        q = q * 2.03 + vec2<f32>(11.7, 3.1);
        a = a * 0.5;
    }
    return v;
}

fn over(acc: vec4<f32>, rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a + acc.rgb * (1.0 - a), a + acc.a * (1.0 - a));
}

fn embers(px: vec2<f32>, t: f32, cell: f32, speed: f32, layer: f32) -> f32 {

    let p = vec2<f32>(px.x + sin(px.y / 140.0 + t * 0.6 + layer) * 18.0, px.y + t * speed);
    let id = floor(p / cell);
    let r = hash2(id + layer * 17.0);
    if r.x < 1.0 - 0.10 * atmo.params.z {
        return 0.0;
    }
    let center = (id + vec2<f32>(0.2, 0.2) + hash2(id + 3.7 + layer) * 0.6) * cell;
    let dist = length(p - center);
    let radius = mix(0.8, 2.2, r.y) * (1.0 + layer * 0.35);
    let core = 1.0 - smoothstep(0.0, radius, dist);
    let glow = (1.0 - smoothstep(0.0, radius * 4.0, dist)) * 0.35;
    let flicker = 0.55 + 0.45 * sin(t * (3.0 + r.y * 5.0) + r.x * 40.0);
    return (core + glow) * flicker;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let intensity = atmo.params.y;
    if intensity <= 0.001 {
        return vec4<f32>(0.0);
    }
    let t = atmo.params.x;
    let uv = in.uv;
    let px = uv * in.size;
    var acc = vec4<f32>(0.0);

    var c = uv - vec2<f32>(0.5, 0.45);
    c.x = c.x * (in.size.x / max(in.size.y, 1.0)) * 0.62;
    let vignette = smoothstep(0.30, 0.95, length(c)) * 0.78 + smoothstep(0.70, 1.0, uv.y) * 0.30;
    acc = over(acc, vec3<f32>(0.0, 0.0, 0.0), clamp(vignette, 0.0, 0.92) * intensity);

    let mist_uv = px / 520.0 + vec2<f32>(t * 0.018, -t * 0.006);
    let mist = smoothstep(0.45, 0.85, fbm(mist_uv)) * smoothstep(0.35, 1.0, uv.y);
    acc = over(acc, vec3<f32>(0.62, 0.64, 0.68), mist * 0.10 * intensity);

    let rise = smoothstep(0.05, 0.95, uv.y);
    var e = embers(px, t, 92.0, 38.0, 0.0) * 0.9;
    e = e + embers(px, t, 150.0, 62.0, 1.0) * 0.8;
    e = e + embers(px, t, 240.0, 95.0, 2.0) * 0.6;
    let ember_alpha = clamp(e * rise, 0.0, 1.0) * atmo.ember.a * intensity;
    acc = over(acc, atmo.ember.rgb, ember_alpha);

    let g = hash(floor(px / 1.5) + floor(t * 24.0) * 7.31) - 0.5;
    acc = over(acc, vec3<f32>(step(0.0, g)), abs(g) * 0.05 * intensity);

    return vec4<f32>(acc.rgb / max(acc.a, 0.00001), acc.a);
}
