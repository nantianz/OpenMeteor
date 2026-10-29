#define_import_path bevy_render::globals

struct Globals {

    time: f32,

    delta_time: f32,

    frame_count: u32,
#ifdef NATIVE_GLES_GLOBALS_ALIGNMENT
    _native_gles_padding: f32,
#endif
#ifdef SIXTEEN_BYTE_ALIGNMENT

    _webgl2_padding: f32
#endif
};
