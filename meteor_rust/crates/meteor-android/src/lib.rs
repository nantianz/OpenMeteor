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

#[allow(non_snake_case)]
#[no_mangle]
#[cfg(target_os = "android")]
pub extern "system" fn Java_com_meteorweb_game_MainActivity_nativeMrcFileSelected(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    name: jni::objects::JString,
    bytes: jni::objects::JByteArray,
    error: jni::objects::JString,
) {
    let result = (|| -> Result<Option<(String, Vec<u8>)>, String> {
        if !error.is_null() {
            return Err(env.get_string(&error).map_err(|e| e.to_string())?.into());
        }
        if bytes.is_null() {
            return Ok(None);
        }
        let name = env.get_string(&name).map_err(|e| e.to_string())?.into();
        let bytes = env.convert_byte_array(&bytes).map_err(|e| e.to_string())?;
        Ok(Some((name, bytes)))
    })();
    meteor_app::plugins::mrc_file_picker::complete_android_selection(result);
}
