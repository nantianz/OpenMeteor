

#define_import_path bevy_pbr::atmosphere::bruneton_functions

#import bevy_pbr::atmosphere::{
    types::Atmosphere,
    bindings::atmosphere,
}

fn transmittance_lut_r_mu_to_uv(atm: Atmosphere, r: f32, mu: f32) -> vec2<f32> {

    let H = sqrt(atm.outer_radius * atm.outer_radius - atm.inner_radius * atm.inner_radius);

    let rho = sqrt(max(r * r - atm.inner_radius * atm.inner_radius, 0.0));

    let d = distance_to_top_atmosphere_boundary(atm, r, mu);

    let d_min = atm.outer_radius - r;
    let d_max = rho + H;

    let u = (d - d_min) / (d_max - d_min);
    let v = rho / H;
    return vec2<f32>(u, v);
}

fn transmittance_lut_uv_to_r_mu(uv: vec2<f32>) -> vec2<f32> {

    let H = sqrt(atmosphere.outer_radius * atmosphere.outer_radius - atmosphere.inner_radius * atmosphere.inner_radius);

    let rho = H * uv.y;
    let r = sqrt(rho * rho + atmosphere.inner_radius * atmosphere.inner_radius);

    let d_min = atmosphere.outer_radius - r;
    let d_max = rho + H;
    let d = d_min + uv.x * (d_max - d_min);

    var mu: f32;
    if d == 0.0 {
        mu = 1.0;
    } else {
        mu = (H * H - rho * rho - d * d) / (2.0 * r * d);
    }

    mu = clamp(mu, -1.0, 1.0);

    return vec2<f32>(r, mu);
}

fn distance_to_top_atmosphere_boundary(atm: Atmosphere, r: f32, mu: f32) -> f32 {

    let positive_discriminant = max(r * r * (mu * mu - 1.0) + atm.outer_radius * atm.outer_radius, 0.0);
    return max(-r * mu + sqrt(positive_discriminant), 0.0);
}

fn distance_to_bottom_atmosphere_boundary(r: f32, mu: f32) -> f32 {
    let positive_discriminant = max(r * r * (mu * mu - 1.0) + atmosphere.inner_radius * atmosphere.inner_radius, 0.0);
    return max(-r * mu - sqrt(positive_discriminant), 0.0);
}

fn ray_intersects_ground(r: f32, mu: f32) -> bool {
    return mu < 0.0 && r * r * (mu * mu - 1.0) + atmosphere.inner_radius * atmosphere.inner_radius >= 0.0;
}
