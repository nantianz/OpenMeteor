use super::placement::ROOTS_PER_TILE;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexAttribute, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::VertexFormat;

pub(super) const LEAF: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassLeaf", 0x4752_4153, VertexFormat::Float32x4);

pub(super) struct Lod {
    pub clumps: usize,
    leaves: usize,
    curved_leaves: usize,
    stem: &'static [f32],
}

pub(super) const DISTANT: usize = 3;

pub(super) const LODS: [Lod; 4] = [
    Lod {
        clumps: ROOTS_PER_TILE,
        leaves: 9,
        curved_leaves: 4,
        stem: &[0., 0.42, 0.67, 0.82, 0.94],
    },
    Lod {
        clumps: 144,
        leaves: 4,
        curved_leaves: 2,
        stem: &[0., 0.65, 0.84, 0.94],
    },
    Lod {
        clumps: 96,
        leaves: 2,
        curved_leaves: 1,
        stem: &[0., 0.79],
    },
    Lod {
        clumps: 80,
        leaves: 1,
        curved_leaves: 1,
        stem: &[0.],
    },
];

impl Lod {
    pub fn triangles(&self, clumps: usize) -> usize {
        let stem = (self.stem.len() - 1) * 2 + 1;
        self.clumps.min(clumps)
            * (stem + (self.curved_leaves - 1) * 3 + self.leaves - self.curved_leaves)
    }

    pub fn mesh(&self, clump_capacity: usize) -> Mesh {
        let clumps = self.clumps.min(clump_capacity);
        let mut vertices = Vec::new();
        let mut leaves = Vec::new();
        let mut indices = Vec::with_capacity(self.triangles(clumps) * 3);
        for leaf in 0..self.leaves {
            let angle = leaf as f32 * 2.399_963_1;
            let (sin, cos) = angle.sin_cos();
            let spread = if self.stem.len() == 1 {
                -1.0
            } else if leaf == 0 {
                0.02
            } else {
                0.32 + 0.075 * leaf as f32
            };
            let leaf_data = [cos, sin, leaf as f32, spread];
            let sections: &[f32] = if leaf == 0 {
                self.stem
            } else if leaf < self.curved_leaves {
                &[0., 0.53]
            } else {
                &[0.]
            };
            for clump in 0..clumps {
                let first = vertices.len() as u16;
                let root = clump as f32;
                for (i, &height) in sections.iter().enumerate() {
                    vertices.extend([[root, height, -1.0], [root, height, 1.0]]);
                    leaves.extend([leaf_data; 2]);
                    if i > 0 {
                        let a = first + (i as u16 - 1) * 2;
                        indices.extend([a, a + 1, a + 2, a + 2, a + 1, a + 3]);
                    }
                }
                let tip = vertices.len() as u16;
                vertices.push([root, 1.0, 0.0]);
                leaves.push(leaf_data);
                indices.extend([tip - 2, tip - 1, tip]);
            }
        }
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
        .with_inserted_attribute(LEAF, leaves)
        .with_inserted_indices(Indices::U16(indices))
    }
}
