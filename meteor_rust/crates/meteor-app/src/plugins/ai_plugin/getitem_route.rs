use super::*;

pub(super) fn direct_getitem_route(
    graph: Option<&WaypointGraph>,
    mesh: Option<&StageMeshCollider>,
    from: Vec3,
    target: Vec3,
) -> bool {
    if graph.is_none_or(|graph| graph.points.is_empty()) {
        return false;
    }
    super::super::npc_navigation::reachable(
        mesh,
        from + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE),
        target,
    )
}
