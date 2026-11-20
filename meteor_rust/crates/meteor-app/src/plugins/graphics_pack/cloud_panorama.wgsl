#define_import_path meteor::cloud_panorama

const PI: f32 = 3.14159265359;
const HORIZON_UV: f32 = 1.0 / 3.0;

struct CloudCacheView {
    origin_time: vec4<f32>,
    previous_origin_time: vec4<f32>,
    state: vec4<u32>,
    drift: vec4<f32>,
    previous_drift: vec4<f32>,
    horizon: vec4<f32>,
    zenith: vec4<f32>,
    layer: vec4<f32>,
}

fn panorama_uv(anchor: vec3<f32>, ray: vec3<f32>, origin: vec3<f32>, drift: vec2<f32>,
               layer: vec2<f32>) -> vec3<f32> {
    let center = layer.x + layer.y * 0.5;
    var relative = ray;
    if (abs(ray.y) > 0.002) {
        let distance = (center - origin.y) / ray.y;
        if (distance > 0.0 && distance < 2200.0 && (center - anchor.y) * ray.y > 0.0) {
            let direction = origin + ray * distance - vec3<f32>(drift.x, 0.0, drift.y) - anchor;
            if (dot(direction, direction) > 0.001) { relative = normalize(direction); }
        }
    }
    if ((anchor.y < layer.x && relative.y <= 0.0)
        || (anchor.y > layer.x + layer.y && relative.y >= 0.0)) {
        return vec3<f32>(0.0);
    }
    let elevation = asin(clamp(relative.y, -1.0, 1.0));
    return vec3<f32>(atan2(relative.z, relative.x) / (2.0 * PI) + 0.5,
        HORIZON_UV + elevation * (2.0 / PI)
            * select(HORIZON_UV, 1.0 - HORIZON_UV, elevation >= 0.0), 1.0);
}
