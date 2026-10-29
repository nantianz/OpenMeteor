pub const ARENA_CENTER: [f32; 3] = [-11.0, 16.0, 0.0];

pub const STAGE_CENTER_OVERRIDES: &[(&str, [f32; 3])] = &[("sn01", [-11.0, 16.0, 0.0])];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageAssetPaths {
    pub stage_id: String,

    pub gmb: String,

    pub des: String,

    pub cob: String,

    pub pst: String,
}

pub fn stage_asset_paths(stage_id: &str) -> StageAssetPaths {
    let id = stage_id.trim().to_lowercase();
    let stem = format!("{}/{}", id, id);
    StageAssetPaths {
        stage_id: id.clone(),
        gmb: format!("{stem}.gmb"),
        des: format!("{stem}.des"),
        cob: format!("{stem}.cob"),
        pst: format!("{stem}.pst"),
    }
}

pub fn estimate_center_from_waypoints(waypoint_text: &str) -> [f32; 3] {
    let mut pts: Vec<(f32, f32)> = Vec::new();
    for line in waypoint_text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Pos") {
            let vals: Vec<f32> = rest
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if vals.len() >= 2 {
                pts.push((vals[0], vals[1]));
            }
        }
    }
    if pts.is_empty() {
        return ARENA_CENTER;
    }
    let n = pts.len() as f32;
    let sx: f32 = pts.iter().map(|p| p.0).sum();
    let sy: f32 = pts.iter().map(|p| p.1).sum();

    [sx / n, sy / n, 0.0]
}

pub fn get_stage_center(stage_id: &str, waypoint_text: Option<&str>) -> [f32; 3] {
    let normalized = stage_id.trim().to_lowercase();
    if let Some((_, center)) = STAGE_CENTER_OVERRIDES
        .iter()
        .find(|(id, _)| *id == normalized)
    {
        return *center;
    }
    match waypoint_text {
        Some(text) => estimate_center_from_waypoints(text),
        None => ARENA_CENTER,
    }
}

#[derive(Debug, Clone)]
pub struct WaypointPoint {
    pub id: usize,

    pub position: [f32; 3],

    pub size: f32,

    pub links: Vec<(usize, f32)>,
}

#[derive(Debug, Clone, Default, bevy_ecs::prelude::Resource)]
pub struct WaypointGraph {
    pub points: Vec<WaypointPoint>,

    pub link_types: std::collections::HashMap<(usize, usize), u8>,
    pub center: [f32; 3],
    pub disabled: std::collections::HashSet<usize>,
}

impl WaypointGraph {
    pub fn set_enabled(&mut self, id: usize, enabled: bool) -> bool {
        if id >= self.points.len() {
            return false;
        }
        if enabled {
            self.disabled.remove(&id)
        } else {
            self.disabled.insert(id)
        }
    }
    pub fn is_enabled(&self, id: usize) -> bool {
        id < self.points.len() && !self.disabled.contains(&id)
    }
    pub fn navigation_links(&self, id: usize) -> Vec<(usize, f32)> {
        if !self.is_enabled(id) {
            return Vec::new();
        }
        self.points[id]
            .links
            .iter()
            .copied()
            .filter(|(target, _)| self.is_enabled(*target))
            .collect()
    }

    pub fn world_position(&self, id: usize) -> Option<[f32; 3]> {
        let point = self.points.get(id)?;
        let [cx, cy, cz] = self.center;
        Some([
            (point.position[0] - cx) * meteor_core::constants::MODEL_SCALE,
            (point.position[2] - cz) * meteor_core::constants::MODEL_SCALE,
            -(point.position[1] - cy) * meteor_core::constants::MODEL_SCALE,
        ])
    }

    pub fn nearest(&self, world_pos: [f32; 3]) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for point in &self.points {
            if !self.is_enabled(point.id) {
                continue;
            }
            let [wx, _, wz] = self.world_position(point.id)?;
            let dx = wx - world_pos[0];
            let dz = wz - world_pos[2];
            let d2 = dx * dx + dz * dz;
            if best.map(|(_, b)| d2 < b).unwrap_or(true) {
                best = Some((point.id, d2));
            }
        }
        best.map(|(id, _)| id)
    }
}

