use crate::role_state::RoleVec3;

pub const ESCORT_STAGE_ID: &str = "sn03";
pub const STOCK_ESCORT_CATEGORY: u32 = 4;
pub const STOCK_ESCORT_ITEM_ID: u32 = 2;

pub const STOCK_ESCORT_SCENE_OBJECT_ID: i32 = 215;
const CHARACTER_RADIUS: f32 = 0.96;
const CHARACTER_HEIGHT: f32 = 3.84;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EscortStage {
    pub flag_home: RoleVec3,
    pub safe_zones: [SafeZone; 2],
    pub return_seconds: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SafeZone {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub min_z: f32,
    pub max_z: f32,
    pub center_x: f32,
    pub center_y: f32,
    pub center_z: f32,
}

pub const SN03: EscortStage = EscortStage {
    flag_home: RoleVec3 {
        x: 26.937_681,
        y: 0.591_84,
        z: -155.950_84,
    },
    safe_zones: [
        SafeZone {
            min_x: 144.403_68,
            min_y: 0.0,
            max_x: 197.876_16,
            max_y: 11.458_383,
            min_z: 136.861_08,
            max_z: 143.667_05,
            center_x: 171.139_92,
            center_y: 5.729_192,
            center_z: 140.264_07,
        },
        SafeZone {
            min_x: -202.055_33,
            min_y: 0.0,
            max_x: -148.582_86,
            max_y: 11.458_383,
            min_z: 136.561_62,
            max_z: 143.494_98,
            center_x: -175.319_09,
            center_y: 5.729_192,
            center_z: 140.028_3,
        },
    ],
    return_seconds: 5.0,
};

pub fn escort_stage(stage_id: &str) -> Option<&'static EscortStage> {
    (stage_id == ESCORT_STAGE_ID).then_some(&SN03)
}

pub fn position_inside_safe_zone(position: RoleVec3, team: i32, stage: &EscortStage) -> bool {
    let Some(zone) = matches!(team, 1 | 2).then(|| stage.safe_zones[(team - 1) as usize]) else {
        return false;
    };
    position.x >= zone.min_x
        && position.x <= zone.max_x
        && position.z >= zone.min_z
        && position.z <= zone.max_z
}

pub fn character_capsule_intersects_flag(player: RoleVec3, flag: RoleVec3) -> bool {
    capsule_intersects_triangles(
        player,
        flag,
        FLAG_INDICES.chunks_exact(3).map(|t| {
            [
                FLAG_VERTICES[t[0]],
                FLAG_VERTICES[t[1]],
                FLAG_VERTICES[t[2]],
            ]
        }),
    )
}

pub fn character_capsule_intersects_mesh(
    player: RoleVec3,
    flag: RoleVec3,
    vertices: &[[f32; 3]],
    indices: &[usize],
) -> bool {
    capsule_intersects_triangles(
        player,
        flag,
        indices.chunks_exact(3).filter_map(|t| {
            let vertex = |i: usize| {
                vertices.get(i).map(|p| Vec3 {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                })
            };
            Some([vertex(t[0])?, vertex(t[1])?, vertex(t[2])?])
        }),
    )
}

fn capsule_intersects_triangles(
    player: RoleVec3,
    flag: RoleVec3,
    triangles: impl Iterator<Item = [Vec3; 3]>,
) -> bool {
    let local_base = subtract(player, flag);
    let start = Vec3 {
        y: local_base.y + CHARACTER_RADIUS,
        ..local_base
    };
    let end = Vec3 {
        y: local_base.y + CHARACTER_HEIGHT - CHARACTER_RADIUS,
        ..local_base
    };
    let radius_squared = CHARACTER_RADIUS * CHARACTER_RADIUS;
    let mut solid_angle = 0.0_f64;
    for triangle in triangles {
        if segment_triangle_distance_squared(start, end, triangle[0], triangle[1], triangle[2])
            <= radius_squared + f32::EPSILON
        {
            return true;
        }

        let [a, b, c] = triangle.map(|v| vector_subtract(v, start));
        let a = [a.x as f64, a.y as f64, a.z as f64];
        let b = [b.x as f64, b.y as f64, b.z as f64];
        let c = [c.x as f64, c.y as f64, c.z as f64];
        let dot64 = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
        let la = dot64(a, a).sqrt();
        let lb = dot64(b, b).sqrt();
        let lc = dot64(c, c).sqrt();
        let cross = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        solid_angle += 2.0
            * dot64(a, cross)
                .atan2(la * lb * lc + dot64(a, b) * lc + dot64(b, c) * la + dot64(c, a) * lb);
    }
    solid_angle.abs() > std::f64::consts::TAU
}

