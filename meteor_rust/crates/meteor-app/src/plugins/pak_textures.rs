use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::renderer::RenderDevice;
use meteor_asset::pak::PakArchive;
use meteor_asset::texture::{decode_image, decode_image_with_budget, DecodedImage};
use std::collections::{HashMap, TryReserveError};
use std::sync::atomic::{AtomicU32, Ordering};
const MAX_DECODED_BYTES: usize =
    if cfg!(all(target_pointer_width = "64", not(target_os = "android"))) {
        256 * 1024 * 1024
    } else {
        64 * 1024 * 1024
    };
static DEVICE_TEXTURE_SIDE: AtomicU32 = AtomicU32::new(u32::MAX);
pub(super) fn update_texture_device_limit(device: Option<Res<RenderDevice>>) {
    if let Some(device) = device.filter(|device| device.is_changed()) {
        DEVICE_TEXTURE_SIDE.store(device.limits().max_texture_dimension_2d, Ordering::Relaxed);
    }
}
pub(super) fn decode_quality_image(bytes: &[u8]) -> Option<DecodedImage> {
    decode_image_with_budget(bytes, MAX_DECODED_BYTES)
        .map_err(|error| warn!("texture decode failed: {error}"))
        .ok()
}

#[derive(Resource, Default)]
pub struct QualityTextureHandles(pub Vec<Handle<Image>>);

pub fn register_quality_texture(handles: &mut QualityTextureHandles, handle: Handle<Image>) {
    handles.0.push(handle);
}

pub fn sync_quality_texture_samplers(
    settings: Res<crate::app_state::Settings>,
    handles: Res<QualityTextureHandles>,
    mut images: ResMut<Assets<Image>>,
) {
    if !settings.is_changed() {
        return;
    }
    sync_texture_samplers(&mut images, &handles.0, settings.texture_quality);
}

pub fn clear_quality_texture_handles(mut handles: ResMut<QualityTextureHandles>) {
    handles.0.clear();
}

pub fn pak_read_ci(pak: &PakArchive, prefix: &str, name: &str) -> Option<Vec<u8>> {
    let prefixed = format!("{prefix}/{name}");
    if let Some(bytes) = pak.read_entry(&prefixed) {
        return Some(bytes);
    }
    let lower = prefixed.to_lowercase();
    pak.entries()
        .find(|entry| entry.name.to_lowercase() == lower)
        .and_then(|entry| pak.read_entry(&entry.name))
}

fn find_pak_entry(pak: &PakArchive, name: &str) -> Option<Vec<u8>> {
    let wanted = name.replace('\\', "/").to_lowercase();
    if let Some(bytes) = pak.read_entry(&wanted) {
        return Some(bytes);
    }
    let suffix = format!("/{wanted}");
    pak.entries()
        .find(|entry| entry.name.to_lowercase().ends_with(&suffix))
        .and_then(|entry| pak.read_entry(&entry.name))
}

pub fn resolve_pak_texture_exact(paks: &[&PakArchive], name: &str) -> Option<Vec<u8>> {
    let wanted = name.replace('\\', "/");
    let base = wanted.rsplit('/').next().unwrap_or(&wanted);
    paks.iter().find_map(|pak| find_pak_entry(pak, base))
}

pub fn resolve_pak_texture(paks: &[&PakArchive], name: &str) -> Option<Vec<u8>> {
    let wanted = name.replace('\\', "/");
    let base = wanted.rsplit('/').next().unwrap_or(&wanted);
    for pak in paks {
        if let Some(bytes) = find_pak_entry(pak, base) {
            return Some(bytes);
        }
    }
    let lower = base.to_lowercase();
    if lower.ends_with(".ifl") {
        return None;
    }
    let dot = lower.rfind('.');
    let stem = dot.map(|i| &base[..i]).unwrap_or(base);
    let mut best: Option<Vec<u8>> = None;
    for ext in [".bmp", ".jpg", ".jpeg", ".tga", ".dds", ".png", ""] {
        let candidate = format!("{stem}{ext}");
        if candidate.to_lowercase().ends_with(".ifl") {
            continue;
        }
        for pak in paks {
            if let Some(bytes) = find_pak_entry(pak, &candidate) {
                if best
                    .as_ref()
                    .is_none_or(|b: &Vec<u8>| bytes.len() > b.len())
                {
                    best = Some(bytes);
                }
            }
        }
    }
    best
}

