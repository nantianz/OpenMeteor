use crate::app_state::{Settings, SettingsSelectField};
use std::sync::OnceLock;

#[cfg(all(test, target_os = "windows", feature = "angle-dx11"))]
mod angle_tests;

pub(crate) const ANGLE_DX11: bool = cfg!(all(target_os = "windows", feature = "angle-dx11"));
pub(crate) const GL_LABEL: &str = if ANGLE_DX11 { "ANGLE / DX11" } else { "OpenGL" };

pub(crate) fn backend_label(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => GL_LABEL,
        _ => "",
    }
}

#[derive(bevy::prelude::Component)]
pub(crate) struct CurrentRendererLabel;

pub(crate) fn update_current_renderer(
    adapter: Option<bevy::prelude::Res<bevy::render::renderer::RenderAdapterInfo>>,
    mut labels: bevy::prelude::Query<
        &mut bevy::prelude::Text,
        bevy::prelude::With<CurrentRendererLabel>,
    >,
) {
    let Some(adapter) = adapter else {
        return;
    };
    for mut text in &mut labels {
        let label = format!(
            "当前：{} / {}",
            adapter.name,
            backend_label(adapter.backend)
        );
        if text.0 != label {
            text.0 = label;
        }
    }
}

pub(crate) const AVAILABLE: bool = !cfg!(any(target_arch = "wasm32", target_os = "android"));

#[derive(Default)]
struct Catalog {
    adapters: Vec<(String, String)>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapters =
                bevy::tasks::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
            let mut entries: Vec<_> = adapters
                .into_iter()
                .filter_map(|adapter| {
                    let info = adapter.get_info();
                    let backend = backend_label(info.backend);
                    if backend.is_empty() {
                        return None;
                    }
                    Some((info.name, backend.to_owned()))
                })
                .collect();
            entries.sort();
            entries.dedup();
            Catalog { adapters: entries }
        }
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        Catalog::default()
    })
}

pub(crate) fn options(field: SettingsSelectField) -> Vec<(f32, String)> {
    let mut names: Vec<String> = catalog()
        .adapters
        .iter()
        .map(|(name, backend)| {
            if field == SettingsSelectField::GraphicsAdapter {
                name.clone()
            } else {
                backend.clone()
            }
        })
        .collect();
    names.sort();
    if field == SettingsSelectField::GraphicsBackend {
        names.extend(
            if cfg!(target_os = "windows") {
                vec!["Vulkan", "DX12", GL_LABEL]
            } else if cfg!(target_os = "macos") {
                vec!["Metal", "Vulkan", "OpenGL"]
            } else {
                vec!["Vulkan", "OpenGL"]
            }
            .into_iter()
            .map(str::to_owned),
        );
        names.sort();
    }
    names.dedup();
    std::iter::once((0.0, "自动".into()))
        .chain(
            names
                .into_iter()
                .enumerate()
                .map(|(i, name)| ((i + 1) as f32, name)),
        )
        .collect()
}

pub(crate) fn value(settings: &Settings, field: SettingsSelectField) -> &str {
    if field == SettingsSelectField::GraphicsAdapter {
        &settings.graphics_adapter
    } else {
        &settings.graphics_backend
    }
}

pub(crate) fn label(settings: &Settings, field: SettingsSelectField) -> String {
    let value = value(settings, field);
    if value.is_empty() {
        "自动".into()
    } else {
        value.into()
    }
}

pub(crate) fn selected(settings: &Settings, field: SettingsSelectField) -> f32 {
    let value = value(settings, field);
    if value.is_empty() {
        return 0.0;
    }
    options(field)
        .iter()
        .find(|(_, name)| name == value)
        .map_or(-1.0, |(id, _)| *id)
}

pub(crate) fn select(settings: &mut Settings, field: SettingsSelectField, id: f32) {
    if unavailable_reason(settings, field, id).is_some() {
        return;
    }
    let Some((_, mut name)) = options(field).into_iter().find(|(value, _)| *value == id) else {
        return;
    };
    if id == 0.0 {
        name.clear();
    }
    if field == SettingsSelectField::GraphicsAdapter {
        settings.graphics_adapter = name;
    } else {
        settings.graphics_backend = name;
    }
}

pub(crate) fn unavailable_reason(
    settings: &Settings,
    field: SettingsSelectField,
    id: f32,
) -> Option<&'static str> {
    if id == 0.0 {
        return None;
    }
    let Some((_, name)) = options(field).into_iter().find(|(value, _)| *value == id) else {
        return Some("未检测到设备或后端");
    };
    availability(settings, field, &name, &catalog().adapters)
}

