

#define_import_path bevy_pbr::raymarch

#import bevy_pbr::mesh_view_bindings::depth_prepass_texture
#import bevy_pbr::view_transformations::{
    direction_world_to_clip,
    ndc_to_uv,
    perspective_camera_near,
    position_world_to_ndc,
}

#ifdef USE_DEPTH_SAMPLERS

@group(2) @binding(2) var depth_linear_sampler: sampler;

@group(2) @binding(3) var depth_nearest_sampler: sampler;
#endif

#ifndef USE_DEPTH_SAMPLERS
fn depth_texel_clamped(texel: vec2<i32>) -> f32 {
    let dims = textureDimensions(depth_prepass_texture);
    let max_coord = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let clamped = clamp(texel, vec2<i32>(0), max_coord);
    return textureLoad(depth_prepass_texture, clamped, 0);
}

fn depth_sample_nearest_clamped(uv: vec2<f32>, tex_size: vec2<f32>) -> f32 {

    let coord = uv * tex_size - vec2(0.5);
    return depth_texel_clamped(vec2<i32>(floor(coord + vec2(0.5))));
}

fn depth_sample_bilinear_clamped(uv: vec2<f32>, tex_size: vec2<f32>) -> f32 {
    let coord = uv * tex_size - vec2(0.5);
    let base = vec2<i32>(floor(coord));
    let frac = coord - floor(coord);

    let d00 = depth_texel_clamped(base);
    let d10 = depth_texel_clamped(base + vec2(1, 0));
    let d01 = depth_texel_clamped(base + vec2(0, 1));
    let d11 = depth_texel_clamped(base + vec2(1, 1));

    let d0 = mix(d00, d10, frac.x);
    let d1 = mix(d01, d11, frac.x);
    return mix(d0, d1, frac.y);
}
#endif

fn depth_sample_linear(uv: vec2<f32>, tex_size: vec2<f32>) -> f32 {
#ifdef USE_DEPTH_SAMPLERS
    return textureSampleLevel(depth_prepass_texture, depth_linear_sampler, uv, 0u);
#else
    return depth_sample_bilinear_clamped(uv, tex_size);
#endif
}

fn depth_sample_nearest(uv: vec2<f32>, tex_size: vec2<f32>) -> f32 {
#ifdef USE_DEPTH_SAMPLERS
    return textureSampleLevel(depth_prepass_texture, depth_nearest_sampler, uv, 0u);
#else
    return depth_sample_nearest_clamped(uv, tex_size);
#endif
}

struct HybridRootFinder {
    linear_steps: u32,
    bisection_steps: u32,
    use_secant: bool,
    linear_march_exponent: f32,

    jitter: f32,
    min_t: f32,
    max_t: f32,
}

fn hybrid_root_finder_new_with_linear_steps(v: u32) -> HybridRootFinder {
    var res: HybridRootFinder;
    res.linear_steps = v;
    res.bisection_steps = 0u;
    res.use_secant = false;
    res.linear_march_exponent = 1.0;
    res.jitter = 1.0;
    res.min_t = 0.0;
    res.max_t = 1.0;
    return res;
}

fn hybrid_root_finder_find_root(
    root_finder: ptr<function, HybridRootFinder>,
    start: vec3<f32>,
    end: vec3<f32>,
    distance_fn: ptr<function, DepthRaymarchDistanceFn>,
    hit_t: ptr<function, f32>,
    miss_t: ptr<function, f32>,
    hit_d: ptr<function, DistanceWithPenetration>,
) -> bool {
    let dir = end - start;

    var min_t = (*root_finder).min_t;
    var max_t = (*root_finder).max_t;

    var min_d = DistanceWithPenetration(0.0, false, 0.0);
    var max_d = DistanceWithPenetration(0.0, false, 0.0);

    let step_size = (max_t - min_t) / f32((*root_finder).linear_steps);

    var intersected = false;

    if ((*root_finder).linear_steps > 0u) {
        let candidate_t = mix(
            min_t,
            max_t,
            pow(
                (*root_finder).jitter / f32((*root_finder).linear_steps),
                (*root_finder).linear_march_exponent
            )
        );

        let candidate = start + dir * candidate_t;
        let candidate_d = depth_raymarch_distance_fn_evaluate(distance_fn, candidate);
        intersected = candidate_d.distance < 0.0 && candidate_d.valid;

        if (intersected) {
            max_t = candidate_t;
            max_d = candidate_d;

        } else {

            min_t = candidate_t;
            min_d = candidate_d;

            for (var step = 1u; step < (*root_finder).linear_steps; step += 1u) {
                let candidate_t = mix(
                    (*root_finder).min_t,
                    (*root_finder).max_t,
                    pow(
                        (f32(step) + (*root_finder).jitter) / f32((*root_finder).linear_steps),
                        (*root_finder).linear_march_exponent
                    )
                );

                let candidate = start + dir * candidate_t;
                let candidate_d = depth_raymarch_distance_fn_evaluate(distance_fn, candidate);
                intersected = candidate_d.distance < 0.0 && candidate_d.valid;

                if (intersected) {
                    max_t = candidate_t;
                    max_d = candidate_d;

                    break;
                } else {

                    min_t = candidate_t;
                    min_d = candidate_d;
                }
            }
        }
    }

    *miss_t = min_t;
    *hit_t = min_t;

    if (intersected) {
        for (var step = 0u; step < (*root_finder).bisection_steps; step += 1u) {
            let mid_t = (min_t + max_t) * 0.5;
            let candidate = start + dir * mid_t;
            let candidate_d = depth_raymarch_distance_fn_evaluate(distance_fn, candidate);

            if (candidate_d.distance < 0.0 && candidate_d.valid) {

                max_t = mid_t;
                max_d = candidate_d;
            } else {

                min_t = mid_t;
                min_d = candidate_d;
            }
        }

        if ((*root_finder).use_secant) {

            let total_d = min_d.distance + -max_d.distance;

            let mid_t = mix(min_t, max_t, min_d.distance / total_d);
            let candidate = start + dir * mid_t;
            let candidate_d = depth_raymarch_distance_fn_evaluate(distance_fn, candidate);

            if (abs(candidate_d.distance) < min_d.distance * 0.9 && candidate_d.valid) {
                *hit_t = mid_t;
                *hit_d = candidate_d;
            } else {
                *hit_t = max_t;
                *hit_d = max_d;
            }

            return true;
        } else {
            *hit_t = max_t;
            *hit_d = max_d;
            return true;
        }
    } else {

        *hit_t = min_t;
        return false;
    }
}