pub fn resolve_runtime_texture(scopes: &[&str], name: &str) -> Option<Vec<u8>> {
    let wanted = name.replace('\\', "/");
    let base = wanted.rsplit('/').next().unwrap_or(&wanted);
    for scope in scopes {
        if let Some(bytes) =
            meteor_asset::runtime_assets::read_entry(&format!("texture/{scope}/{base}"))
        {
            return Some(bytes);
        }
    }
    let lower = base.to_lowercase();
    if lower.ends_with(".ifl") {
        return None;
    }
    let stem = lower.rfind('.').map(|index| &base[..index]).unwrap_or(base);
    let mut best: Option<Vec<u8>> = None;
    for ext in [".bmp", ".jpg", ".jpeg", ".tga", ".dds", ".png", ""] {
        for scope in scopes {
            if let Some(bytes) =
                meteor_asset::runtime_assets::read_entry(&format!("texture/{scope}/{stem}{ext}"))
            {
                if best
                    .as_ref()
                    .is_none_or(|current| bytes.len() > current.len())
                {
                    best = Some(bytes);
                }
            }
        }
    }
    best
}

pub fn resolve_runtime_texture_exact(scopes: &[&str], name: &str) -> Option<Vec<u8>> {
    let wanted = name.replace('\\', "/");
    let base = wanted.rsplit('/').next().unwrap_or(&wanted);
    scopes.iter().find_map(|scope| {
        meteor_asset::runtime_assets::read_entry(&format!("texture/{scope}/{base}"))
    })
}

pub fn pak_chain(root: &std::path::Path, scope: &str) -> Vec<PakArchive> {
    let mut names = Vec::new();
    if scope.len() == 4 && scope.starts_with("sn") && scope[2..].chars().all(|c| c.is_ascii_digit())
    {
        names.push(format!("{scope}.pak"));
    }
    names.extend([
        "TexturePatch.pak".to_string(),
        "ctexture.pak".to_string(),
        "ptexture.pak".to_string(),
    ]);
    names
        .into_iter()
        .filter_map(|name| crate::resource_io::read_legacy_pak(root.join(name)))
        .filter_map(|bytes| PakArchive::from_bytes(&bytes).ok())
        .collect()
}

pub fn stage_pak_chain(root: &std::path::Path, stage_id: &str) -> Vec<PakArchive> {
    pak_chain(root, stage_id)
}

