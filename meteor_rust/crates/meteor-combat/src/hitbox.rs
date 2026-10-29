use meteor_core::Vec3;

pub const BODY_RADIUS: f32 = 0.5;

pub const BODY_BOTTOM: f32 = 0.1;

pub const BODY_TOP: f32 = 4.0;

pub const BLADE_EDGE_RADIUS: f32 = 0.06;

pub const FIST_RADIUS: f32 = 0.12;

pub const SECONDARY_BONE_WEIGHT: f32 = 0.3;

pub fn segment_segment_distance_sq(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> f32 {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(&d1);
    let e = d2.dot(&d2);
    let f = d2.dot(&r);
    let eps = 1e-9;
    let (mut s, mut t);
    if a <= eps && e <= eps {
        return r.dot(&r);
    }
    if a <= eps {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(&r);
        if e <= eps {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(&d2);
            let denom = a * e - b * b;
            if denom > eps {
                s = ((b * f - c * e) / denom).clamp(0.0, 1.0);
            } else {
                s = 0.0;
            }
            t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
        }
    }
    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    let d = c1 - c2;
    d.dot(&d)
}

#[derive(Debug, Clone, Copy)]
pub struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub r: f32,
}

pub fn capsules_intersect(cap_a: &Capsule, cap_b: &Capsule) -> bool {
    let rr = cap_a.r + cap_b.r;
    segment_segment_distance_sq(cap_a.a, cap_a.b, cap_b.a, cap_b.b) <= rr * rr
}

#[derive(Debug, Clone)]
pub struct Obb {
    pub center: Vec3,

    pub axes: [Vec3; 3],
    pub half_size: Vec3,
}

