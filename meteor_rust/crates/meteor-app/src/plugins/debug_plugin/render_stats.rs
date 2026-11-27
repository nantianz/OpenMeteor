use crate::{app_state::AppState, plugins::camera_plugin::GameplayCamera};
use bevy::pbr::RenderMeshInstances;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::render::{
    mesh::{RenderMesh, RenderMeshBufferInfo},
    render_asset::RenderAssets,
    render_resource::PrimitiveTopology,
    view::{RenderVisibleEntities, ViewTarget},
    Render, RenderApp, RenderSystems,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Resource, Clone, Default)]
pub(super) struct RenderGeometryStats(Arc<SharedStats>);

#[derive(Default)]
struct SharedStats {
    enabled: AtomicBool,
    geometry: Mutex<Option<GeometryCounts>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GeometryCounts {
    meshes: u64,
    triangles: u64,
    vertices: u64,
}

impl GeometryCounts {
    fn add_mesh(
        &mut self,
        vertex_count: u32,
        buffer_info: &RenderMeshBufferInfo,
        topology: PrimitiveTopology,
    ) {
        let elements = match buffer_info {
            RenderMeshBufferInfo::Indexed { count, .. } => *count,
            RenderMeshBufferInfo::NonIndexed => vertex_count,
        };
        self.meshes += 1;
        self.vertices += u64::from(vertex_count);
        self.triangles += u64::from(match topology {
            PrimitiveTopology::TriangleList => elements / 3,
            PrimitiveTopology::TriangleStrip => elements.saturating_sub(2),
            _ => 0,
        });
    }
}

impl RenderGeometryStats {
    pub(super) fn set_enabled(&self, enabled: bool) {
        self.0.enabled.store(enabled, Ordering::Relaxed);
    }

    pub(super) fn label(&self) -> String {
        let Some(counts) = *self.0.geometry.lock().unwrap() else {
            return "主视图网格：等待采样".into();
        };
        format!(
            "主视图网格实例：{}\n三角面：{} / 顶点：{}",
            counts.meshes, counts.triangles, counts.vertices,
        )
    }
}

pub(super) fn install(app: &mut App) {
    let stats = RenderGeometryStats::default();
    app.insert_resource(stats.clone())
        .add_systems(OnExit(AppState::Playing), stop_capture);
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(stats).add_systems(
            Render,
            capture_geometry
                .in_set(crate::plugins::render_pacing::DrawFrame)
                .after(RenderSystems::Prepare)
                .before(RenderSystems::Render),
        );
    }
}

fn stop_capture(stats: Res<RenderGeometryStats>) {
    stats.set_enabled(false);
}

fn capture_geometry(
    shared: Res<RenderGeometryStats>,
    views: Query<(Entity, &RenderVisibleEntities), (With<GameplayCamera>, With<ViewTarget>)>,
    instances: Option<Res<RenderMeshInstances>>,
    meshes: Option<Res<RenderAssets<RenderMesh>>>,
    mut last_sample: Local<Option<(Entity, Instant)>>,
) {
    let active_view = if shared.0.enabled.load(Ordering::Relaxed) {
        views.single().ok()
    } else {
        None
    };
    let Some((camera, visible)) = active_view else {
        if last_sample.take().is_some() {
            *shared.0.geometry.lock().unwrap() = None;
        }
        return;
    };
    let now = Instant::now();
    if last_sample.is_some_and(|(previous_camera, previous_time)| {
        camera == previous_camera
            && (now - previous_time).as_secs_f32() < super::STATS_INTERVAL_SECS
    }) {
        return;
    }
    let (Some(instances), Some(meshes)) = (instances, meshes) else {
        return;
    };

    let mut counts = GeometryCounts::default();
    if let Some(visible) = visible.get::<Mesh3d>() {
        for (_, entity) in visible.iter_visible() {
            if let Some(mesh) = instances
                .mesh_asset_id(*entity)
                .and_then(|id| meshes.get(id))
            {
                counts.add_mesh(
                    mesh.vertex_count,
                    &mesh.buffer_info,
                    mesh.primitive_topology(),
                );
            }
        }
    }
    *shared.0.geometry.lock().unwrap() = Some(counts);
    *last_sample = Some((camera, now));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_counts_mesh_instances_with_indexed_and_non_indexed_topologies() {
        let mut counts = GeometryCounts::default();
        let quad = RenderMeshBufferInfo::Indexed {
            count: 6,
            index_format: bevy::render::render_resource::IndexFormat::Uint32,
        };
        for _ in 0..2 {
            counts.add_mesh(4, &quad, PrimitiveTopology::TriangleList);
        }
        let non_indexed = RenderMeshBufferInfo::NonIndexed;
        counts.add_mesh(5, &non_indexed, PrimitiveTopology::TriangleStrip);
        counts.add_mesh(2, &non_indexed, PrimitiveTopology::LineList);
        counts.add_mesh(6, &non_indexed, PrimitiveTopology::TriangleList);
        assert_eq!(
            counts,
            GeometryCounts {
                meshes: 5,
                triangles: 9,
                vertices: 21
            }
        );
    }
}
