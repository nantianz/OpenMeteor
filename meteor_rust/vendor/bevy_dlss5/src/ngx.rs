use ash::vk::Handle as _;
use std::ffi::{c_void, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;

pub const FEATURE_DLSSNR: u32 = 18;
const NGX_VERSION_API: u32 = 0x0000015;
pub const NGX_SUCCESS: u32 = 0x1;

type VkH = *const c_void;
type Pfn = *const c_void;

extern "C" {
    fn NVSDK_NGX_VULKAN_Init_with_ProjectID(
        project_id: *const c_char,
        engine_type: u32,
        engine_version: *const c_char,
        app_data_path: *const u16,
        instance: VkH,
        pd: VkH,
        device: VkH,
        gipa: Pfn,
        gdpa: Pfn,
        feature_info: *const c_void,
        sdk_version: u32,
    ) -> u32;
    fn NVSDK_NGX_VULKAN_GetCapabilityParameters(out: *mut *mut c_void) -> u32;
    fn NVSDK_NGX_Parameter_SetUI(p: *mut c_void, n: *const c_char, v: u32);
    fn NVSDK_NGX_Parameter_SetI(p: *mut c_void, n: *const c_char, v: i32);
    fn NVSDK_NGX_Parameter_SetF(p: *mut c_void, n: *const c_char, v: f32);
    fn NVSDK_NGX_Parameter_SetVoidPointer(p: *mut c_void, n: *const c_char, v: *mut c_void);
}

type PfnInitExt2 =
    unsafe extern "C" fn(u64, *const u16, VkH, VkH, VkH, Pfn, Pfn, u32, *const c_void) -> u32;
type PfnCreate1 = unsafe extern "C" fn(VkH, VkH, u32, *const c_void, *mut *mut c_void) -> u32;
type PfnEval = unsafe extern "C" fn(VkH, *const c_void, *const c_void, *const c_void) -> u32;
type PfnRelease = unsafe extern "C" fn(*mut c_void) -> u32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResourceVk {
    view: u64,
    image: u64,
    aspect: u32,
    base_mip: u32,
    level_count: u32,
    base_layer: u32,
    layer_count: u32,
    format: u32,
    width: u32,
    height: u32,
    type_: u32,
    read_write: u32,
}
impl ResourceVk {
    pub fn image_view(
        view: u64,
        image: u64,
        format: u32,
        width: u32,
        height: u32,
        depth_aspect: bool,
        read_write: bool,
    ) -> Self {
        Self {
            view,
            image,
            aspect: if depth_aspect { 0x2 } else { 0x1 },
            base_mip: 0,
            level_count: 1,
            base_layer: 0,
            layer_count: 1,
            format,
            width,
            height,
            type_: 0,
            read_write: read_write as u32,
        }
    }
}

pub struct NgxCore {
    params: *mut c_void,
    _lib: libloading::Library,
    create1: PfnCreate1,
    eval: PfnEval,
    release: PfnRelease,
}

unsafe impl Send for NgxCore {}
unsafe impl Sync for NgxCore {}

#[derive(Clone, Copy)]
pub struct DeviceHandles {
    pub instance: VkH,
    pub physical_device: VkH,
    pub device: VkH,
    pub gipa: Pfn,
    pub gdpa: Pfn,
}

impl NgxCore {
    pub unsafe fn init(project_id: &str, dll: &Path, h: DeviceHandles) -> Result<Self, String> {
        let proj = CString::new(project_id).map_err(|_| "bad project id")?;
        let engine = CString::new(env!("CARGO_PKG_VERSION")).unwrap();
        let tmp = std::env::temp_dir().join("bevy_dlss5_ngx");
        let _ = std::fs::create_dir_all(&tmp);
        let data_path: Vec<u16> = tmp
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let r = NVSDK_NGX_VULKAN_Init_with_ProjectID(
            proj.as_ptr(),
            0,
            engine.as_ptr(),
            data_path.as_ptr(),
            h.instance,
            h.physical_device,
            h.device,
            h.gipa,
            h.gdpa,
            ptr::null(),
            NGX_VERSION_API,
        );
        if r != NGX_SUCCESS {
            return Err(format!("NGX core init failed (0x{r:08X})"));
        }
        let mut params: *mut c_void = ptr::null_mut();
        let _ = NVSDK_NGX_VULKAN_GetCapabilityParameters(&mut params);
        if params.is_null() {
            return Err("NGX returned no capability parameters".into());
        }

        let lib = libloading::Library::new(dll)
            .map_err(|e| format!("could not load DLSSNR DLL '{}': {e}", dll.display()))?;
        let init2: libloading::Symbol<PfnInitExt2> = lib
            .get(b"NVSDK_NGX_VULKAN_Init_Ext2\0")
            .map_err(|e| e.to_string())?;
        let create1: libloading::Symbol<PfnCreate1> = lib
            .get(b"NVSDK_NGX_VULKAN_CreateFeature1\0")
            .map_err(|e| e.to_string())?;
        let eval: libloading::Symbol<PfnEval> = lib
            .get(b"NVSDK_NGX_VULKAN_EvaluateFeature\0")
            .map_err(|e| e.to_string())?;
        let release: libloading::Symbol<PfnRelease> = lib
            .get(b"NVSDK_NGX_VULKAN_ReleaseFeature\0")
            .map_err(|e| e.to_string())?;
        let (create1, eval, release) = (*create1, *eval, *release);

        let ri = (init2)(
            0x1122_3344_5566_7788,
            data_path.as_ptr(),
            h.instance,
            h.physical_device,
            h.device,
            h.gipa,
            h.gdpa,
            NGX_VERSION_API,
            params as *const c_void,
        );
        if ri != NGX_SUCCESS {
            return Err(format!(
                "DLSSNR snippet init failed (0x{ri:08X}); the process executable \
                 must be named `nvngx.dll` (NGX caller check)"
            ));
        }
        Ok(Self {
            params,
            _lib: lib,
            create1,
            eval,
            release,
        })
    }

    pub unsafe fn create_feature(
        &self,
        device: VkH,
        cmdbuf: VkH,
        w: u32,
        h: u32,
        preset: i32,
    ) -> Result<*mut c_void, u32> {
        let set_ui = |n: &str, v: u32| {
            let c = CString::new(n).unwrap();
            NVSDK_NGX_Parameter_SetUI(self.params, c.as_ptr(), v);
        };
        let set_i = |n: &str, v: i32| {
            let c = CString::new(n).unwrap();
            NVSDK_NGX_Parameter_SetI(self.params, c.as_ptr(), v);
        };
        set_ui("CreationNodeMask", 1);
        set_ui("VisibilityNodeMask", 1);
        set_ui("Width", w);
        set_ui("Height", h);
        set_ui("OutWidth", w);
        set_ui("OutHeight", h);
        set_ui("DLSSNR.Width", w);
        set_ui("DLSSNR.Height", h);
        set_ui("DLSSNR.OutputWidth", w);
        set_ui("DLSSNR.OutputHeight", h);
        set_ui("DLSSNR.Enabled", 1);
        set_i("DLSSNR.Hint.Render.Preset", preset);
        let mut handle: *mut c_void = ptr::null_mut();
        let rc = (self.create1)(
            device,
            cmdbuf,
            FEATURE_DLSSNR,
            self.params as *const c_void,
            &mut handle,
        );
        if rc == NGX_SUCCESS && !handle.is_null() {
            Ok(handle)
        } else {
            Err(rc)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn evaluate(
        &self,
        cmdbuf: VkH,
        feature: *mut c_void,
        color: &mut ResourceVk,
        output: &mut ResourceVk,
        depth: &mut ResourceVk,
        mvec: &mut ResourceVk,
        w: u32,
        h: u32,
        intensity: f32,
        tone: f32,
        structure: f32,
        reset: bool,
    ) -> u32 {
        let p = self.params;
        let set_ui = |n: &str, v: u32| {
            let c = CString::new(n).unwrap();
            NVSDK_NGX_Parameter_SetUI(p, c.as_ptr(), v);
        };
        let set_f = |n: &str, v: f32| {
            let c = CString::new(n).unwrap();
            NVSDK_NGX_Parameter_SetF(p, c.as_ptr(), v);
        };
        let set_ptr = |n: &str, v: *mut c_void| {
            let c = CString::new(n).unwrap();
            NVSDK_NGX_Parameter_SetVoidPointer(p, c.as_ptr(), v);
        };
        set_ptr("DLSSNR.Color", color as *mut _ as *mut c_void);
        set_ptr("DLSSNR.Output", output as *mut _ as *mut c_void);
        set_ptr("DLSSNR.Depth", depth as *mut _ as *mut c_void);
        set_ptr("DLSSNR.MVec", mvec as *mut _ as *mut c_void);
        for (n, v) in [
            ("DLSSNR.ColorSubrectWidth", w),
            ("DLSSNR.ColorSubrectHeight", h),
            ("DLSSNR.DepthSubrectWidth", w),
            ("DLSSNR.DepthSubrectHeight", h),
            ("DLSSNR.MVecSubrectWidth", w),
            ("DLSSNR.MVecSubrectHeight", h),
            ("DLSSNR.OutputSubrectWidth", w),
            ("DLSSNR.OutputSubrectHeight", h),
        ] {
            set_ui(n, v);
        }
        set_f("DLSSNR.MVecScaleX", w as f32);
        set_f("DLSSNR.MVecScaleY", h as f32);
        set_ui("DLSSNR.Reset", reset as u32);
        set_ui("DLSSNR.DepthInverted", 1);
        set_ui("DLSSNR.Enabled", 1);
        set_f("DLSSNR.Intensity", intensity);
        set_f("DLSSNR.GlobalToneStrength", tone);
        set_f("DLSSNR.LocalToneStrength", tone);
        set_f("DLSSNR.LocalStructureStrength", structure);
        (self.eval)(
            cmdbuf,
            feature as *const c_void,
            self.params as *const c_void,
            ptr::null(),
        )
    }

    pub unsafe fn release_feature(&self, feature: *mut c_void) {
        let _ = (self.release)(feature);
    }
}

pub unsafe fn device_handles(device: &wgpu::Device) -> Option<DeviceHandles> {
    use wgpu::hal::api::Vulkan;
    let hal = device.as_hal::<Vulkan>()?;
    let shared = hal.shared_instance();
    let inst = shared.raw_instance();
    Some(DeviceHandles {
        instance: inst.handle().as_raw() as usize as VkH,
        physical_device: hal.raw_physical_device().as_raw() as usize as VkH,
        device: hal.raw_device().handle().as_raw() as usize as VkH,
        gipa: shared.entry().static_fn().get_instance_proc_addr as usize as Pfn,
        gdpa: inst.fp_v1_0().get_device_proc_addr as usize as Pfn,
    })
}

pub unsafe fn raw_view(v: &wgpu::TextureView) -> u64 {
    use wgpu::hal::api::Vulkan;
    v.as_hal::<Vulkan>().unwrap().raw_handle().as_raw()
}

pub unsafe fn raw_image(t: &wgpu::Texture) -> u64 {
    use wgpu::hal::api::Vulkan;
    t.as_hal::<Vulkan>().unwrap().raw_handle().as_raw()
}

pub unsafe fn vk_format(adapter: &wgpu::Adapter, t: &wgpu::Texture) -> u32 {
    use wgpu::hal::api::Vulkan;
    adapter
        .as_hal::<Vulkan>()
        .unwrap()
        .texture_format_as_raw(t.format())
        .as_raw() as u32
}

pub fn raw_command_buffer(enc: &mut wgpu::CommandEncoder) -> VkH {
    use wgpu::hal::api::Vulkan;

    unsafe {
        let mut out: VkH = ptr::null();
        enc.as_hal_mut::<Vulkan, _, _>(|e| {
            out = e.unwrap().raw_handle().as_raw() as usize as VkH;
        });
        out
    }
}

pub fn vk_handle(dev: &wgpu::Device) -> VkH {
    unsafe { device_handles(dev).map(|h| h.device).unwrap_or(ptr::null()) }
}
