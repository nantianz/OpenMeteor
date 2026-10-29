#import bevy_ui::ui_vertex_output::UiVertexOutput
@group(1) @binding(0) var<uniform> params: vec4<f32>;

fn hash(p: vec2<f32>) -> f32 {
    var h = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    h += dot(h, h.yzx + 33.33);
    return fract((h.x + h.y) * h.z);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f*f*f*(f*(f*6.0-15.0)+10.0);
    return mix(
        mix(hash(i), hash(i+vec2<f32>(1.0,0.0)), u.x),
        mix(hash(i+vec2<f32>(0.0,1.0)), hash(i+vec2<f32>(1.0,1.0)), u.x), u.y);
}

fn billow(p0: vec2<f32>) -> f32 {
    var p = p0;
    var density = 0.0;
    var weight = 0.62;

    for (var i = 0; i < 4; i += 1) {
        density += weight * noise(p);
        p = mat2x2<f32>(0.8,0.6,-0.6,0.8) * p * 1.93 + vec2<f32>(7.1,3.4);
        weight *= 0.38;
    }
    return density;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;

    let t = params.x * 3.0;

    let p = (uv-0.5) * vec2<f32>(params.z,1.0) * 2.4;
    let drift = vec2<f32>(t*0.075,-t*0.025);
    let curl = vec2<f32>(
        noise(p*0.72+vec2<f32>(0.0,t*0.04)),
        noise(p*0.72+vec2<f32>(6.4,-t*0.035))) - 0.5;

    let far_field = billow(p*0.85 + drift*0.55 + curl*0.55 + vec2<f32>(4.3,1.7));
    let near_field = billow(p + vec2<f32>(-drift.x,drift.y) + curl*1.35);
    let far_fog = smoothstep(0.24,0.72,far_field);
    let near_fog = smoothstep(0.28,0.68,near_field);
    let veil_height = (p.y+0.12+curl.y*0.7)*0.8;
    let veil = exp(-veil_height*veil_height);

    let light_offset = p-vec2<f32>(-0.65,-0.4);
    let light = exp(-dot(light_offset,light_offset)*0.28);
    var color = mix(vec3<f32>(0.009,0.014,0.021),vec3<f32>(0.065,0.085,0.105),veil*0.65);
    color = mix(color,vec3<f32>(0.18,0.23,0.27),far_fog*(0.40+light*0.25));
    color = mix(color,vec3<f32>(0.30,0.35,0.39)*(0.62+light*0.38),near_fog*0.82);
    let title_offset = p*vec2<f32>(0.9,1.6);
    color *= 1.0-exp(-dot(title_offset,title_offset))*0.32;

    let density = far_fog*0.4 + near_fog*0.6;
    let dissolve = smoothstep(density*0.25,0.75+density*0.25,params.w);
    let alpha = (1.0-dissolve)*params.y;
    return vec4<f32>(color,alpha);
}
