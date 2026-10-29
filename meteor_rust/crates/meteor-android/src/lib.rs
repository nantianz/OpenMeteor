use bevy::prelude::*;
#[cfg(target_os = "android")]
use std::ffi::c_void;

#[allow(non_snake_case)]
#[no_mangle]
#[cfg(target_os = "android")]
pub extern "system" fn Java_com_meteorweb_game_MainActivity_nativeSetSafeAreaInsets(
    _env: *mut c_void,
    _class: *mut c_void,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    meteor_app::plugins::ui_scale_plugin::set_android_safe_area_insets(left, top, right, bottom);
}

#[bevy_main]
pub fn main() {
    meteor_app::build_app().run();
}
