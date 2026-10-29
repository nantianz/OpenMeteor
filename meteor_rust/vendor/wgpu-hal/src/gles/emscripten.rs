extern "C" {

    fn emscripten_webgl_enable_extension(
        context: core::ffi::c_int,
        extension: *const core::ffi::c_char,
    ) -> core::ffi::c_int;
    fn emscripten_webgl_get_current_context() -> core::ffi::c_int;
}

pub unsafe fn enable_extension(extension_name_null_terminated: &str) -> bool {
    unsafe {
        emscripten_webgl_enable_extension(
            emscripten_webgl_get_current_context(),
            extension_name_null_terminated.as_ptr().cast(),
        ) == 1
    }
}
