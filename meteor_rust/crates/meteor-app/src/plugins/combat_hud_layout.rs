use super::ui_scale_plugin::SafeAreaInsets;
use bevy::prelude::*;

pub(crate) struct CombatHudLayout {
    pub map_origin: Vec2,
    pub map_diameter: f32,
    pub stats_origin: Vec2,
    pub status_width: f32,
    pub dialogue_origin: Vec2,
    pub dialogue_size: Vec2,
    pub button_size: Vec2,
    pub pause_center: Vec2,
    pub debug_center: Vec2,
}

impl CombatHudLayout {
    pub fn new(width: f32, height: f32, safe: SafeAreaInsets, mobile: bool) -> Self {
        let safe = safe.sanitized(width, height);
        let map_diameter = if mobile {
            (height * 0.22).clamp(80.0, 100.0)
        } else {
            152.0
        };
        let map_origin = Vec2::new(width - safe.right - 10.0 - map_diameter, safe.top + 10.0);
        let button_size = Vec2::new(64.0, 28.0);
        let center = safe.left + (width - safe.left - safe.right) * 0.5;
        let mut pause_center = Vec2::new(
            center + (button_size.x + 8.0) * 0.5,
            safe.top + 10.0 + button_size.y * 0.5,
        );
        let status_width = if mobile { 180.0 } else { 224.0 };
        let stats_origin = Vec2::new(safe.left + 10.0, safe.top + 14.0);

        if center - button_size.x - 4.0 < stats_origin.x + status_width + 12.0
            || center + button_size.x + 4.0 > map_origin.x - 12.0
        {
            pause_center.y = (stats_origin.y + 132.0).max(map_origin.y + map_diameter + 12.0)
                + button_size.y * 0.5;
        }
        let debug_center = if cfg!(target_arch = "wasm32") && !mobile {
            Vec2::new(center, pause_center.y)
        } else {
            pause_center - Vec2::X * (button_size.x + 8.0)
        };
        let dialogue_size = Vec2::new(
            (width - safe.left - safe.right - 32.0).min(if mobile { 360.0 } else { 420.0 }),
            120.0,
        );
        let center_x = safe.left + (width - safe.left - safe.right - dialogue_size.x) * 0.5;
        let mut top = (safe.top + (height * 0.10).clamp(24.0, 64.0))
            .max(pause_center.y + button_size.y * 0.5 + 12.0);

        if center_x < stats_origin.x + status_width + 12.0
            && center_x + dialogue_size.x > stats_origin.x
        {
            top = top.max(stats_origin.y + 132.0);
        }
        if center_x + dialogue_size.x > map_origin.x - 12.0
            && center_x < map_origin.x + map_diameter
        {
            top = top.max(map_origin.y + map_diameter + 12.0);
        }
        let dialogue_origin = Vec2::new(center_x, top);
        Self {
            map_origin,
            map_diameter,
            stats_origin,
            status_width,
            dialogue_origin,
            dialogue_size,
            button_size,
            pause_center,
            debug_center,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combat_hud_reserves_disjoint_map_stats_and_toolbar_regions() {
        for (width, height) in [(899.0, 432.0), (640.0, 360.0), (393.0, 852.0)] {
            let layout = CombatHudLayout::new(
                width,
                height,
                SafeAreaInsets {
                    left: 28.0,
                    right: 12.0,
                    ..default()
                },
                true,
            );
            let map = Rect::from_corners(
                layout.map_origin,
                layout.map_origin + Vec2::splat(layout.map_diameter),
            );
            let debug = Rect::from_center_size(layout.debug_center, layout.button_size);
            let pause = Rect::from_center_size(layout.pause_center, layout.button_size);
            assert!(pause.max.x < map.min.x || pause.min.y > map.max.y);
            assert!(debug.max.x < pause.min.x);
            assert!(layout.stats_origin.x + layout.status_width < map.min.x);
            assert!(
                debug.min.x > layout.stats_origin.x + layout.status_width
                    || debug.min.y >= layout.stats_origin.y + 132.0
            );
            assert!(
                ((debug.min.x + pause.max.x) * 0.5 - (28.0 + (width - 40.0) * 0.5)).abs() < 0.01
            );
            assert!(pause.max.x <= width - 12.0);
        }
    }
}
