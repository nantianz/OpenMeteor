use std::collections::HashMap;

use meteor_asset::bnc::BoneData;
use meteor_core::{Mat4, Quat, Vec3};

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub nodes: Vec<SkeletonNode>,

    pub index_by_name: HashMap<String, usize>,

    pub skin_indices: Vec<usize>,

    pub skin_world_bind_inverse: Vec<Mat4>,

    pub skin_world_bind: Vec<Mat4>,
}

#[derive(Debug, Clone)]
pub struct SkeletonNode {
    pub name: String,
    pub is_dummy: bool,

    pub parent: Option<usize>,

    pub children: Vec<usize>,

    pub local_translation: Vec3,
    pub local_rotation: Quat,

    pub world_bind: Mat4,
}

impl Skeleton {
    pub fn from_bone_data(data: &BoneData) -> Self {
        let index_by_name: HashMap<String, usize> = data
            .all
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.clone(), i))
            .collect();

        let mut nodes: Vec<SkeletonNode> = Vec::with_capacity(data.all.len());
        for (i, n) in data.all.iter().enumerate() {
            let parent = n
                .parent
                .as_deref()
                .and_then(|name| index_by_name.get(name).copied());

            let parent = parent.filter(|&p| p != i);
            nodes.push(SkeletonNode {
                name: n.name.clone(),
                is_dummy: n.is_dummy,
                parent,
                children: Vec::new(),
                local_translation: n.pivot,
                local_rotation: n.quat,
                world_bind: Mat4::IDENTITY,
            });
        }
        for i in 0..nodes.len() {
            if let Some(p) = nodes[i].parent {
                nodes[p].children.push(i);
            }
        }

        let roots: Vec<usize> = (0..nodes.len())
            .filter(|&i| nodes[i].parent.is_none())
            .collect();
        for r in roots {
            propagate_world_bind(&mut nodes, r);
        }

        let skin_indices: Vec<usize> = data
            .bones
            .iter()
            .map(|b| index_by_name.get(&b.name).copied().unwrap_or(usize::MAX))
            .collect();
        let skin_world_bind: Vec<Mat4> = skin_indices
            .iter()
            .map(|&i| {
                if i == usize::MAX {
                    Mat4::IDENTITY
                } else {
                    nodes[i].world_bind
                }
            })
            .collect();
        let skin_world_bind_inverse: Vec<Mat4> =
            skin_world_bind.iter().map(|m| m.invert()).collect();

        Skeleton {
            nodes,
            index_by_name,
            skin_indices,
            skin_world_bind_inverse,
            skin_world_bind,
        }
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.index_by_name.get(name).copied()
    }

    pub fn skin_index_of(&self, name: &str) -> Option<usize> {
        let node_idx = self.index_of(name)?;
        self.skin_indices.iter().position(|&i| i == node_idx)
    }

    pub fn skin_bone_count(&self) -> usize {
        self.skin_indices.len()
    }
}

