
#import bevy_pbr::forward_io::VertexOutput;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> gain: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = length(in.uv);
    let aa = max(fwidth(d), 0.001);
    var shape: f32;
    if in.uv_b.x > 0.5 {
        shape = pow(max(0.0, 1.0 - d), 2.4);
    } else {
        let core_radius = 1.0 / 2.6;
        let core = 1.0 - smoothstep(core_radius * 0.18, core_radius + aa, d);
        let halo = pow(max(0.0, 1.0 - d), 2.15);
        shape = core + 0.42 * halo;
    }

    let alpha = shape * in.color.a;
    if alpha <= 0.004 { discard; }
    return vec4<f32>(in.color.rgb * gain.rgb, alpha * gain.a);
}