pub fn parse_waypoint_graph(text: &str, center: [f32; 3]) -> WaypointGraph {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let Some(header) = lines.first().and_then(|l| l.strip_prefix("WayPoints")) else {
        return WaypointGraph::default();
    };
    let expected: usize = header
        .split_whitespace()
        .find_map(|s| s.parse().ok())
        .unwrap_or(0);
    let mut points: Vec<WaypointPoint> = Vec::new();
    let mut link_types = std::collections::HashMap::new();
    let mut index = 1usize;
    while index < lines.len() && points.len() < expected {
        let pos: Option<[f32; 3]> = lines.get(index).and_then(|l| {
            l.strip_prefix("Pos").and_then(|rest| {
                let vals: Vec<f32> = rest
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                (vals.len() >= 3).then(|| [vals[0], vals[1], vals[2]])
            })
        });
        index += 1;
        let size: f32 = lines
            .get(index)
            .and_then(|l| l.strip_prefix("Size"))
            .and_then(|rest| rest.split_whitespace().find_map(|s| s.parse().ok()))
            .unwrap_or(0.0);
        index += 1;
        let link_count: usize = lines
            .get(index)
            .and_then(|l| l.strip_prefix("Link"))
            .and_then(|rest| rest.split_whitespace().find_map(|s| s.parse().ok()))
            .unwrap_or(0);
        index += 1;
        let Some(position) = pos else {
            break;
        };
        let mut links = Vec::with_capacity(link_count);
        for _ in 0..link_count {
            if let Some(edge) = lines.get(index) {
                let vals: Vec<f32> = edge
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if vals.len() >= 3 {
                    links.push((vals[0] as usize, vals[2].max(0.0)));
                    link_types.insert((points.len(), vals[0] as usize), vals[1] as u8);
                }
            }
            index += 1;
        }
        points.push(WaypointPoint {
            id: points.len(),
            position,
            size,
            links,
        });
    }
    WaypointGraph {
        points,
        link_types,
        center,
        disabled: Default::default(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoadedStage {
    pub stage_id: String,

    pub scene_object_count: usize,

    pub dummy_count: usize,

    pub collision_face_count: usize,

    pub center: [f32; 3],

    pub ready: bool,
}

pub trait StageAssetSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

pub fn load_stage_assets<S: StageAssetSource>(
    source: &S,
    stage_id: &str,
    center: [f32; 3],
) -> LoadedStage {
    let paths = stage_asset_paths(stage_id);
    let gmb = source.read(&paths.gmb);
    let des = source.read(&paths.des);
    let cob = source.read(&paths.cob);
    let _pst = source.read(&paths.pst);

    LoadedStage {
        stage_id: paths.stage_id.clone(),
        scene_object_count: des.as_ref().map(|d| d.len()).unwrap_or(0),
        dummy_count: 0,
        collision_face_count: cob.as_ref().map(|c| c.len() / 4 / 3).unwrap_or(0),
        center,
        ready: gmb.is_some() && des.is_some() && cob.is_some(),
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_convention() {
        let p = stage_asset_paths("sn01");
        assert_eq!(p.gmb, "sn01/sn01.gmb");
        assert_eq!(p.des, "sn01/sn01.des");
        assert_eq!(p.cob, "sn01/sn01.cob");
        assert_eq!(p.pst, "sn01/sn01.pst");

        let p = stage_asset_paths("SN02");
        assert_eq!(p.gmb, "sn02/sn02.gmb");
    }

    #[test]
    fn waypoint_graph_parses_blocks() {
        let text = "WayPoints 2\nPos 0 0 0\nSize 1\nLink 1\n1 0 5.0\nPos 10 0 0\nSize 2\nLink 1\n0 0 5.0\n";
        let graph = parse_waypoint_graph(text, [0.0, 0.0, 0.0]);
        assert_eq!(graph.points.len(), 2);
        assert_eq!(graph.points[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(graph.points[0].size, 1.0);
        assert_eq!(graph.points[0].links, vec![(1, 5.0)]);
        assert_eq!(graph.points[1].links, vec![(0, 5.0)]);
    }

    #[test]
    fn waypoint_graph_world_position_matches_web_transform() {
        let graph = parse_waypoint_graph(
            "WayPoints 1\nPos 10 20 30\nSize 1\nLink 0\n",
            [0.0, 0.0, 0.0],
        );
        let w = graph.world_position(0).unwrap();
        assert!((w[0] - 10.0 * 0.12).abs() < 1e-5);
        assert!((w[1] - 30.0 * 0.12).abs() < 1e-5);
        assert!((w[2] - (-20.0 * 0.12)).abs() < 1e-5);

        let graph = parse_waypoint_graph(
            "WayPoints 1\nPos 10 20 30\nSize 1\nLink 0\n",
            [-11.0, 16.0, 0.0],
        );
        let w = graph.world_position(0).unwrap();
        assert!((w[0] - 21.0 * 0.12).abs() < 1e-5);
        assert!((w[2] - (-(20.0 - 16.0) * 0.12)).abs() < 1e-5);
    }

    #[test]
    fn waypoint_graph_nearest_uses_planar_distance() {
        let graph = parse_waypoint_graph(
            "WayPoints 2\nPos 0 0 0\nSize 1\nLink 0\nPos 10 0 0\nSize 1\nLink 0\n",
            [0.0, 0.0, 0.0],
        );
        assert_eq!(graph.nearest([0.2, 0.0, 0.1]), Some(0));
        assert_eq!(graph.nearest([9.9, 5.0, 0.0]), Some(1));
        assert!(graph.nearest([0.0, 0.0, 0.0]).is_some());
        assert_eq!(WaypointGraph::default().nearest([0.0, 0.0, 0.0]), None);
    }

    #[test]
    fn origin_res_sn01_waypoint_graph_regression() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("Level/sn01.wp");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let graph = parse_waypoint_graph(&text, ARENA_CENTER);
        assert!(
            graph.points.len() > 10,
            "sn01 waypoints: {}",
            graph.points.len()
        );

        let start = graph.nearest([0.0, 0.0, 0.0]).unwrap();
        let goal = graph.nearest([3.0, 0.0, 3.0]).unwrap();
        let ai_points: Vec<meteor_ai::behavior::Waypoint> = graph
            .points
            .iter()
            .map(|p| meteor_ai::behavior::Waypoint {
                id: p.id,
                links: p.links.clone(),
            })
            .collect();
        let path = meteor_ai::behavior::find_waypoint_path(&ai_points, start, goal);
        assert!(!path.is_empty(), "sn01 waypoint graph must be connected");
    }

    #[test]
    fn center_estimate_from_waypoints() {
        let text = "Pos 10 20 0\nPos 30 40 5\n";
        let c = estimate_center_from_waypoints(text);

        assert!((c[0] - 20.0).abs() < 1e-5);
        assert!((c[1] - 30.0).abs() < 1e-5);
        assert_eq!(c[2], 0.0);
    }

    #[test]
    fn center_estimate_empty_fallback() {
        let c = estimate_center_from_waypoints("");
        assert_eq!(c, ARENA_CENTER);
    }

    #[test]
    fn get_center_prefers_override() {
        let c = get_stage_center("sn01", Some("Pos 100 100 0"));
        assert_eq!(c, [-11.0, 16.0, 0.0]);

        let c = get_stage_center("sn02", Some("Pos 5 6 0"));
        assert!((c[0] - 5.0).abs() < 1e-5);

        let c = get_stage_center("sn02", None);
        assert_eq!(c, ARENA_CENTER);
    }

    #[test]
    fn load_stage_readiness() {
        struct Stub;
        impl StageAssetSource for Stub {
            fn read(&self, path: &str) -> Option<Vec<u8>> {
                match path {
                    "sn01/sn01.gmb" => Some(vec![0u8; 100]),
                    "sn01/sn01.des" => Some(vec![0u8; 50]),
                    "sn01/sn01.cob" => Some(vec![0u8; 36]),
                    "sn01/sn01.pst" => Some(vec![0u8; 10]),
                    _ => None,
                }
            }
        }
        let loaded = load_stage_assets(&Stub, "sn01", [0.0, 0.0, 0.0]);
        assert!(loaded.ready);
        assert_eq!(loaded.stage_id, "sn01");
    }

    #[test]
    fn load_stage_missing_assets() {
        struct Empty;
        impl StageAssetSource for Empty {
            fn read(&self, _path: &str) -> Option<Vec<u8>> {
                None
            }
        }
        let loaded = load_stage_assets(&Empty, "sn01", [0.0, 0.0, 0.0]);
        assert!(!loaded.ready);
    }
}
