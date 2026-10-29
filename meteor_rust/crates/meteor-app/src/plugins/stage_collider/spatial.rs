#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    min: [f32; 3],
    max: [f32; 3],
}

impl Bounds {
    pub(super) fn between(a: [f32; 3], b: [f32; 3]) -> Self {
        Self {
            min: std::array::from_fn(|i| a[i].min(b[i]) - 0.001),
            max: std::array::from_fn(|i| a[i].max(b[i]) + 0.001),
        }
    }
    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|i| self.min[i].min(other.min[i])),
            max: std::array::from_fn(|i| self.max[i].max(other.max[i])),
        }
    }
    fn overlaps(self, other: Self) -> bool {
        (0..3).all(|i| self.min[i] <= other.max[i] && other.min[i] <= self.max[i])
    }
    fn triangle(positions: &[[f32; 3]], indices: &[u32], tri: u32) -> Self {
        let ix = &indices[tri as usize * 3..tri as usize * 3 + 3];
        let a = positions[ix[0] as usize];
        let b = positions[ix[1] as usize];
        let c = positions[ix[2] as usize];
        Self::between(a, b).union(Self::between(c, c))
    }
}

#[derive(Debug, Clone)]
struct Leaf {
    tri: u32,
    class: u8,
    bounds: Bounds,
}
#[derive(Debug, Clone)]
struct Node {
    bounds: Bounds,
    mask: u8,
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Tree {
    leaves: Vec<Leaf>,
    nodes: Vec<Node>,
}
impl Tree {
    pub(super) fn build(
        triangles: impl Iterator<Item = (u32, u8)>,
        positions: &[[f32; 3]],
        indices: &[u32],
    ) -> Self {
        let mut tree = Self {
            leaves: triangles
                .map(|(tri, class)| Leaf {
                    tri,
                    class,
                    bounds: Bounds::triangle(positions, indices, tri),
                })
                .collect(),
            nodes: Vec::new(),
        };
        if !tree.leaves.is_empty() {
            tree.branch(0, tree.leaves.len());
        }
        tree
    }
    fn branch(&mut self, start: usize, end: usize) -> usize {
        let bounds = self.leaves[start..end]
            .iter()
            .map(|l| l.bounds)
            .reduce(Bounds::union)
            .unwrap();
        let mask = self.leaves[start..end]
            .iter()
            .fold(0, |m, l| m | (1 << l.class));
        let id = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            mask,
            start,
            end,
            children: None,
        });
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| {
                    (bounds.max[a] - bounds.min[a]).total_cmp(&(bounds.max[b] - bounds.min[b]))
                })
                .unwrap();
            let mid = start + (end - start) / 2;
            self.leaves[start..end].select_nth_unstable_by(mid - start, |a, b| {
                (a.bounds.min[axis] + a.bounds.max[axis])
                    .total_cmp(&(b.bounds.min[axis] + b.bounds.max[axis]))
            });
            let left = self.branch(start, mid);
            let right = self.branch(mid, end);
            self.nodes[id].children = Some((left, right));
        }
        id
    }

    pub(super) fn refit(&mut self, positions: &[[f32; 3]], indices: &[u32]) {
        for leaf in &mut self.leaves {
            leaf.bounds = Bounds::triangle(positions, indices, leaf.tri);
        }
        for i in (0..self.nodes.len()).rev() {
            let node = &self.nodes[i];
            let bounds = if let Some((left, right)) = node.children {
                self.nodes[left].bounds.union(self.nodes[right].bounds)
            } else {
                self.leaves[node.start..node.end]
                    .iter()
                    .map(|l| l.bounds)
                    .reduce(Bounds::union)
                    .unwrap()
            };
            self.nodes[i].bounds = bounds;
        }
    }
    pub(super) fn query(&self, bounds: Bounds, mask: u8, result: &mut Vec<(u8, u32)>) {
        if !self.nodes.is_empty() {
            self.visit(0, bounds, mask, result);
        }
    }
    fn visit(&self, id: usize, bounds: Bounds, mask: u8, result: &mut Vec<(u8, u32)>) {
        let node = &self.nodes[id];
        if node.mask & mask == 0 || !node.bounds.overlaps(bounds) {
            return;
        }
        if let Some((left, right)) = node.children {
            self.visit(left, bounds, mask, result);
            self.visit(right, bounds, mask, result);
        } else {
            for leaf in &self.leaves[node.start..node.end] {
                if mask & (1 << leaf.class) != 0 && leaf.bounds.overlaps(bounds) {
                    result.push((leaf.class, leaf.tri));
                }
            }
        }
    }
}