struct DistanceWithPenetration {

    distance: f32,

    valid: bool,

    penetration: f32,
}

struct DepthRaymarchDistanceFn {
    depth_tex_size: vec2<f32>,

    march_behind_surfaces: bool,
    depth_thickness: f32,

    use_sloppy_march: bool,
}

fn depth_raymarch_distance_fn_evaluate(
    distance_fn: ptr<function, DepthRaymarchDistanceFn>,
    ray_point_cs: vec3<f32>,
) -> DistanceWithPenetration {
    let interp_uv = ndc_to_uv(ray_point_cs.xy);

    let ray_depth = 1.0 / ray_point_cs.z;

    let linear_depth = 1.0 / depth_sample_linear(interp_uv, (*distance_fn).depth_tex_size);
    let unfiltered_depth = 1.0 / depth_sample_nearest(interp_uv, (*distance_fn).depth_tex_size);

    var max_depth: f32;
    var min_depth: f32;

    if ((*distance_fn).use_sloppy_march) {
        max_depth = unfiltered_depth;
        min_depth = unfiltered_depth;
    } else {
        max_depth = max(linear_depth, unfiltered_depth);
        min_depth = min(linear_depth, unfiltered_depth);
    }

    let bias = 0.000002;

    var res: DistanceWithPenetration;
    res.distance = max_depth * (1.0 + bias) - ray_depth;

    res.penetration = ray_depth - min_depth;

    if ((*distance_fn).march_behind_surfaces) {
        res.valid = res.penetration < (*distance_fn).depth_thickness;
    } else {
        res.valid = true;
    }

    return res;
}

struct DepthRayMarchResult {

    hit: bool,

    hit_t: f32,

    hit_uv: vec2<f32>,

    hit_penetration: f32,

    hit_penetration_frac: f32,
}

struct DepthRayMarch {

    linear_steps: u32,

    linear_march_exponent: f32,

    bisection_steps: u32,

    use_secant: bool,

    jitter: f32,

    ray_start_cs: vec3<f32>,
    ray_end_cs: vec3<f32>,

    march_behind_surfaces: bool,

    use_sloppy_march: bool,

    depth_thickness_linear_z: f32,

    depth_tex_size: vec2<f32>,
}

fn depth_ray_march_new_from_depth(depth_tex_size: vec2<f32>) -> DepthRayMarch {
    var res: DepthRayMarch;
    res.jitter = 1.0;
    res.linear_steps = 4u;
    res.bisection_steps = 0u;
    res.linear_march_exponent = 1.0;
    res.depth_tex_size = depth_tex_size;
    res.depth_thickness_linear_z = 1.0;
    res.march_behind_surfaces = false;
    res.use_sloppy_march = false;
    return res;
}