#[derive(Debug, Clone, Copy)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl From<RoleVec3> for Vec3 {
    fn from(value: RoleVec3) -> Self {
        Self {
            x: value.x,
            y: value.y,
            z: value.z,
        }
    }
}

impl From<Vec3> for RoleVec3 {
    fn from(value: Vec3) -> Self {
        Self {
            x: value.x,
            y: value.y,
            z: value.z,
        }
    }
}

fn subtract(left: RoleVec3, right: RoleVec3) -> Vec3 {
    Vec3 {
        x: left.x - right.x,
        y: left.y - right.y,
        z: left.z - right.z,
    }
}

fn add_scaled(origin: Vec3, direction: Vec3, scale: f32) -> Vec3 {
    Vec3 {
        x: origin.x + direction.x * scale,
        y: origin.y + direction.y * scale,
        z: origin.z + direction.z * scale,
    }
}

fn dot(left: Vec3, right: Vec3) -> f32 {
    left.x * right.x + left.y * right.y + left.z * right.z
}

fn length_squared(value: Vec3) -> f32 {
    dot(value, value)
}

fn vector_subtract(left: Vec3, right: Vec3) -> Vec3 {
    Vec3 {
        x: left.x - right.x,
        y: left.y - right.y,
        z: left.z - right.z,
    }
}

fn point_triangle_distance_squared(point: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = vector_subtract(b, a);
    let ac = vector_subtract(c, a);
    let ap = vector_subtract(point, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return length_squared(ap);
    }
    let bp = vector_subtract(point, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return length_squared(bp);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return length_squared(vector_subtract(point, add_scaled(a, ab, d1 / (d1 - d3))));
    }
    let cp = vector_subtract(point, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return length_squared(cp);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return length_squared(vector_subtract(point, add_scaled(a, ac, d2 / (d2 - d6))));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let bc = vector_subtract(c, b);
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return length_squared(vector_subtract(point, add_scaled(b, bc, w)));
    }
    let denominator = 1.0 / (va + vb + vc);
    let closest = add_scaled(add_scaled(a, ab, vb * denominator), ac, vc * denominator);
    length_squared(vector_subtract(point, closest))
}

