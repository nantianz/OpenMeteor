use crate::{gmc::GmcMesh, gmdl::GmdlMesh};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct WeaponCollisionBox {
    pub center: [f32; 3],
    pub half_size: [f32; 3],
    pub source_name: String,

    pub fallback: bool,
}

struct ObjectView<'a> {
    name: &'a str,
    positions: &'a [f32],
    vertices: usize,
    faces: usize,
}

fn is_named_collision_object(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "atbox" | "latbox" | "ratbox"
    )
}

fn local_bounds(object: &ObjectView<'_>, fallback: bool) -> Option<WeaponCollisionBox> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in object.positions.chunks_exact(3) {
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    min[0].is_finite().then(|| WeaponCollisionBox {
        center: [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ],
        half_size: [
            (max[0] - min[0]) * 0.5,
            (max[1] - min[1]) * 0.5,
            (max[2] - min[2]) * 0.5,
        ],
        source_name: object.name.to_string(),
        fallback,
    })
}

fn extract<'a>(
    objects: impl Iterator<Item = ObjectView<'a>>,
    visibility: &HashMap<String, bool>,
) -> Vec<WeaponCollisionBox> {
    let objects: Vec<ObjectView<'a>> = objects.collect();
    let mut authored = Vec::new();
    for object in &objects {
        let hidden_box = visibility
            .get(&object.name.to_ascii_lowercase())
            .is_some_and(|visible| !visible)
            && object.vertices == 8
            && object.faces == 12;
        if is_named_collision_object(object.name) || hidden_box {
            if let Some(bounds) = local_bounds(object, false) {
                authored.push(bounds);
            }
        }
    }
    if !authored.is_empty() {
        return authored;
    }

    let visible: Vec<WeaponCollisionBox> = objects
        .iter()
        .filter(|object| {
            visibility
                .get(&object.name.to_ascii_lowercase())
                .copied()
                .unwrap_or(true)
        })
        .filter_map(|object| local_bounds(object, true))
        .collect();
    if visible.is_empty() {
        return Vec::new();
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut names = Vec::with_capacity(visible.len());
    for bounds in visible {
        for axis in 0..3 {
            min[axis] = min[axis].min(bounds.center[axis] - bounds.half_size[axis]);
            max[axis] = max[axis].max(bounds.center[axis] + bounds.half_size[axis]);
        }
        names.push(bounds.source_name);
    }
    vec![WeaponCollisionBox {
        center: [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ],
        half_size: [
            (max[0] - min[0]) * 0.5,
            (max[1] - min[1]) * 0.5,
            (max[2] - min[2]) * 0.5,
        ],
        source_name: names.join("+"),
        fallback: true,
    }]
}

pub fn extract_gmc_weapon_collision_boxes(
    model: &GmcMesh,
    visibility: &HashMap<String, bool>,
) -> Vec<WeaponCollisionBox> {
    extract(
        model.objects.iter().map(|object| ObjectView {
            name: &object.name,
            positions: &object.positions,
            vertices: object.positions.len() / 3,
            faces: object.faces.len(),
        }),
        visibility,
    )
}

pub fn extract_gmdl_weapon_collision_boxes(
    model: &GmdlMesh,
    visibility: &HashMap<String, bool>,
) -> Vec<WeaponCollisionBox> {
    extract(
        model.objects.iter().map(|object| ObjectView {
            name: &object.name,
            positions: &object.positions,
            vertices: object.num_verts,
            faces: object.num_faces,
        }),
        visibility,
    )
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gmc::{GmcFace, GmcObject};

    fn cube(name: &str) -> GmcObject {
        GmcObject {
            name: name.to_string(),
            positions: vec![
                -1.0, -2.0, -3.0, 1.0, -2.0, -3.0, -1.0, 2.0, -3.0, 1.0, 2.0, -3.0, -1.0, -2.0,
                3.0, 1.0, -2.0, 3.0, -1.0, 2.0, 3.0, 1.0, 2.0, 3.0,
            ],
            faces: vec![
                GmcFace {
                    shader: -1,
                    indices: [0, 1, 2],
                };
                12
            ],
            ..Default::default()
        }
    }

    #[test]
    fn hidden_eight_vertex_twelve_face_object_is_authored_collision() {
        let model = GmcMesh {
            objects: vec![cube("hgat")],
            ..Default::default()
        };
        let visibility = HashMap::from([("hgat".to_string(), false)]);
        let boxes = extract_gmc_weapon_collision_boxes(&model, &visibility);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].center, [0.0, 0.0, 0.0]);
        assert_eq!(boxes[0].half_size, [1.0, 2.0, 3.0]);
        assert!(!boxes[0].fallback);
    }

    #[test]
    fn visible_geometry_unions_only_when_no_authored_box_exists() {
        let mut a = cube("blade");
        a.faces.truncate(1);
        let mut b = cube("guard");
        b.faces.truncate(1);
        for p in b.positions.chunks_exact_mut(3) {
            p[0] += 5.0;
        }
        let model = GmcMesh {
            objects: vec![a, b],
            ..Default::default()
        };
        let boxes = extract_gmc_weapon_collision_boxes(&model, &HashMap::new());
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].center, [2.5, 0.0, 0.0]);
        assert_eq!(boxes[0].half_size, [3.5, 2.0, 3.0]);
        assert!(boxes[0].fallback);
    }

    fn read_ci(dir: &std::path::Path, name: &str) -> Vec<u8> {
        std::fs::read_dir(dir)
            .expect("read cmodel")
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(name)
            })
            .and_then(|entry| std::fs::read(entry.path()).ok())
            .unwrap_or_else(|| panic!("missing {}", dir.join(name).display()))
    }

    fn assert_vec3(actual: [f32; 3], expected: [f32; 3]) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() < 1e-3,
                "axis {axis}: {} != {}",
                actual[axis],
                expected[axis]
            );
        }
    }

    #[test]
    fn origin_weapon_classes_match_web_collision_box_projection() {
        struct Expected {
            stem: &'static str,
            source: &'static str,
            fallback: bool,
            center: [f32; 3],
            half: [f32; 3],
        }
        let expected = [
            Expected {
                stem: "w3_0",
                source: "wpnFT01",
                fallback: true,
                center: [0.0, -1.423, 0.0],
                half: [0.575, 2.785, 0.081],
            },
            Expected {
                stem: "w6_0",
                source: "wpnFR02",
                fallback: true,
                center: [0.0115, -0.018, -0.3785],
                half: [5.3635, 5.365, 0.5695],
            },
            Expected {
                stem: "w2_0",
                source: "wpnFA03",
                fallback: true,
                center: [0.6235, -3.143, 0.0],
                half: [1.3375, 11.088, 1.102],
            },
            Expected {
                stem: "w8_0r",
                source: "Rlnat",
                fallback: false,
                center: [-1.234, 0.0015, 0.003],
                half: [4.371, 9.9655, 3.022],
            },
            Expected {
                stem: "w1_0",
                source: "ldat",
                fallback: false,
                center: [0.0575, 5.795, -0.0475],
                half: [2.9705, 10.633, 2.8405],
            },
            Expected {
                stem: "w0_0",
                source: "lsat",
                fallback: false,
                center: [0.046, -8.2395, 0.078],
                half: [3.16, 15.7305, 3.21],
            },
            Expected {
                stem: "w7_0",
                source: "hgat",
                fallback: false,
                center: [-0.3435, -5.045, 0.1305],
                half: [4.8575, 30.669, 4.8685],
            },
            Expected {
                stem: "w5_0",
                source: "hbat",
                fallback: false,
                center: [0.0055, -6.858, 0.102],
                half: [6.1065, 20.65, 4.965],
            },
            Expected {
                stem: "w4_0",
                source: "hmat",
                fallback: false,
                center: [0.0, -0.561, 0.0],
                half: [7.575, 21.754, 7.272],
            },
            Expected {
                stem: "w9_0r",
                source: "atBox",
                fallback: false,
                center: [0.0, -8.121968, 0.0],
                half: [2.245549, 15.419439, 1.34733],
            },
            Expected {
                stem: "w10_0r",
                source: "RatBox",
                fallback: false,
                center: [-0.366306, 0.454024, 0.880897],
                half: [4.334893, 2.354761, 1.710285],
            },
            Expected {
                stem: "w11_0",
                source: "AtBox",
                fallback: false,
                center: [-0.0184, -5.218577, 0.00451],
                half: [1.323578, 10.726162, 0.897097],
            },
        ];
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/cmodel");
        for item in expected {
            let des = read_ci(&dir, &format!("{}.des", item.stem));
            let visibility = crate::des::parse_object_visibility(&String::from_utf8_lossy(&des));
            let gmc_name = format!("{}.gmc", item.stem);
            let boxes = if std::fs::read_dir(&dir)
                .expect("read cmodel")
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&gmc_name)
                }) {
                let bytes = read_ci(&dir, &gmc_name);
                let model = crate::gmc::parse_gmc(&String::from_utf8_lossy(&bytes));
                extract_gmc_weapon_collision_boxes(&model, &visibility)
            } else {
                let bytes = read_ci(&dir, &format!("{}.gmb", item.stem));
                let model = crate::gmdl::decode_gmdl(&bytes).expect("decode weapon gmb");
                extract_gmdl_weapon_collision_boxes(&model, &visibility)
            };
            assert_eq!(boxes.len(), 1, "{}", item.stem);
            assert_eq!(boxes[0].source_name, item.source, "{}", item.stem);
            assert_eq!(boxes[0].fallback, item.fallback, "{}", item.stem);
            assert_vec3(boxes[0].center, item.center);
            assert_vec3(boxes[0].half_size, item.half);
        }
    }
}
