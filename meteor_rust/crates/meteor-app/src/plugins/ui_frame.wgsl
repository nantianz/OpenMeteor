#import bevy_ui::ui_vertex_output::UiVertexOutput

struct Frame {
    color: vec4<f32>,
    params: vec4<f32>,
};
@group(1) @binding(0) var<uniform> frame: Frame;

fn line(dist: f32, width: f32) -> f32 {
    return 1.0 - smoothstep(width * 0.5, width * 0.5 + 0.9, abs(dist));
}

fn over(acc: vec4<f32>, rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a + acc.rgb * (1.0 - a), a + acc.a * (1.0 - a));
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let size = in.size;
    let px = in.uv * size;
    let gold = frame.color.rgb;
    var acc = vec4<f32>(0.0);

    let band = max(1.0 - px.y / 36.0, 1.0 - (size.y - px.y) / 36.0);
    acc = over(acc, vec3<f32>(0.0), clamp(band, 0.0, 1.0) * 0.35 * frame.params.y);

    let fade = 1.0 - smoothstep(0.18, 0.5, abs(in.uv.x - 0.5)) * 0.82;
    let rules = max(line(px.y - 0.5, 1.0), line(size.y - 0.5 - px.y, 1.0)) * fade;
    acc = over(acc, gold, rules * 0.55);

    let q = min(px, size - px);
    let len = frame.params.x;
    var corner = 0.0;
    if q.x < len && q.y < len {
        let outer = max(line(q.y - 1.0, 1.6) * step(q.x, len), line(q.x - 1.0, 1.6) * step(q.y, len));
        let inner_len = len * 0.42;
        let inner = max(
            line(q.y - 5.0, 1.0) * step(4.0, q.x) * step(q.x, inner_len),
            line(q.x - 5.0, 1.0) * step(4.0, q.y) * step(q.y, inner_len),
        );
        let falloff = 1.0 - smoothstep(len * 0.55, len, max(q.x, q.y));
        corner = max(outer, inner * 0.8) * falloff;
    }
    acc = over(acc, gold * 1.15, corner * 0.95);

    if frame.params.z > 0.5 {
        let c = abs(px - vec2<f32>(size.x * 0.5, 0.5));
        let diamond = c.x * 0.5 + c.y;
        let lozenge = line(diamond - 3.5, 1.2) + (1.0 - smoothstep(1.2, 2.2, diamond));
        acc = over(acc, gold * 1.2, clamp(lozenge, 0.0, 1.0) * step(c.y, 6.0));
    }

    let alpha = acc.a * frame.color.a;
    return vec4<f32>(acc.rgb / max(acc.a, 0.00001), alpha);
}
