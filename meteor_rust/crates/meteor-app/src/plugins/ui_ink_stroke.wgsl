#import bevy_ui::ui_vertex_output::UiVertexOutput

struct InkStroke {
    color: vec4<f32>,
    params: vec4<f32>,
};
@group(1) @binding(0) var<uniform> stroke: InkStroke;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
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

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let progress = clamp(stroke.params.x, 0.0, 1.0);
    if progress <= 0.001 {
        return vec4<f32>(0.0);
    }
    let seed = stroke.params.y;
    let x = in.uv.x;
    let y = in.uv.y * 2.0 - 1.0;
    let aspect = max(in.size.x / max(in.size.y, 1.0), 1.0);

    let wobble = (noise(vec2<f32>(x * 5.0 + seed, seed * 3.1)) - 0.5) * 0.22
        + (noise(vec2<f32>(x * aspect * 3.0, seed + 7.0)) - 0.5) * 0.14;
    let taper = 1.0 - smoothstep(0.30, 1.0, x) * 0.55;
    let head = smoothstep(0.0, 0.05, x);
    let half_height = (0.66 + wobble) * taper * mix(0.55, 1.0, head);
    let bend = (noise(vec2<f32>(x * 1.6, seed * 1.7)) - 0.5) * 0.22;
    let d = abs(y - bend) / max(half_height, 0.001);
    var body = 1.0 - smoothstep(0.78, 1.0, d);

    let bristle_freq = max(in.size.y / 3.0, 6.0);
    let streak = noise(vec2<f32>(x * 2.5 + seed, y * bristle_freq));
    let dry = smoothstep(0.40, 1.0, x);
    body *= mix(1.0, smoothstep(dry * 0.7, dry * 0.7 + 0.18, streak), dry);

    let fiber = 0.80 + 0.20 * noise(vec2<f32>(x * aspect * 40.0, y * 9.0 + seed));
    let pool = 0.85 + 0.15 * noise(vec2<f32>(x * aspect * 4.0 + stroke.params.z * 0.15, y * 2.0));

    let edge = progress * 1.1;
    let reveal = 1.0 - smoothstep(edge - 0.10, edge, x);
    let tail = 1.0 - smoothstep(0.88, 1.0, x);

    let alpha = clamp(body * fiber * reveal * tail, 0.0, 1.0) * stroke.color.a;

    let shade = mix(0.72, 1.18, clamp(d * 0.8 + (1.0 - fiber), 0.0, 1.0)) * pool;
    return vec4<f32>(stroke.color.rgb * shade, alpha);
}
