#define_import_path bevy_pbr::utils

#import bevy_pbr::rgb9e5
#import bevy_render::maths::{PI, PI_2, orthonormalize}

fn rand_u(state: ptr<function, u32>) -> u32 {
    *state = *state * 747796405u + 2891336453u;
    let word = ((*state >> ((*state >> 28u) + 4u)) ^ *state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn rand_f(state: ptr<function, u32>) -> f32 {
    *state = *state * 747796405u + 2891336453u;
    let word = ((*state >> ((*state >> 28u) + 4u)) ^ *state) * 277803737u;
    return f32((word >> 22u) ^ word) * bitcast<f32>(0x2f800004u);
}

fn rand_vec2f(state: ptr<function, u32>) -> vec2<f32> {
    return vec2(rand_f(state), rand_f(state));
}

fn rand_range_u(n: u32, state: ptr<function, u32>) -> u32 {
    return rand_u(state) % n;
}

fn coords_to_viewport_uv(position: vec2<f32>, viewport: vec4<f32>) -> vec2<f32> {
    return (position - viewport.xy) / viewport.zw;
}

fn octahedral_encode(v: vec3<f32>) -> vec2<f32> {
    var n = v / (abs(v.x) + abs(v.y) + abs(v.z));
    let octahedral_wrap = (1.0 - abs(n.yx)) * select(vec2(-1.0), vec2(1.0), n.xy > vec2f(0.0));
    let n_xy = select(octahedral_wrap, n.xy, n.z >= 0.0);
    return n_xy * 0.5 + 0.5;
}

fn octahedral_decode(v: vec2<f32>) -> vec3<f32> {
    let f = v * 2.0 - 1.0;
    return octahedral_decode_signed(f);
}

fn octahedral_decode_signed(v: vec2<f32>) -> vec3<f32> {
    var n = vec3(v.xy, 1.0 - abs(v.x) - abs(v.y));
    let t = saturate(-n.z);
    let w = select(vec2(t), vec2(-t), n.xy >= vec2(0.0));
    n = vec3(n.xy + w, n.z);
    return normalize(n);
}

fn interleaved_gradient_noise(pixel_coordinates: vec2<f32>, frame: u32) -> f32 {
    let xy = pixel_coordinates + 5.588238 * f32(frame % 64u);
    return fract(52.9829189 * fract(0.06711056 * xy.x + 0.00583715 * xy.y));
}

fn hammersley_2d(i: u32, n: u32) -> vec2f {
    let inv_n = 1.0 / f32(n);
    let vdc = f32(reverseBits(i)) * 2.3283064365386963e-10;
    return vec2f(f32(i) * inv_n, vdc);
}

const SPIRAL_OFFSET_0_ = vec2<f32>(-0.7071,  0.7071);
const SPIRAL_OFFSET_1_ = vec2<f32>(-0.0000, -0.8750);
const SPIRAL_OFFSET_2_ = vec2<f32>( 0.5303,  0.5303);
const SPIRAL_OFFSET_3_ = vec2<f32>(-0.6250, -0.0000);
const SPIRAL_OFFSET_4_ = vec2<f32>( 0.3536, -0.3536);
const SPIRAL_OFFSET_5_ = vec2<f32>(-0.0000,  0.3750);
const SPIRAL_OFFSET_6_ = vec2<f32>(-0.1768, -0.1768);
const SPIRAL_OFFSET_7_ = vec2<f32>( 0.1250,  0.0000);

fn sample_cosine_hemisphere(normal: vec3<f32>, rng: ptr<function, u32>) -> vec3<f32> {
    let cos_theta = 1.0 - 2.0 * rand_f(rng);
    let phi = PI_2 * rand_f(rng);
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let direction = normal + vec3(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
    let len_sq = dot(direction, direction);
    if len_sq < 1e-8 { return normal; }
    return direction * inverseSqrt(len_sq);
}

fn sample_uniform_hemisphere(normal: vec3<f32>, rng: ptr<function, u32>) -> vec3<f32> {
    let cos_theta = rand_f(rng);
    let phi = PI_2 * rand_f(rng);
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let x = sin_theta * cos(phi);
    let y = sin_theta * sin(phi);
    let z = cos_theta;
    return orthonormalize(normal) * vec3(x, y, z);
}

fn uniform_hemisphere_inverse_pdf() -> f32 {
    return PI_2;
}

fn sample_disk(disk_radius: f32, rng: ptr<function, u32>) -> vec2<f32> {
    let ab = 2.0 * rand_vec2f(rng) - 1.0;
    let a = ab.x;
    var b = ab.y;
    if (b == 0.0) { b = 1.0; }

    var phi: f32;
    var r: f32;
    if (a * a > b * b) {
        r = disk_radius * a;
        phi = (PI / 4.0) * (b / a);
    } else {
        r = disk_radius * b;
        phi = (PI / 2.0) - (PI / 4.0) * (a / b);
    }

    let x = r * cos(phi);
    let y = r * sin(phi);
    return vec2(x, y);
}

fn sample_cube_dir(uv: vec2f, face: u32) -> vec3f {

    let uvc = 2.0 * uv - 1.0;

    var dir: vec3f;
    switch(face) {
        case 0u: { dir = vec3f( 1.0,  -uvc.y, -uvc.x); }
        case 1u: { dir = vec3f(-1.0,  -uvc.y,  uvc.x); }
        case 2u: { dir = vec3f( uvc.x,  1.0,   uvc.y); }
        case 3u: { dir = vec3f( uvc.x, -1.0,  -uvc.y); }
        case 4u: { dir = vec3f( uvc.x, -uvc.y,  1.0);  }
        case 5u: { dir = vec3f(-uvc.x, -uvc.y, -1.0);  }
        default: { dir = vec3f(0.0); }
    }
    return normalize(dir);
}

struct CubeUV {
    uv: vec2f,
    face: u32,
}
fn dir_to_cube_uv(dir: vec3f) -> CubeUV {
    let abs_dir = abs(dir);
    var face: u32 = 0u;
    var uv: vec2f = vec2f(0.0);

    if (abs_dir.x >= abs_dir.y && abs_dir.x >= abs_dir.z) {

        if (dir.x > 0.0) {
            face = 0u;
            uv = vec2f(-dir.z, -dir.y) / dir.x;
        } else {
            face = 1u;
            uv = vec2f(dir.z, -dir.y) / abs_dir.x;
        }
    } else if (abs_dir.y >= abs_dir.x && abs_dir.y >= abs_dir.z) {

        if (dir.y > 0.0) {
            face = 2u;
            uv = vec2f(dir.x, dir.z) / dir.y;
        } else {
            face = 3u;
            uv = vec2f(dir.x, -dir.z) / abs_dir.y;
        }
    } else {

        if (dir.z > 0.0) {
            face = 4u;
            uv = vec2f(dir.x, -dir.y) / dir.z;
        } else {
            face = 5u;
            uv = vec2f(-dir.x, -dir.y) / abs_dir.z;
        }
    }

    return CubeUV(uv * 0.5 + 0.5, face);
}

fn porter_duff_over(bg: vec4<f32>, fg: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(mix(bg.rgb * bg.a, fg.rgb, fg.a), bg.a + fg.a * (1.0 - bg.a));
}
