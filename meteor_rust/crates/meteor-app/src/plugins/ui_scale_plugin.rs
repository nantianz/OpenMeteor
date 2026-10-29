use crate::app_state::Settings;
use bevy::prelude::*;
use bevy::ui::UiScale;

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicU32, Ordering};

pub const UI_REFERENCE_WIDTH: f32 = 1280.0;
pub const UI_REFERENCE_HEIGHT: f32 = 720.0;

pub const UI_SCALE_MIN: f32 = 0.55;

pub fn adaptive_ui_scale(width: f32, height: f32, manual: f32, native_mobile: bool) -> f32 {
    if manual > 0.0 {
        manual.clamp(0.5, 1.5)
    } else if native_mobile {
        1.0
    } else {
        (width / UI_REFERENCE_WIDTH)
            .min(height / UI_REFERENCE_HEIGHT)
            .clamp(UI_SCALE_MIN, 1.0)
    }
}

#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct EffectiveUiScale(pub f32);

impl Default for EffectiveUiScale {
    fn default() -> Self {
        Self(1.0)
    }
}

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct SafeAreaInsets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SafeAreaInsets {
    pub fn sanitized(self, width: f32, height: f32) -> Self {
        let max_x = (width.max(0.0) * 0.45).max(0.0);
        let max_y = (height.max(0.0) * 0.45).max(0.0);
        let axis = |value: f32, max: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                0.0
            }
        };
        Self {
            left: axis(self.left, max_x),
            top: axis(self.top, max_y),
            right: axis(self.right, max_x),
            bottom: axis(self.bottom, max_y),
        }
    }
}

#[cfg(target_os = "android")]
static ANDROID_SAFE_AREA_BITS: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

#[cfg(target_os = "android")]
pub fn set_android_safe_area_insets(left: f32, top: f32, right: f32, bottom: f32) {
    for (slot, value) in ANDROID_SAFE_AREA_BITS
        .iter()
        .zip([left, top, right, bottom])
    {
        slot.store(value.max(0.0).to_bits(), Ordering::Release);
    }
}

#[cfg(target_os = "android")]
fn update_android_safe_area(mut current: ResMut<SafeAreaInsets>) {
    let values = ANDROID_SAFE_AREA_BITS
        .each_ref()
        .map(|slot| f32::from_bits(slot.load(Ordering::Acquire)));
    let next = SafeAreaInsets {
        left: values[0],
        top: values[1],
        right: values[2],
        bottom: values[3],
    };
    if *current != next {
        info!(
            target: "meteor::ui",
            "android safe-area logical=({:.1},{:.1},{:.1},{:.1})",
            next.left,
            next.top,
            next.right,
            next.bottom,
        );
        *current = next;
    }
}

fn update_adaptive_ui_scale(
    windows: Query<&Window>,
    settings: Res<Settings>,
    mut ui_scale: ResMut<UiScale>,
    mut effective: ResMut<EffectiveUiScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = adaptive_ui_scale(
        window.width(),
        window.height(),
        settings.ui_scale,
        cfg!(target_os = "android"),
    );
    if (scale - effective.0).abs() > 0.001 {
        effective.0 = scale;
        ui_scale.0 = scale;
    }
}

pub struct UiScalePlugin;

impl Plugin for UiScalePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EffectiveUiScale>()
            .init_resource::<SafeAreaInsets>()
            .add_systems(Update, update_adaptive_ui_scale);
        #[cfg(target_os = "android")]
        app.add_systems(First, update_android_safe_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scale_shrinks_but_never_grows() {
        assert_eq!(adaptive_ui_scale(1280.0, 720.0, 0.0, false), 1.0);

        assert_eq!(adaptive_ui_scale(2560.0, 1440.0, 0.0, false), 1.0);

        assert!((adaptive_ui_scale(800.0, 360.0, 0.0, false) - 0.55).abs() < 1e-6);

        assert!((adaptive_ui_scale(854.0, 480.0, 0.0, false) - 480.0 / 720.0).abs() < 1e-6);

        assert_eq!(adaptive_ui_scale(320.0, 240.0, 0.0, false), UI_SCALE_MIN);
    }

    #[test]
    fn android_auto_scale_preserves_readable_logical_size() {
        assert_eq!(adaptive_ui_scale(817.0, 393.0, 0.0, true), 1.0);
        assert_eq!(adaptive_ui_scale(412.0, 915.0, 0.0, true), 1.0);
    }

    #[test]
    fn manual_scale_overrides_and_clamps() {
        assert!((adaptive_ui_scale(817.0, 393.0, 1.2, true) - 1.2).abs() < 1e-6);
        assert!((adaptive_ui_scale(320.0, 240.0, 1.0, false) - 1.0).abs() < 1e-6);

        assert!((adaptive_ui_scale(1280.0, 720.0, 9.9, false) - 1.5).abs() < 1e-6);
        assert_eq!(adaptive_ui_scale(400.0, 200.0, 0.1, false), 0.5);
    }

    #[test]
    fn safe_area_rejects_invalid_or_impossible_platform_values() {
        let safe = SafeAreaInsets {
            left: 80.0,
            top: f32::NAN,
            right: -4.0,
            bottom: 999.0,
        }
        .sanitized(800.0, 400.0);
        assert_eq!(safe.left, 80.0);
        assert_eq!(safe.top, 0.0);
        assert_eq!(safe.right, 0.0);
        assert_eq!(safe.bottom, 180.0);
    }
}
