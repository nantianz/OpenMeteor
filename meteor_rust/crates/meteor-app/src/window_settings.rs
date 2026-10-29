#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
use crate::app_state::Settings;
#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
use bevy::prelude::*;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use bevy::window::PrimaryWindow;
#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
use bevy::window::{MonitorSelection, WindowMode};

const RESOLUTIONS: &[(u32, u32, &str)] = &[
    (1280, 720, "1280x720"),
    (1024, 768, "1024x768"),
    (1366, 768, "1366x768"),
    (1600, 900, "1600x900"),
    (1920, 1080, "1920x1080"),
    (2560, 1440, "2560x1440"),
    (3840, 2160, "3840x2160"),
];

pub(crate) fn resolution_label(index: u8) -> &'static str {
    RESOLUTIONS.get(index as usize).unwrap_or(&RESOLUTIONS[0]).2
}

pub(crate) fn parse_resolution(value: &str) -> u8 {
    RESOLUTIONS
        .iter()
        .position(|entry| entry.2 == value)
        .unwrap_or(0) as u8
}

pub(crate) fn resolution_options() -> Vec<(f32, String)> {
    RESOLUTIONS
        .iter()
        .enumerate()
        .map(|(i, entry)| (i as f32, entry.2.into()))
        .collect()
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(crate) fn sync_window_settings(
    settings: Res<Settings>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut applied: Local<Option<(u8, bool, bool)>>,
) {
    let requested = (
        settings.window_resolution,
        settings.fullscreen,
        settings.borderless_window,
    );
    if *applied == Some(requested) {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    apply(&mut window, &settings);
    *applied = Some(requested);
}

#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
fn apply(window: &mut Window, settings: &Settings) {
    window.decorations = !settings.borderless_window;
    window.mode = if settings.fullscreen {
        WindowMode::BorderlessFullscreen(MonitorSelection::Current)
    } else {
        let &(width, height, _) = RESOLUTIONS
            .get(settings.window_resolution as usize)
            .unwrap_or(&RESOLUTIONS[0]);
        window.resolution.set_physical_resolution(width, height);
        WindowMode::Windowed
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_settings_restore_resolution_after_fullscreen_and_keep_border_preference() {
        let mut window = Window::default();
        let mut settings = Settings {
            window_resolution: 4,
            borderless_window: true,
            ..Default::default()
        };
        apply(&mut window, &settings);
        assert_eq!(window.resolution.physical_size(), UVec2::new(1920, 1080));
        assert!(!window.decorations);
        settings.fullscreen = true;
        apply(&mut window, &settings);
        assert_eq!(
            window.mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        );
        window.resolution.set_physical_resolution(2560, 1440);
        settings.fullscreen = false;
        apply(&mut window, &settings);
        assert_eq!(window.mode, WindowMode::Windowed);
        assert_eq!(window.resolution.physical_size(), UVec2::new(1920, 1080));
        assert!(!window.decorations);
        assert_eq!(parse_resolution("invalid"), 0);
        assert_eq!(parse_resolution("1920x1080"), 4);
    }
}