fn propagate_world_bind(nodes: &mut [SkeletonNode], i: usize) {
    let (local_t, local_r, parent) = {
        let n = &nodes[i];
        (n.local_translation, n.local_rotation, n.parent)
    };
    let local = Mat4::from_trs(local_t, local_r);
    let world = match parent {
        Some(p) => nodes[p].world_bind.mul(&local),
        None => local,
    };
    nodes[i].world_bind = world;

    let children: Vec<usize> = nodes[i].children.clone();
    for c in children {
        propagate_world_bind(nodes, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_asset::bnc::parse_bones;

    fn chain_bnc() -> &'static str {
        "Bones: 30 Dummey: 6\n\
         bone b { parent NULL pivot 0 0 0 quaternion 1 0 0 0 }\n\
         bone b1 { parent b pivot 10 0 0 quaternion 1 0 0 0 }\n\
         bone b2 { parent b1 pivot 10 0 0 quaternion 1 0 0 0 }\n"
    }

    #[test]
    fn builds_hierarchy_and_parent_indices() {
        let data = parse_bones(chain_bnc());
        let skel = Skeleton::from_bone_data(&data);
        assert_eq!(skel.nodes.len(), 3);
        assert!(skel.nodes[0].parent.is_none());
        assert_eq!(skel.nodes[1].parent, Some(0));
        assert_eq!(skel.nodes[2].parent, Some(1));
        assert_eq!(skel.nodes[0].children, vec![1]);
        assert_eq!(skel.nodes[1].children, vec![2]);
    }

    #[test]
    fn world_bind_accumulates_translation_down_chain() {
        let data = parse_bones(chain_bnc());
        let skel = Skeleton::from_bone_data(&data);

        let o = Vec3::ZERO;
        assert_eq!(skel.nodes[0].world_bind.transform_point(o), Vec3::ZERO);
        let p1 = skel.nodes[1].world_bind.transform_point(o);
        assert!((p1.x - 10.0).abs() < 1e-4 && p1.y.abs() < 1e-4 && p1.z.abs() < 1e-4);
        let p2 = skel.nodes[2].world_bind.transform_point(o);
        assert!((p2.x - 20.0).abs() < 1e-4 && p2.y.abs() < 1e-4 && p2.z.abs() < 1e-4);
    }

    #[test]
    fn skin_indices_match_real_bone_order() {
        let data = parse_bones(chain_bnc());
        let skel = Skeleton::from_bone_data(&data);
        assert_eq!(skel.skin_bone_count(), 3);
        assert_eq!(skel.skin_index_of("b"), Some(0));
        assert_eq!(skel.skin_index_of("b1"), Some(1));
        assert_eq!(skel.skin_index_of("b2"), Some(2));
        assert_eq!(skel.skin_index_of("nope"), None);
    }

    #[test]
    fn bone_inverses_invert_world_bind() {
        let data = parse_bones(chain_bnc());
        let skel = Skeleton::from_bone_data(&data);
        let id = Mat4::IDENTITY;
        for i in 0..skel.skin_bone_count() {
            let w = skel.skin_world_bind[i];
            let inv = skel.skin_world_bind_inverse[i];
            let product = w.mul(&inv);
            for j in 0..16 {
                let delta = (product.m[j] - id.m[j]).abs();
                assert!(
                    delta < 1e-3,
                    "inverse mismatch at skin bone {i} idx {j}: {delta}"
                );
            }
        }
    }

    #[test]
    fn world_bind_composes_rotation_and_translation() {
        let s = (std::f32::consts::FRAC_PI_4).sin();
        let c = (std::f32::consts::FRAC_PI_4).cos();
        let bnc = format!(
            "Bones: 30 Dummey: 6\n\
             bone b {{ parent NULL pivot 0 0 0 quaternion {c} 0 0 {s} }}\n\
             bone b1 {{ parent b pivot 10 0 0 quaternion 1 0 0 0 }}\n"
        );
        let data = parse_bones(&bnc);
        let skel = Skeleton::from_bone_data(&data);
        let p = skel.nodes[1].world_bind.transform_point(Vec3::ZERO);

        assert!(p.x.abs() < 1e-4, "x = {}", p.x);
        assert!((p.y - 10.0).abs() < 1e-4, "y = {}", p.y);
        assert!(p.z.abs() < 1e-4, "z = {}", p.z);
    }

    #[test]
    fn dummies_excluded_from_skin_but_in_hierarchy() {
        let bnc = "Bones: 30 Dummey: 6\n\
            bone b { parent NULL pivot 0 0 0 quaternion 1 0 0 0 }\n\
            Dummey d_wpnR { parent b pivot 5 0 0 quaternion 1 0 0 0 }\n";
        let data = parse_bones(bnc);
        let skel = Skeleton::from_bone_data(&data);
        assert_eq!(skel.skin_bone_count(), 1);
        assert!(skel.skin_index_of("d_wpnR").is_none());
        assert_eq!(skel.index_of("d_wpnR"), Some(1));
        let p = skel.nodes[1].world_bind.transform_point(Vec3::ZERO);
        assert!((p.x - 5.0).abs() < 1e-4);
    }

    #[test]
    fn parent_alias_resolves_through_dummy() {
        let bnc = "Bones: 30 Dummey: 6\n\
            bone b { parent NULL pivot 0 0 0 quaternion 1 0 0 0 }\n\
            Dummey d_wpnRP { parent b pivot 0 0 0 quaternion 1 0 0 0 }\n\
            Dummey d_wpnR { parent d0 pivot 5 0 0 quaternion 1 0 0 0 }\n";
        let data = parse_bones(bnc);

        assert_eq!(data.dummies[1].parent.as_deref(), Some("d_wpnRP"));
        let skel = Skeleton::from_bone_data(&data);
        let wpn_r = skel.index_of("d_wpnR").unwrap();
        let parent = skel.nodes[wpn_r].parent.unwrap();
        assert_eq!(skel.nodes[parent].name, "d_wpnRP");
        let p = skel.nodes[wpn_r].world_bind.transform_point(Vec3::ZERO);
        assert!((p.x - 5.0).abs() < 1e-4);
    }
}