fn segment_segment_distance_squared(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> f32 {
    let d1 = vector_subtract(q1, p1);
    let d2 = vector_subtract(q2, p2);
    let r = vector_subtract(p1, p2);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    let (s, t) = if a <= f32::EPSILON && e <= f32::EPSILON {
        return length_squared(r);
    } else if a <= f32::EPSILON {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = dot(d1, r);
        if e <= f32::EPSILON {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = dot(d1, d2);
            let denominator = a * e - b * b;
            let mut s = if denominator.abs() > f32::EPSILON {
                ((b * f - c * e) / denominator).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    length_squared(vector_subtract(
        add_scaled(p1, d1, s),
        add_scaled(p2, d2, t),
    ))
}

fn segment_intersects_triangle(start: Vec3, end: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
    let direction = vector_subtract(end, start);
    let edge1 = vector_subtract(b, a);
    let edge2 = vector_subtract(c, a);
    let p = Vec3 {
        x: direction.y * edge2.z - direction.z * edge2.y,
        y: direction.z * edge2.x - direction.x * edge2.z,
        z: direction.x * edge2.y - direction.y * edge2.x,
    };
    let determinant = dot(edge1, p);
    if determinant.abs() <= f32::EPSILON {
        return false;
    }
    let inverse = 1.0 / determinant;
    let offset = vector_subtract(start, a);
    let u = dot(offset, p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = Vec3 {
        x: offset.y * edge1.z - offset.z * edge1.y,
        y: offset.z * edge1.x - offset.x * edge1.z,
        z: offset.x * edge1.y - offset.y * edge1.x,
    };
    let v = dot(direction, q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let t = dot(edge2, q) * inverse;
    (0.0..=1.0).contains(&t)
}

fn segment_triangle_distance_squared(start: Vec3, end: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    if segment_intersects_triangle(start, end, a, b, c) {
        return 0.0;
    }
    point_triangle_distance_squared(start, a, b, c)
        .min(point_triangle_distance_squared(end, a, b, c))
        .min(segment_segment_distance_squared(start, end, a, b))
        .min(segment_segment_distance_squared(start, end, b, c))
        .min(segment_segment_distance_squared(start, end, c, a))
}

const FLAG_VERTICES: [Vec3; 16] = [
    Vec3 {
        x: 0.632_775,
        y: 0.0,
        z: 0.593_633,
    },
    Vec3 {
        x: -0.571_161,
        y: 0.0,
        z: 0.593_633,
    },
    Vec3 {
        x: 0.632_775,
        y: 0.0,
        z: -0.610_303,
    },
    Vec3 {
        x: -0.571_161,
        y: 0.0,
        z: -0.610_303,
    },
    Vec3 {
        x: -0.571_161,
        y: 0.094_305,
        z: 0.593_633,
    },
    Vec3 {
        x: 0.578_051,
        y: 0.806_651,
        z: 0.538_908,
    },
    Vec3 {
        x: -0.516_436,
        y: 0.806_651,
        z: 0.538_908,
    },
    Vec3 {
        x: -0.516_436,
        y: 0.806_651,
        z: -0.555_579,
    },
    Vec3 {
        x: -0.247_35,
        y: 0.897_65,
        z: 0.269_822,
    },
    Vec3 {
        x: 0.632_775,
        y: 0.094_305,
        z: -0.610_303,
    },
    Vec3 {
        x: 0.578_051,
        y: 0.806_651,
        z: -0.555_579,
    },
    Vec3 {
        x: 0.308_964,
        y: 0.897_65,
        z: -0.286_492,
    },
    Vec3 {
        x: -0.571_161,
        y: 0.094_305,
        z: -0.610_303,
    },
    Vec3 {
        x: 0.632_775,
        y: 0.094_305,
        z: 0.593_633,
    },
    Vec3 {
        x: 0.308_964,
        y: 0.897_65,
        z: 0.269_822,
    },
    Vec3 {
        x: -0.247_35,
        y: 0.897_65,
        z: -0.286_492,
    },
];

const FLAG_INDICES: [usize; 84] = [
    0, 1, 2, 3, 2, 1, 4, 5, 6, 4, 6, 7, 4, 3, 1, 4, 1, 0, 8, 7, 6, 8, 6, 5, 9, 2, 3, 9, 7, 10, 9,
    10, 5, 9, 0, 2, 11, 10, 7, 11, 5, 10, 12, 7, 9, 12, 9, 3, 12, 3, 4, 12, 4, 7, 13, 0, 9, 13, 9,
    5, 13, 5, 4, 13, 4, 0, 14, 5, 11, 14, 11, 8, 14, 8, 5, 15, 8, 11, 15, 11, 7, 15, 7, 8,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sn03_is_the_only_authored_escort_stage() {
        assert!(escort_stage("sn03").is_some());
        assert!(escort_stage("sn01").is_none());
    }

    #[test]
    fn safe_zones_and_original_flag_hull_use_world_coordinates() {
        assert!(position_inside_safe_zone(
            RoleVec3 {
                x: 171.0,
                y: 99.0,
                z: 140.0
            },
            1,
            &SN03,
        ));
        assert!(!position_inside_safe_zone(
            RoleVec3 {
                x: 171.0,
                y: 0.0,
                z: 140.0
            },
            2,
            &SN03,
        ));
        assert!(character_capsule_intersects_flag(
            SN03.flag_home,
            SN03.flag_home
        ));
        assert!(!character_capsule_intersects_flag(
            RoleVec3 {
                x: 100.0,
                y: 100.0,
                z: 100.0
            },
            SN03.flag_home,
        ));
    }
}