pub fn pak_files_root(asset_roots: &[std::path::PathBuf], stage_id: &str) -> std::path::PathBuf {
    for base in asset_roots {
        if base.join(format!("{stage_id}.pak")).is_file() {
            return base.clone();
        }
    }
    for base in asset_roots {
        if base.join("ctexture.pak").is_file() {
            return base.clone();
        }
    }
    asset_roots
        .first()
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

#[derive(Default)]
pub struct PakCache {
    chains: HashMap<String, Vec<PakArchive>>,
}

impl PakCache {
    pub fn chain(&mut self, root: &std::path::Path, scope: &str) -> &[PakArchive] {
        if !self.chains.contains_key(scope) {
            let chain = pak_chain(root, scope);
            self.chains.insert(scope.to_string(), chain);
        }
        self.chains
            .get(scope)
            .expect("chain inserted above")
            .as_slice()
    }

    pub fn resolve(&mut self, root: &std::path::Path, scope: &str, name: &str) -> Option<Vec<u8>> {
        let (effective_scope, file) = texture_scope_and_name(scope, name);
        let runtime_scopes: Vec<&str> = if is_stage_scope(&effective_scope) {
            vec![effective_scope.as_str(), "patch", "ctexture", "ptexture"]
        } else {
            vec!["patch", "ctexture", "ptexture"]
        };
        if let Some(bytes) = resolve_runtime_texture(&runtime_scopes, &file) {
            return Some(bytes);
        }
        let chain = self.chain(root, &effective_scope);
        let refs: Vec<&PakArchive> = chain.iter().collect();
        resolve_pak_texture(&refs, &file)
    }

    pub fn resolve_exact(
        &mut self,
        root: &std::path::Path,
        scope: &str,
        name: &str,
    ) -> Option<Vec<u8>> {
        let (effective_scope, file) = texture_scope_and_name(scope, name);
        let runtime_scopes: Vec<&str> = if is_stage_scope(&effective_scope) {
            vec![effective_scope.as_str(), "patch", "ctexture", "ptexture"]
        } else {
            vec!["patch", "ctexture", "ptexture"]
        };
        if let Some(bytes) = resolve_runtime_texture_exact(&runtime_scopes, &file) {
            return Some(bytes);
        }
        let chain = self.chain(root, &effective_scope);
        let refs: Vec<&PakArchive> = chain.iter().collect();
        resolve_pak_texture_exact(&refs, &file)
    }
}

fn is_stage_scope(scope: &str) -> bool {
    scope.len() == 4
        && scope.starts_with("sn")
        && scope[2..]
            .chars()
            .all(|character| character.is_ascii_digit())
}

fn texture_scope_and_name(scope: &str, name: &str) -> (String, String) {
    let normalized = name.replace('\\', "/");
    if let Some((prefix, rest)) = normalized.split_once('/') {
        let prefix = prefix.to_ascii_lowercase();
        if is_stage_scope(&prefix) && !rest.is_empty() {
            return (prefix, rest.to_string());
        }
    }
    (scope.to_ascii_lowercase(), normalized)
}

pub fn image_handle(images: &mut Assets<Image>, bytes: &[u8]) -> Option<Handle<Image>> {
    let decoded = decode_image(bytes).ok()?;
    Some(image_from_decoded(images, &decoded))
}

pub fn image_handle_with_quality(
    images: &mut Assets<Image>,
    bytes: &[u8],
    texture_quality: u8,
) -> Option<Handle<Image>> {
    image_from_decoded_with_quality(images, decode_quality_image(bytes)?, texture_quality)
}

pub fn legacy_alpha_companion_name(name: &str) -> Option<String> {
    let file = name.replace('\\', "/");
    let file = file.rsplit('/').next()?;
    let (stem, extension) = file.rsplit_once('.')?;
    ((stem.to_ascii_lowercase().starts_with('a') || stem.to_ascii_lowercase().starts_with("km"))
        && matches!(extension.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
    .then(|| format!("{stem}.bmp"))
}

fn apply_legacy_alpha(
    color: &mut meteor_asset::texture::DecodedImage,
    companion: Option<&meteor_asset::texture::DecodedImage>,
    luminance_fallback: bool,
) {
    if let Some(mask) = companion {
        if mask.width == 0 || mask.height == 0 {
            return;
        }

        for y in 0..color.height {
            let mask_y = (u64::from(y) * u64::from(mask.height) / u64::from(color.height)) as u32;
            for x in 0..color.width {
                let mask_x = (u64::from(x) * u64::from(mask.width) / u64::from(color.width)) as u32;
                let color_index = ((y * color.width + x) * 4) as usize;
                let mask_index = ((mask_y * mask.width + mask_x) * 4) as usize;
                color.rgba[color_index + 3] = mask.rgba[mask_index];
            }
        }
    } else if luminance_fallback {
        for pixel in color.rgba.chunks_exact_mut(4) {
            pixel[3] = (0.299 * f32::from(pixel[0])
                + 0.587 * f32::from(pixel[1])
                + 0.114 * f32::from(pixel[2]))
            .round()
            .clamp(0.0, 255.0) as u8;
        }
    }
}

pub fn punch_color_key(rgba: &mut [u8], threshold: u8, band: u8) {
    if rgba.len() < 4 {
        return;
    }
    let (key_r, key_g, key_b) = (rgba[0], rgba[1], rgba[2]);
    for pixel in rgba.chunks_exact_mut(4) {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        let near_black = r <= threshold && g <= threshold && b <= threshold;
        let near_key =
            r.abs_diff(key_r) <= band && g.abs_diff(key_g) <= band && b.abs_diff(key_b) <= band;
        if near_black || near_key {
            pixel[3] = 0;
        }
    }
}

pub fn image_handle_with_legacy_alpha(
    images: &mut Assets<Image>,
    color_bytes: &[u8],
    companion_bytes: Option<&[u8]>,
    luminance_fallback: bool,
) -> Option<Handle<Image>> {
    image_handle_with_legacy_style(
        images,
        color_bytes,
        companion_bytes,
        luminance_fallback,
        false,
    )
}

pub fn image_handle_with_legacy_style(
    images: &mut Assets<Image>,
    color_bytes: &[u8],
    companion_bytes: Option<&[u8]>,
    luminance_fallback: bool,
    color_key: bool,
) -> Option<Handle<Image>> {
    let color = decode_legacy_style(color_bytes, companion_bytes, luminance_fallback, color_key)?;
    Some(image_from_decoded(images, &color))
}

pub(super) fn decode_legacy_style(
    color_bytes: &[u8],
    companion_bytes: Option<&[u8]>,
    luminance_fallback: bool,
    color_key: bool,
) -> Option<meteor_asset::texture::DecodedImage> {
    let mut color = decode_quality_image(color_bytes)?;
    let companion = companion_bytes.and_then(decode_quality_image);
    apply_legacy_alpha(&mut color, companion.as_ref(), luminance_fallback);
    if color_key && companion.is_none() {
        punch_color_key(&mut color.rgba, 8, 26);
    }
    Some(color)
}

pub(crate) fn hd_image_handle_with_legacy_style_quality(
    images: &mut Assets<Image>,
    hd_bytes: &[u8],
    original_bytes: Option<&[u8]>,
    companion_bytes: Option<&[u8]>,
    luminance_fallback: bool,
    color_key: bool,
    quality: u8,
) -> Option<Handle<Image>> {
    let mut hd = decode_quality_image(hd_bytes)?;
    if let Some(bytes) = original_bytes {
        let original = decode_legacy_style(bytes, companion_bytes, luminance_fallback, color_key)?;
        for y in 0..hd.height {
            let source_y =
                (u64::from(y) * u64::from(original.height) / u64::from(hd.height)) as u32;
            for x in 0..hd.width {
                let source_x =
                    (u64::from(x) * u64::from(original.width) / u64::from(hd.width)) as u32;
                hd.rgba[((y * hd.width + x) * 4 + 3) as usize] =
                    original.rgba[((source_y * original.width + source_x) * 4 + 3) as usize];
            }
        }
    } else if companion_bytes.is_some() || luminance_fallback || color_key {
        return None;
    }
    image_from_decoded_with_quality(images, hd, quality)
}

pub fn image_handle_with_legacy_style_quality(
    images: &mut Assets<Image>,
    color_bytes: &[u8],
    companion_bytes: Option<&[u8]>,
    luminance_fallback: bool,
    color_key: bool,
    texture_quality: u8,
) -> Option<Handle<Image>> {
    let color = decode_legacy_style(color_bytes, companion_bytes, luminance_fallback, color_key)?;
    image_from_decoded_with_quality(images, color, texture_quality)
}

pub fn image_from_decoded(
    images: &mut Assets<Image>,
    decoded: &meteor_asset::texture::DecodedImage,
) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: decoded.width,
            height: decoded.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        decoded.rgba.clone(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    images.add(image)
}

pub fn apply_texture_quality(image: &mut Image, texture_quality: u8) {
    image.sampler = texture_quality_sampler(texture_quality);
}
pub(super) fn sync_texture_samplers(
    images: &mut Assets<Image>,
    handles: &[Handle<Image>],
    texture_quality: u8,
) {
    let sampler = texture_quality_sampler(texture_quality);
    for handle in handles {
        if images
            .get(handle)
            .is_none_or(|image| image.sampler == sampler)
        {
            continue;
        }
        if let Some(mut image) = images.get_mut(handle) {
            image.sampler = sampler.clone();
        }
    }
}
fn texture_quality_sampler(texture_quality: u8) -> ImageSampler {
    let original_point = texture_quality == 2;
    let filter = if original_point {
        ImageFilterMode::Nearest
    } else {
        ImageFilterMode::Linear
    };
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: filter,
        lod_max_clamp: if original_point { 0.0 } else { 32.0 },
        anisotropy_clamp: if original_point {
            1
        } else if texture_quality == 1 {
            2
        } else {
            16
        },
        ..Default::default()
    })
}

pub fn image_from_decoded_with_quality(
    images: &mut Assets<Image>,
    decoded: DecodedImage,
    texture_quality: u8,
) -> Option<Handle<Image>> {
    let max_side = texture_device_side();
    if decoded.width.max(decoded.height) > max_side {
        info!(
            "texture {}x{} exceeds device limit {max_side}; using a smaller mip",
            decoded.width, decoded.height
        );
    }
    Some(images.add(prepare_quality_image(decoded, texture_quality, max_side)?))
}
pub(super) fn texture_device_side() -> u32 {
    DEVICE_TEXTURE_SIDE.load(Ordering::Relaxed).max(1)
}
pub(super) fn prepare_quality_image(
    decoded: DecodedImage,
    texture_quality: u8,
    max_side: u32,
) -> Option<Image> {
    let decoded = fit_to_texture_limit(decoded, max_side)
        .map_err(|error| warn!("texture resize failed: {error}"))
        .ok()?;
    let mut image = Image::new_uninit(
        Extent3d {
            width: decoded.width,
            height: decoded.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    let (data, mip_level_count) = rgba8_mip_chain(decoded)
        .map_err(|error| warn!("texture mip allocation failed: {error}"))
        .ok()?;
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mip_level_count;
    apply_texture_quality(&mut image, texture_quality);
    Some(image)
}

fn fit_to_texture_limit(
    mut image: DecodedImage,
    max_side: u32,
) -> Result<DecodedImage, TryReserveError> {
    while image.width.max(image.height) > max_side {
        let width = (image.width / 2).max(1);
        let height = (image.height / 2).max(1);
        let len = width as usize * height as usize * 4;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(len)?;
        rgba.resize(len, 0);
        downsample_rgba8(&image.rgba, image.width, image.height, &mut rgba);
        image = DecodedImage {
            width,
            height,
            rgba,
        };
    }
    Ok(image)
}
fn rgba8_mip_chain(decoded: DecodedImage) -> Result<(Vec<u8>, u32), TryReserveError> {
    let DecodedImage {
        mut width,
        mut height,
        rgba: mut data,
    } = decoded;
    let levels = u32::BITS - width.max(height).leading_zeros();
    let extra_bytes = (1..levels)
        .map(|level| (width >> level).max(1) as usize * (height >> level).max(1) as usize * 4)
        .sum();
    data.try_reserve_exact(extra_bytes)?;
    let mut offset = 0;
    for _ in 1..levels {
        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let end = data.len();
        data.resize(end + next_width as usize * next_height as usize * 4, 0);
        let (written, next) = data.split_at_mut(end);
        downsample_rgba8(&written[offset..], width, height, next);
        offset = end;
        width = next_width;
        height = next_height;
    }
    Ok((data, levels))
}
fn downsample_rgba8(source: &[u8], width: u32, height: u32, next: &mut [u8]) {
    let width = width as usize;
    let height = height as usize;
    let stride = width * 4;
    let next_stride = (width / 2).max(1) * 4;
    for (y, row) in next.chunks_exact_mut(next_stride).enumerate() {
        let top = y * 2 * stride;
        let bottom = (y * 2 + 1).min(height - 1) * stride;
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let left = x * 2 * 4;
            let right = (x * 2 + 1).min(width - 1) * 4;
            for (channel, value) in pixel.iter_mut().enumerate() {
                let sum = u16::from(source[top + left + channel])
                    + u16::from(source[top + right + channel])
                    + u16::from(source[bottom + left + channel])
                    + u16::from(source[bottom + right + channel]);
                *value = ((sum + 2) / 4) as u8;
            }
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    fn zip_pak(entries: &[(&str, &[u8])]) -> PakArchive {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (name, bytes) in entries {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .expect("start entry");
                zip.write_all(bytes).expect("write entry");
            }
            zip.finish().expect("finish zip");
        }
        PakArchive::from_bytes(&buf).expect("parse pak")
    }

    #[test]
    fn named_alpha_companion_matches_original_a_jpeg_rule() {
        assert_eq!(
            legacy_alpha_companion_name("sn06/Asn06wu.jpg").as_deref(),
            Some("Asn06wu.bmp")
        );
        assert_eq!(
            legacy_alpha_companion_name(r"sn06\Asn06wu1.JPEG").as_deref(),
            Some("Asn06wu1.bmp")
        );
        assert_eq!(legacy_alpha_companion_name("sn06zhu2.jpg"), None);
        assert_eq!(legacy_alpha_companion_name("alpha.tga"), None);
    }

    #[test]
    fn companion_red_channel_replaces_color_alpha() {
        let mut color = meteor_asset::texture::DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![240, 230, 220, 255, 10, 20, 30, 255],
        };
        let mask = meteor_asset::texture::DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![17, 17, 17, 255, 203, 203, 203, 255],
        };
        apply_legacy_alpha(&mut color, Some(&mask), false);
        assert_eq!(color.rgba, vec![240, 230, 220, 17, 10, 20, 30, 203]);
    }

    #[test]
    fn alpha_channel_luminance_fallback_keeps_rgb() {
        let mut color = meteor_asset::texture::DecodedImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        apply_legacy_alpha(&mut color, None, true);
        assert_eq!(color.rgba, vec![0, 0, 0, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn rgba8_mips_average_in_authored_byte_space() {
        let decoded = meteor_asset::texture::DecodedImage {
            width: 2,
            height: 2,
            rgba: vec![
                0, 10, 20, 30, 100, 110, 120, 130, 200, 210, 220, 230, 252, 250, 248, 246,
            ],
        };
        let (data, levels) = rgba8_mip_chain(decoded).unwrap();
        assert_eq!(levels, 2);
        assert_eq!(data.len(), 20);
        assert_eq!(&data[16..20], &[138, 145, 152, 159]);
    }
    #[test]
    fn oversized_texture_uses_its_device_compatible_mip_suffix() {
        let decoded = DecodedImage {
            width: 7,
            height: 3,
            rgba: (0..84).collect(),
        };
        let (full, _) = rgba8_mip_chain(decoded.clone()).unwrap();
        let fitted = fit_to_texture_limit(decoded, 3).unwrap();
        assert_eq!((fitted.width, fitted.height), (3, 1));
        let (data, levels) = rgba8_mip_chain(fitted).unwrap();
        assert_eq!(levels, 2);
        assert_eq!(data, full[84..]);
    }
    #[cfg(all(target_pointer_width = "64", not(target_os = "android")))]
    #[test]
    fn large_scene_tga_keeps_8k_mip_chain() {
        let size = 8192u16;
        let mut tga = vec![0; 18];
        tga[2] = 10;
        tga[12..14].copy_from_slice(&size.to_le_bytes());
        tga[14..16].copy_from_slice(&size.to_le_bytes());
        tga[16] = 32;
        tga[17] = 8;
        let pixel = [30, 20, 10, 192];
        for _ in 0..usize::from(size).pow(2) / 128 {
            tga.push(255);
            tga.extend_from_slice(&pixel);
        }
        let image = decode_quality_image(&tga).expect("8K desktop decode");
        let (mips, levels) = rgba8_mip_chain(image).unwrap();
        let expected_len: usize = (0..14).map(|level| (8192usize >> level).pow(2) * 4).sum();
        assert_eq!(levels, 14);
        assert_eq!(mips.len(), expected_len);
        assert_eq!(&mips[..4], &[10, 20, 30, 192]);
        assert_eq!(&mips[mips.len() - 4..], &[10, 20, 30, 192]);
    }

    #[test]
    fn texture_quality_maps_to_web_sampler_profiles() {
        for (quality, filter, anisotropy, lod_max) in [
            (0, ImageFilterMode::Linear, 16, 32.0),
            (1, ImageFilterMode::Linear, 2, 32.0),
            (2, ImageFilterMode::Nearest, 1, 0.0),
        ] {
            let mut image = Image::default();
            apply_texture_quality(&mut image, quality);
            let ImageSampler::Descriptor(sampler) = &image.sampler else {
                panic!("quality sampler must be explicit");
            };
            assert_eq!(sampler.mag_filter, filter);
            assert_eq!(sampler.min_filter, filter);
            assert_eq!(sampler.mipmap_filter, filter);
            assert_eq!(sampler.anisotropy_clamp, anisotropy);
            assert_eq!(sampler.lod_max_clamp, lod_max);
        }
    }

    #[test]
    fn read_ci_matches_mixed_case() {
        let pak = zip_pak(&[("ctexture/An07y1.BMP", b"data")]);
        assert_eq!(
            pak_read_ci(&pak, "ctexture", "an07y1.bmp").as_deref(),
            Some(b"data".as_slice())
        );
        assert_eq!(
            pak_read_ci(&pak, "ctexture", "An07y1.BMP").as_deref(),
            Some(b"data".as_slice())
        );
        assert_eq!(pak_read_ci(&pak, "ctexture", "missing.bmp"), None);
    }

    #[test]
    fn read_ci_requires_prefix() {
        let pak = zip_pak(&[("ctexture/x.bmp", b"data")]);
        assert_eq!(pak_read_ci(&pak, "ptexture", "x.bmp"), None);
    }

    #[test]
    fn resolve_exact_basename_across_dirs() {
        let stage = zip_pak(&[("sn01/sn01z1.jpg", b"stage")]);
        let chain = [&stage];
        assert_eq!(
            resolve_pak_texture(&chain, "sn01z1.jpg").as_deref(),
            Some(b"stage".as_slice())
        );

        assert_eq!(
            resolve_pak_texture(&chain, "sn01/sn01z1.jpg").as_deref(),
            Some(b"stage".as_slice())
        );
    }

    #[test]
    fn resolve_chain_order_prefers_scope_pak() {
        let stage = zip_pak(&[("sn01/x.bmp", b"stage")]);
        let ctex = zip_pak(&[("ctexture/x.bmp", b"ctexture")]);
        let chain = [&stage, &ctex];
        assert_eq!(
            resolve_pak_texture(&chain, "x.bmp").as_deref(),
            Some(b"stage".as_slice())
        );
    }

    #[test]
    fn resolve_fallback_to_ctexture() {
        let stage = zip_pak(&[("sn01/y.bmp", b"stage")]);
        let ctex = zip_pak(&[("ctexture/x.bmp", b"ctexture")]);
        let chain = [&stage, &ctex];
        assert_eq!(
            resolve_pak_texture(&chain, "x.bmp").as_deref(),
            Some(b"ctexture".as_slice())
        );
    }

    #[test]
    fn resolve_same_stem_other_extension() {
        let pak = zip_pak(&[("sn01/logo.bmp", b"bmp-art")]);
        let chain = [&pak];
        assert_eq!(
            resolve_pak_texture(&chain, "logo.jpg").as_deref(),
            Some(b"bmp-art".as_slice())
        );
    }

    #[test]
    fn exact_resolve_never_substitutes_same_stem_extension() {
        let pak = zip_pak(&[("sn01/fog.jpg", b"color")]);
        let chain = [&pak];
        assert_eq!(
            resolve_pak_texture_exact(&chain, "fog.jpg").as_deref(),
            Some(b"color".as_slice())
        );
        assert_eq!(resolve_pak_texture_exact(&chain, "fog.bmp"), None);
    }

    #[test]
    fn scoped_texture_name_selects_source_stage() {
        assert_eq!(
            texture_scope_and_name("sn01", r"sn03\tr.JPG"),
            ("sn03".to_string(), "tr.JPG".to_string())
        );
        assert_eq!(
            texture_scope_and_name("sn01", "plain.jpg"),
            ("sn01".to_string(), "plain.jpg".to_string())
        );
    }

    #[test]
    fn resolve_never_substitutes_ifl() {
        let pak = zip_pak(&[
            ("sn01/fire.ifl", b"frame1.jpg\nframe2.jpg"),
            ("sn01/fire.jpg", b"bitmap"),
        ]);
        let chain = [&pak];

        assert_eq!(
            resolve_pak_texture(&chain, "fire.jpg").as_deref(),
            Some(b"bitmap".as_slice())
        );

        assert_eq!(
            resolve_pak_texture(&chain, "fire.ifl").as_deref(),
            Some(b"frame1.jpg\nframe2.jpg".as_slice())
        );
    }

    #[test]
    fn resolve_strict_first_pack_wins() {
        let a = zip_pak(&[("ctexture/art.jpg", b"small")]);
        let b = zip_pak(&[("ctexture/art.jpg", b"much-larger-art")]);
        let chain = [&a, &b];
        assert_eq!(
            resolve_pak_texture(&chain, "art.jpg").as_deref(),
            Some(b"small".as_slice())
        );
    }

    #[test]
    fn resolve_same_stem_prefers_larger_payload() {
        let a = zip_pak(&[("ctexture/art.bmp", b"small")]);
        let b = zip_pak(&[("ctexture/art.jpg", b"much-larger-art")]);
        let chain = [&a, &b];
        assert_eq!(
            resolve_pak_texture(&chain, "art.png").as_deref(),
            Some(b"much-larger-art".as_slice())
        );
    }

    #[test]
    fn pak_files_root_prefers_origin_res_over_workdir() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("sn06.pak").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let workdir = origin.parent().unwrap().join("meteor_rust");
        let roots = vec![workdir, origin.clone()];
        assert_eq!(pak_files_root(&roots, "sn06"), origin);
    }
}