fn depth_ray_march_to_cs_dir_impl(
    raymarch: ptr<function, DepthRayMarch>,
    dir_cs: vec4<f32>,
    infinite: bool,
) {
    var end_cs = vec4((*raymarch).ray_start_cs, 1.0) + dir_cs;

    end_cs /= select(-1.0, 1.0, end_cs.w >= 0.0) * max(1e-10, abs(end_cs.w));

    var delta_cs = end_cs.xyz - (*raymarch).ray_start_cs;
    let near_edge = select(vec3(-1.0, -1.0, 0.0), vec3(1.0, 1.0, 1.0), delta_cs < vec3(0.0));
    let dist_to_near_edge = (near_edge - (*raymarch).ray_start_cs) / delta_cs;
    let max_dist_to_near_edge = max(dist_to_near_edge.x, dist_to_near_edge.y);
    (*raymarch).ray_start_cs += delta_cs * max(0.0, max_dist_to_near_edge);

    delta_cs = end_cs.xyz - (*raymarch).ray_start_cs;
    let far_edge = select(vec3(-1.0, -1.0, 0.0), vec3(1.0, 1.0, 1.0), delta_cs >= vec3(0.0));
    let dist_to_far_edge = (far_edge - (*raymarch).ray_start_cs) / delta_cs;
    let min_dist_to_far_edge = min(
        min(dist_to_far_edge.x, dist_to_far_edge.y),
        dist_to_far_edge.z
    );

    if (infinite) {
        delta_cs *= min_dist_to_far_edge;
    } else {

        delta_cs *= min(1.0, min_dist_to_far_edge);
    }

    (*raymarch).ray_end_cs = (*raymarch).ray_start_cs + delta_cs;
}

fn depth_ray_march_from_cs(raymarch: ptr<function, DepthRayMarch>, v: vec3<f32>) {
    (*raymarch).ray_start_cs = v;
}

fn depth_ray_march_to_cs(raymarch: ptr<function, DepthRayMarch>, end_cs: vec3<f32>) {
    let dir = vec4(end_cs - (*raymarch).ray_start_cs, 0.0) * sign(end_cs.z);
    depth_ray_march_to_cs_dir_impl(raymarch, dir, false);
}

fn depth_ray_march_to_cs_dir(raymarch: ptr<function, DepthRayMarch>, dir: vec4<f32>) {
    depth_ray_march_to_cs_dir_impl(raymarch, dir, true);
}

fn depth_ray_march_to_ws(raymarch: ptr<function, DepthRayMarch>, end: vec3<f32>) {
    depth_ray_march_to_cs(raymarch, position_world_to_ndc(end));
}

fn depth_ray_march_to_ws_dir(raymarch: ptr<function, DepthRayMarch>, dir: vec3<f32>) {
    depth_ray_march_to_cs_dir_impl(raymarch, direction_world_to_clip(dir), true);
}

fn depth_ray_march_march(raymarch: ptr<function, DepthRayMarch>) -> DepthRayMarchResult {
    var res = DepthRayMarchResult(false, 0.0, vec2(0.0), 0.0, 0.0);

    let ray_start_uv = ndc_to_uv((*raymarch).ray_start_cs.xy);
    let ray_end_uv = ndc_to_uv((*raymarch).ray_end_cs.xy);

    let ray_uv_delta = ray_end_uv - ray_start_uv;
    let ray_len_px = ray_uv_delta * (*raymarch).depth_tex_size;

    let min_px_per_step = 1u;
    let step_count = max(
        2,
        min(i32((*raymarch).linear_steps), i32(floor(length(ray_len_px) / f32(min_px_per_step))))
    );

    let linear_z_to_scaled_linear_z = 1.0 / perspective_camera_near();
    let depth_thickness = (*raymarch).depth_thickness_linear_z * linear_z_to_scaled_linear_z;

    var distance_fn: DepthRaymarchDistanceFn;
    distance_fn.depth_tex_size = (*raymarch).depth_tex_size;
    distance_fn.march_behind_surfaces = (*raymarch).march_behind_surfaces;
    distance_fn.depth_thickness = depth_thickness;
    distance_fn.use_sloppy_march = (*raymarch).use_sloppy_march;

    var hit: DistanceWithPenetration;

    var hit_t = 0.0;
    var miss_t = 0.0;
    var root_finder = hybrid_root_finder_new_with_linear_steps(u32(step_count));
    root_finder.bisection_steps = (*raymarch).bisection_steps;
    root_finder.use_secant = (*raymarch).use_secant;
    root_finder.linear_march_exponent = (*raymarch).linear_march_exponent;
    root_finder.jitter = (*raymarch).jitter;
    let intersected = hybrid_root_finder_find_root(
        &root_finder,
        (*raymarch).ray_start_cs,
        (*raymarch).ray_end_cs,
        &distance_fn,
        &hit_t,
        &miss_t,
        &hit
    );

    res.hit_t = hit_t;

    if (intersected && hit.penetration < depth_thickness && hit.distance < depth_thickness) {
        res.hit = true;
        res.hit_uv = mix(ray_start_uv, ray_end_uv, res.hit_t);
        res.hit_penetration = hit.penetration / linear_z_to_scaled_linear_z;
        res.hit_penetration_frac = hit.penetration / depth_thickness;
        return res;
    }

    res.hit_t = miss_t;
    res.hit_uv = mix(ray_start_uv, ray_end_uv, res.hit_t);

    return res;
}