fn availability(
    settings: &Settings,
    field: SettingsSelectField,
    name: &str,
    adapters: &[(String, String)],
) -> Option<&'static str> {
    let is_adapter = field == SettingsSelectField::GraphicsAdapter;
    if !is_adapter && name == "ANGLE / DX11" && !ANGLE_DX11 {
        return Some("需要启用 angle-dx11 的 Windows 构建");
    }
    if !adapters.iter().any(|(adapter, backend)| {
        if is_adapter {
            adapter == name
        } else {
            backend == name
        }
    }) {
        if !is_adapter && name == "ANGLE / DX11" && ANGLE_DX11 {
            return Some("ANGLE 运行库缺失或 D3D11 初始化失败，请检查 console.log");
        }
        return Some("未检测到设备或后端");
    }
    let compatible = adapters.iter().any(|(adapter, backend)| {
        if is_adapter {
            adapter == name
                && (settings.graphics_backend.is_empty() || backend == &settings.graphics_backend)
        } else {
            backend == name
                && (settings.graphics_adapter.is_empty() || adapter == &settings.graphics_adapter)
        }
    });
    if compatible {
        None
    } else {
        Some("与另一项选择不兼容，请先将另一项设为自动")
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub(crate) fn renderer_settings(settings: &Settings) -> bevy::render::settings::WgpuSettings {
    resolve(settings, &catalog().adapters)
}

#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
fn resolve(
    settings: &Settings,
    adapters: &[(String, String)],
) -> bevy::render::settings::WgpuSettings {
    use bevy::render::settings::{Backends, WgpuSettings};
    let mut result = WgpuSettings::default();
    let backend = settings.graphics_backend.as_str();
    if adapters.iter().any(|(_, available)| available == backend) {
        result.backends = match backend {
            "Vulkan" => Some(Backends::VULKAN),
            "DX12" => Some(Backends::DX12),
            "Metal" => Some(Backends::METAL),
            name if name == GL_LABEL => Some(Backends::GL),
            _ => result.backends,
        };
    }
    if !settings.graphics_adapter.is_empty()
        && adapters.iter().any(|(name, available)| {
            name == &settings.graphics_adapter && (backend.is_empty() || available == backend)
        })
    {
        result.adapter_name = Some(settings.graphics_adapter.clone());
    }
    if angle_may_be_selected(&result) {
        result.gles3_minor_version = wgpu::Gles3MinorVersion::Version0;
        let mut limits = result.limits.clone();
        limits.max_storage_buffers_per_shader_stage = 0;
        result.constrained_limits = Some(limits);
    }
    result
}

#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
pub(crate) fn angle_may_be_selected(settings: &bevy::render::settings::WgpuSettings) -> bool {
    ANGLE_DX11
        && settings
            .backends
            .unwrap_or(wgpu::Backends::all())
            .contains(wgpu::Backends::GL)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gl_backend_label_and_compatibility_limits_match_the_build() {
        let adapters = vec![("GL GPU".into(), GL_LABEL.into())];
        let settings = Settings {
            graphics_backend: GL_LABEL.into(),
            ..Default::default()
        };
        let resolved = resolve(&settings, &adapters);
        assert_eq!(resolved.backends, Some(wgpu::Backends::GL));
        assert_eq!(backend_label(wgpu::Backend::Gl), GL_LABEL);
        assert_eq!(angle_may_be_selected(&resolved), ANGLE_DX11);
        if ANGLE_DX11 {
            assert_eq!(
                resolved.gles3_minor_version,
                wgpu::Gles3MinorVersion::Version0
            );
            assert_eq!(
                resolved
                    .constrained_limits
                    .unwrap()
                    .max_storage_buffers_per_shader_stage,
                0
            );
        }
        let native = resolve(
            &Settings {
                graphics_backend: "DX12".into(),
                ..Default::default()
            },
            &[("GPU".into(), "DX12".into())],
        );
        assert!(!angle_may_be_selected(&native));
        assert!(native.constrained_limits.is_none());
    }
    #[test]
    fn graphics_choices_reject_missing_or_incompatible_combinations() {
        let adapters = vec![
            ("GPU A".into(), "Vulkan".into()),
            ("GPU B".into(), "DX12".into()),
        ];
        let mut settings = Settings {
            graphics_adapter: "GPU A".into(),
            graphics_backend: "Vulkan".into(),
            ..Default::default()
        };
        assert!(availability(
            &settings,
            SettingsSelectField::GraphicsAdapter,
            "GPU A",
            &adapters
        )
        .is_none());
        assert!(availability(
            &settings,
            SettingsSelectField::GraphicsAdapter,
            "GPU B",
            &adapters
        )
        .is_some());
        assert!(availability(
            &settings,
            SettingsSelectField::GraphicsBackend,
            "DX12",
            &adapters
        )
        .is_some());
        assert!(availability(
            &settings,
            SettingsSelectField::GraphicsBackend,
            "OpenGL",
            &adapters
        )
        .is_some());
        settings.graphics_adapter.clear();
        assert!(availability(
            &settings,
            SettingsSelectField::GraphicsBackend,
            "DX12",
            &adapters
        )
        .is_none());
    }
    #[test]
    fn renderer_preferences_select_backend_and_adapter_and_handle_missing_hardware() {
        let adapters = vec![("Test GPU".into(), "Vulkan".into())];
        let mut settings = Settings {
            graphics_adapter: "Test GPU".into(),
            graphics_backend: "Vulkan".into(),
            ..Default::default()
        };
        let resolved = resolve(&settings, &adapters);
        assert_eq!(resolved.backends, Some(wgpu::Backends::VULKAN));
        assert_eq!(resolved.adapter_name.as_deref(), Some("Test GPU"));
        settings.graphics_adapter = "Removed GPU".into();
        assert!(resolve(&settings, &adapters).adapter_name.is_none());
        settings.graphics_backend = "invalid".into();
        assert_eq!(
            resolve(&settings, &adapters).backends,
            bevy::render::settings::WgpuSettings::default().backends
        );
    }
}