fn dot(a: &Vec3, b: &Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

pub fn obbs_intersect(box_a: &Obb, box_b: &Obb) -> bool {
    let a = [box_a.half_size.x, box_a.half_size.y, box_a.half_size.z];
    let b = [box_b.half_size.x, box_b.half_size.y, box_b.half_size.z];
    if a.iter().chain(b.iter()).any(|v| !v.is_finite() || *v < 0.0) {
        return false;
    }

    let mut rotation = [[0.0f32; 3]; 3];
    let mut abs_rotation = [[0.0f32; 3]; 3];
    let epsilon = 1e-6;
    for i in 0..3 {
        for j in 0..3 {
            rotation[i][j] = dot(&box_a.axes[i], &box_b.axes[j]);
            abs_rotation[i][j] = rotation[i][j].abs() + epsilon;
        }
    }

    let center_delta = box_b.center - box_a.center;

    let translation = [
        dot(&center_delta, &box_a.axes[0]),
        dot(&center_delta, &box_a.axes[1]),
        dot(&center_delta, &box_a.axes[2]),
    ];

    for i in 0..3 {
        let rb = b[0] * abs_rotation[i][0] + b[1] * abs_rotation[i][1] + b[2] * abs_rotation[i][2];
        if translation[i].abs() > a[i] + rb {
            return false;
        }
    }

    for j in 0..3 {
        let ra = a[0] * abs_rotation[0][j] + a[1] * abs_rotation[1][j] + a[2] * abs_rotation[2][j];
        let projected = (translation[0] * rotation[0][j]
            + translation[1] * rotation[1][j]
            + translation[2] * rotation[2][j])
            .abs();
        if projected > ra + b[j] {
            return false;
        }
    }

    for i in 0..3 {
        let i1 = (i + 1) % 3;
        let i2 = (i + 2) % 3;
        for j in 0..3 {
            let j1 = (j + 1) % 3;
            let j2 = (j + 2) % 3;
            let ra = a[i1] * abs_rotation[i2][j] + a[i2] * abs_rotation[i1][j];
            let rb = b[j1] * abs_rotation[i][j2] + b[j2] * abs_rotation[i][j1];
            let projected =
                (translation[i2] * rotation[i1][j] - translation[i1] * rotation[i2][j]).abs();
            if projected > ra + rb {
                return false;
            }
        }
    }

    true
}

#[derive(Debug, Clone, Copy)]
pub struct BoneLocalBox {
    pub center: Vec3,
    pub half_size: Vec3,
}

#[derive(Debug, Clone)]
pub struct BoneInfluence {
    pub bone: u32,
    pub weight: f32,
}

pub fn build_bone_local_bounds(
    positions: &[Vec3],
    influences: &[Vec<BoneInfluence>],
) -> Vec<Option<BoneLocalBox>> {
    let mut mins: Vec<Vec3> = Vec::new();
    let mut maxs: Vec<Vec3> = Vec::new();
    let inf = f32::INFINITY;

    for (vertex, point) in positions.iter().enumerate() {
        let refs = influences.get(vertex).cloned().unwrap_or_default();
        for (slot, ref_) in refs.iter().enumerate() {
            if slot > 0
                && ref_.weight.partial_cmp(&SECONDARY_BONE_WEIGHT)
                    != Some(std::cmp::Ordering::Greater)
            {
                continue;
            }
            let bone_index = ref_.bone as usize;
            while mins.len() <= bone_index {
                mins.push(Vec3::new(inf, inf, inf));
                maxs.push(Vec3::new(-inf, -inf, -inf));
            }
            mins[bone_index].x = mins[bone_index].x.min(point.x);
            mins[bone_index].y = mins[bone_index].y.min(point.y);
            mins[bone_index].z = mins[bone_index].z.min(point.z);
            maxs[bone_index].x = maxs[bone_index].x.max(point.x);
            maxs[bone_index].y = maxs[bone_index].y.max(point.y);
            maxs[bone_index].z = maxs[bone_index].z.max(point.z);
        }
    }

    mins.into_iter()
        .zip(maxs)
        .map(|(mn, mx)| {
            if !mn.x.is_finite() {
                return None;
            }
            let half = (mx - mn) * 0.5;
            Some(BoneLocalBox {
                center: mn + half,
                half_size: half,
            })
        })
        .collect()
}

pub fn normalized_bone_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

pub fn is_hurt_bone_name(name: &str) -> bool {
    name.as_bytes().get(1) == Some(&b'a')
}

pub fn bone_world_obb(bone_world_matrix: &meteor_core::Mat4, local: &BoneLocalBox) -> Option<Obb> {
    let m = &bone_world_matrix.m;
    let axis_x = Vec3::new(m[0], m[1], m[2]);
    let axis_y = Vec3::new(m[4], m[5], m[6]);
    let axis_z = Vec3::new(m[8], m[9], m[10]);
    let scale_x = axis_x.length();
    let scale_y = axis_y.length();
    let scale_z = axis_z.length();
    if scale_x <= 1e-8 || scale_y <= 1e-8 || scale_z <= 1e-8 {
        return None;
    }
    let axes = [
        Vec3::new(axis_x.x / scale_x, axis_x.y / scale_x, axis_x.z / scale_x),
        Vec3::new(axis_y.x / scale_y, axis_y.y / scale_y, axis_y.z / scale_y),
        Vec3::new(axis_z.x / scale_z, axis_z.y / scale_z, axis_z.z / scale_z),
    ];
    let center = bone_world_matrix.transform_point(local.center);
    let half_size = Vec3::new(
        local.half_size.x * scale_x,
        local.half_size.y * scale_y,
        local.half_size.z * scale_z,
    );
    Some(Obb {
        center,
        axes,
        half_size,
    })
}

#[derive(Debug, Clone)]
pub struct WeaponCollisionPart {
    pub world_matrix: meteor_core::Mat4,
    pub boxes: Vec<BoneLocalBox>,
}

pub fn attack_hit_obbs(
    bone_names: &[String],
    weapon_parts: &[WeaponCollisionPart],
    mut bone_resolver: impl FnMut(&str) -> Option<(meteor_core::Mat4, BoneLocalBox)>,
) -> Vec<Obb> {
    let owned_default: Vec<String>;
    let names: &[String] = if bone_names.is_empty() {
        owned_default = vec!["weapon".to_string()];
        &owned_default
    } else {
        bone_names
    };
    let mut result = Vec::new();
    let mut weapon_added = false;
    let mut seen_bones = std::collections::HashSet::new();
    for raw in names {
        let name = raw.to_lowercase();
        if name == "effect" {
            continue;
        }
        if name == "weapon" {
            if !weapon_added {
                for part in weapon_parts {
                    for local in &part.boxes {
                        if let Some(obb) = bone_world_obb(&part.world_matrix, local) {
                            result.push(obb);
                        }
                    }
                }
            }
            weapon_added = true;
            continue;
        }

        let normalized = normalized_bone_name(&name);
        if normalized.is_empty() || !seen_bones.insert(normalized.clone()) {
            continue;
        }
        if let Some((wm, local)) = bone_resolver(&normalized) {
            if let Some(obb) = bone_world_obb(&wm, &local) {
                result.push(obb);
            }
        }
    }
    result
}

pub fn body_hurt_obbs<'a>(
    bones: impl Iterator<Item = (&'a str, meteor_core::Mat4, Option<BoneLocalBox>)>,
) -> Vec<Obb> {
    let mut result = Vec::new();
    for (name, world_matrix, local) in bones {
        if !is_hurt_bone_name(name) {
            continue;
        }
        let Some(local) = local else {
            continue;
        };
        if let Some(obb) = bone_world_obb(&world_matrix, &local) {
            result.push(obb);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance_parallel() {
        let d = segment_segment_distance_sq(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        );
        assert!((d - 1.0).abs() < 1e-5);
    }

    #[test]
    fn segment_distance_intersecting() {
        let d = segment_segment_distance_sq(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.5, -1.0, 0.0),
            Vec3::new(0.5, 1.0, 0.0),
        );
        assert!(d < 1e-5);
    }

    #[test]
    fn capsule_intersect() {
        let a = Capsule {
            a: Vec3::ZERO,
            b: Vec3::new(0.0, 0.0, 1.0),
            r: 0.5,
        };
        let b = Capsule {
            a: Vec3::new(0.8, 0.0, 0.5),
            b: Vec3::new(1.8, 0.0, 0.5),
            r: 0.5,
        };

        assert!(capsules_intersect(&a, &b));
        let far = Capsule {
            a: Vec3::new(5.0, 0.0, 0.5),
            b: Vec3::new(6.0, 0.0, 0.5),
            r: 0.5,
        };
        assert!(!capsules_intersect(&a, &far));
    }

    #[test]
    fn obb_overlap_and_separated() {
        let box_a = Obb {
            center: Vec3::ZERO,
            axes: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        let box_b = Obb {
            center: Vec3::new(0.5, 0.0, 0.0),
            axes: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        assert!(obbs_intersect(&box_a, &box_b));

        let far_b = Obb {
            center: Vec3::new(3.1, 0.0, 0.0),
            axes: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        assert!(!obbs_intersect(&box_a, &far_b));
    }

    #[test]
    fn obb_rotated_overlap() {
        let box_a = Obb {
            center: Vec3::ZERO,
            axes: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let box_b = Obb {
            center: Vec3::ZERO,
            axes: [
                Vec3::new(s, s, 0.0),
                Vec3::new(-s, s, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        assert!(obbs_intersect(&box_a, &box_b));
    }

    #[test]
    fn bone_bounds_primary_only() {
        let positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)];
        let influences = vec![
            vec![BoneInfluence {
                bone: 0,
                weight: 1.0,
            }],
            vec![BoneInfluence {
                bone: 0,
                weight: 1.0,
            }],
        ];
        let bounds = build_bone_local_bounds(&positions, &influences);
        assert_eq!(bounds.len(), 1);
        let box_ = bounds[0].as_ref().unwrap();
        assert!((box_.center.x - 1.0).abs() < 1e-5);
        assert!((box_.half_size.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bone_bounds_secondary_included_when_heavy() {
        let positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 4.0, 0.0)];
        let influences = vec![
            vec![BoneInfluence {
                bone: 0,
                weight: 1.0,
            }],
            vec![BoneInfluence {
                bone: 1,
                weight: 0.5,
            }],
        ];
        let bounds = build_bone_local_bounds(&positions, &influences);
        assert_eq!(bounds.len(), 2);
        assert!(bounds[1].as_ref().unwrap().center.y > 1.0);
    }

    #[test]
    fn bone_name_normalization() {
        assert_eq!(normalized_bone_name("bad L Thigh"), "badlthigh");
        assert_eq!(normalized_bone_name("bad_L_Thigh"), "badlthigh");
        assert_eq!(normalized_bone_name(""), "");
    }

    #[test]
    fn hurt_bone_name_matches_bad_prefix() {
        assert!(is_hurt_bone_name("bad_L_Thigh"));
        assert!(is_hurt_bone_name("bau_Head"));
    }

    #[test]
    fn hurt_bone_name_excludes_bbd_toe_helpers() {
        assert!(!is_hurt_bone_name("bbd_L_Toe"));
    }

    #[test]
    fn hurt_bone_name_handles_short_names() {
        assert!(!is_hurt_bone_name(""));
        assert!(!is_hurt_bone_name("b"));
    }

    #[test]
    fn bone_world_obb_identity_matrix_passthrough() {
        let local = BoneLocalBox {
            center: Vec3::new(1.0, 2.0, 3.0),
            half_size: Vec3::new(0.5, 0.6, 0.7),
        };
        let obb = bone_world_obb(&meteor_core::Mat4::IDENTITY, &local).unwrap();
        assert!((obb.center.x - 1.0).abs() < 1e-5);
        assert!((obb.center.y - 2.0).abs() < 1e-5);
        assert!((obb.center.z - 3.0).abs() < 1e-5);
        assert!((obb.half_size.x - 0.5).abs() < 1e-5);
        assert!((obb.half_size.y - 0.6).abs() < 1e-5);
        assert!((obb.half_size.z - 0.7).abs() < 1e-5);

        assert!((obb.axes[0].x - 1.0).abs() < 1e-5);
        assert!((obb.axes[1].y - 1.0).abs() < 1e-5);
        assert!((obb.axes[2].z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bone_world_obb_translation_moves_center_only() {
        let local = BoneLocalBox {
            center: Vec3::ZERO,
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        let m = meteor_core::Mat4::from_translation(Vec3::new(10.0, 0.0, 5.0));
        let obb = bone_world_obb(&m, &local).unwrap();
        assert!((obb.center.x - 10.0).abs() < 1e-5);
        assert!((obb.center.z - 5.0).abs() < 1e-5);

        assert!((obb.half_size.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bone_world_obb_rotation_rotates_axes() {
        let local = BoneLocalBox {
            center: Vec3::ZERO,
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        let q = meteor_core::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let m = meteor_core::Mat4::from_quaternion(q);
        let obb = bone_world_obb(&m, &local).unwrap();

        assert!(obb.axes[0].x.abs() < 1e-4);
        assert!((obb.axes[0].y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn bone_world_obb_degenerate_scale_returns_none() {
        let local = BoneLocalBox {
            center: Vec3::ZERO,
            half_size: Vec3::new(1.0, 1.0, 1.0),
        };
        let mut m = meteor_core::Mat4::IDENTITY;
        m.m[0] = 0.0;
        m.m[1] = 0.0;
        m.m[2] = 0.0;
        assert!(bone_world_obb(&m, &local).is_none());
    }

    #[test]
    fn attack_hit_obbs_empty_bones_defaults_to_weapon() {
        let parts = vec![WeaponCollisionPart {
            world_matrix: meteor_core::Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            boxes: vec![BoneLocalBox {
                center: Vec3::ZERO,
                half_size: Vec3::new(0.08, 0.08, 0.5),
            }],
        }];
        let obbs = attack_hit_obbs(&[], &parts, |_| None);
        assert_eq!(obbs.len(), 1);
        assert!((obbs[0].center.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn attack_hit_obbs_weapon_added_only_once() {
        let parts = vec![WeaponCollisionPart {
            world_matrix: meteor_core::Mat4::IDENTITY,
            boxes: vec![BoneLocalBox {
                center: Vec3::ZERO,
                half_size: Vec3::new(0.1, 0.2, 0.3),
            }],
        }];
        let names = vec!["weapon".to_string(), "weapon".to_string()];
        let obbs = attack_hit_obbs(&names, &parts, |_| None);
        assert_eq!(obbs.len(), 1);
    }

    #[test]
    fn attack_hit_obbs_skips_effect_bone() {
        let names = vec!["effect".to_string()];
        let obbs = attack_hit_obbs(&names, &[], |_| None);
        assert!(obbs.is_empty());
    }

    #[test]
    fn attack_hit_obbs_named_bone_uses_resolver() {
        let names = vec!["bau R Hand".to_string()];
        let local = BoneLocalBox {
            center: Vec3::ZERO,
            half_size: Vec3::new(0.1, 0.1, 0.1),
        };
        let obbs = attack_hit_obbs(&names, &[], |name| {
            (name == "baurhand").then_some((meteor_core::Mat4::IDENTITY, local))
        });
        assert_eq!(obbs.len(), 1);
    }

    #[test]
    fn attack_hit_obbs_normalizes_pos_and_bnc_names_and_deduplicates_aliases() {
        let names = vec![
            "bad L Thigh".to_string(),
            "bad_L_Thigh".to_string(),
            "BAD-L-THIGH".to_string(),
        ];
        let local = BoneLocalBox {
            center: Vec3::new(1.0, 2.0, 3.0),
            half_size: Vec3::new(0.2, 0.3, 0.4),
        };
        let mut calls = Vec::new();
        let obbs = attack_hit_obbs(&names, &[], |name| {
            calls.push(name.to_string());
            (name == "badlthigh").then_some((meteor_core::Mat4::IDENTITY, local))
        });
        assert_eq!(calls, ["badlthigh"]);
        assert_eq!(obbs.len(), 1);
        assert!((obbs[0].center.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn attack_hit_obbs_missing_weapon_geometry_yields_empty() {
        let names = vec!["weapon".to_string()];
        let obbs = attack_hit_obbs(&names, &[], |_| None);
        assert!(obbs.is_empty());
    }

    #[test]
    fn body_hurt_obbs_filters_by_hurt_bone_name() {
        let local = BoneLocalBox {
            center: Vec3::ZERO,
            half_size: Vec3::new(0.2, 0.2, 0.2),
        };
        let bones = vec![
            ("bad_L_Thigh", meteor_core::Mat4::IDENTITY, Some(local)),
            ("bbd_L_Toe", meteor_core::Mat4::IDENTITY, Some(local)),
            ("d_wpnR", meteor_core::Mat4::IDENTITY, Some(local)),
        ];
        let obbs = body_hurt_obbs(bones.into_iter());
        assert_eq!(obbs.len(), 1);
    }

    #[test]
    fn body_hurt_obbs_skips_missing_local_box() {
        let bones = vec![("bad_L_Thigh", meteor_core::Mat4::IDENTITY, None)];
        let obbs = body_hurt_obbs(bones.into_iter());
        assert!(obbs.is_empty());
    }
}
