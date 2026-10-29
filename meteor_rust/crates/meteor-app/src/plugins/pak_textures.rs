use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use meteor_asset::pak::PakArchive;
use meteor_asset::texture::decode_image;
use std::collections::HashMap;

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
        .filter_map(|name| crate::resource_io::read(root.join(&name)).ok())
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

    pub fn resolve_mask(
        &mut self,
        root: &std::path::Path,
        scope: &str,
        name: &str,
    ) -> Option<(String, Vec<u8>)> {
        let (effective_scope, file) = texture_scope_and_name(scope, name);
        let stem = file
            .rsplit_once('.')
            .map_or(file.as_str(), |(stem, _)| stem);
        let candidates = [format!("k{stem}.bmp"), format!("_k{stem}.bmp")];

        let runtime_mask = meteor_asset::runtime_assets::read_entry(&format!(
            "texture-masks/{effective_scope}.json"
        ))
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|json| {
            json.get("masks")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .is_some_and(|masks| {
            masks.iter().any(|mask| {
                mask.as_str()
                    .is_some_and(|mask| mask.eq_ignore_ascii_case(stem))
            })
        });
        if runtime_mask {
            let runtime_scopes: Vec<&str> = if is_stage_scope(&effective_scope) {
                vec![effective_scope.as_str(), "patch", "ctexture", "ptexture"]
            } else {
                vec!["patch", "ctexture", "ptexture"]
            };

            for scope in &runtime_scopes {
                for candidate in &candidates {
                    if let Some(bytes) = resolve_runtime_texture_exact(&[scope], candidate) {
                        return Some((candidate.clone(), bytes));
                    }
                }
            }
        }

        let chain = self.chain(root, &effective_scope);
        let refs: Vec<&PakArchive> = chain.iter().collect();
        resolve_mask_in_chain(&refs, stem)
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

fn is_8bit_bmp(bytes: &[u8]) -> bool {
    bytes.len() > 30 && bytes.starts_with(b"BM") && u16::from_le_bytes([bytes[28], bytes[29]]) == 8
}

fn pak_has_colour_stem(pak: &PakArchive, stem: &str) -> bool {
    pak.entries().any(|entry| {
        let base = entry.name.rsplit('/').next().unwrap_or(&entry.name);
        base.rsplit_once('.')
            .is_some_and(|(s, _)| !s.is_empty() && s.eq_ignore_ascii_case(stem))
    })
}

fn resolve_mask_in_chain(refs: &[&PakArchive], stem: &str) -> Option<(String, Vec<u8>)> {
    let candidates = [format!("k{stem}.bmp"), format!("_k{stem}.bmp")];
    let gated = refs.iter().any(|pak| {
        pak_has_colour_stem(pak, stem)
            && candidates
                .iter()
                .any(|candidate| find_pak_entry(pak, candidate).is_some_and(|b| is_8bit_bmp(&b)))
    });
    if !gated {
        return None;
    }
    for pak in refs {
        for candidate in &candidates {
            if let Some(bytes) = find_pak_entry(pak, candidate) {
                return Some((candidate.clone(), bytes));
            }
        }
    }
    None
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
    let decoded = decode_image(bytes).ok()?;
    Some(image_from_decoded_with_quality(
        images,
        &decoded,
        texture_quality,
    ))
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
    let mut color = decode_image(color_bytes).ok()?;
    let companion = companion_bytes.and_then(|bytes| decode_image(bytes).ok());
    apply_legacy_alpha(&mut color, companion.as_ref(), luminance_fallback);

    if color_key && companion.is_none() {
        punch_color_key(&mut color.rgba, 8, 26);
    }
    Some(image_from_decoded(images, &color))
}

pub fn image_handle_with_legacy_style_quality(
    images: &mut Assets<Image>,
    color_bytes: &[u8],
    companion_bytes: Option<&[u8]>,
    luminance_fallback: bool,
    color_key: bool,
    texture_quality: u8,
) -> Option<Handle<Image>> {
    let mut color = decode_image(color_bytes).ok()?;
    let companion = companion_bytes.and_then(|bytes| decode_image(bytes).ok());
    apply_legacy_alpha(&mut color, companion.as_ref(), luminance_fallback);

    if color_key && companion.is_none() {
        punch_color_key(&mut color.rgba, 8, 26);
    }
    Some(image_from_decoded_with_quality(
        images,
        &color,
        texture_quality,
    ))
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
    let original_point = texture_quality == 2;
    let filter = if original_point {
        ImageFilterMode::Nearest
    } else {
        ImageFilterMode::Linear
    };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
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
            8
        },
        ..Default::default()
    });
}

pub fn image_from_decoded_with_quality(
    images: &mut Assets<Image>,
    decoded: &meteor_asset::texture::DecodedImage,
    texture_quality: u8,
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
    let (data, mip_level_count) = rgba8_mip_chain(decoded);
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mip_level_count;
    apply_texture_quality(&mut image, texture_quality);
    images.add(image)
}

fn rgba8_mip_chain(decoded: &meteor_asset::texture::DecodedImage) -> (Vec<u8>, u32) {
    let mut data = decoded.rgba.clone();
    let mut level = decoded.rgba.clone();
    let mut width = decoded.width;
    let mut height = decoded.height;
    let mut mip_level_count = 1;
    while width > 1 || height > 1 {
        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let mut next = vec![0u8; (next_width * next_height * 4) as usize];
        for y in 0..next_height {
            for x in 0..next_width {
                let mut sum = [0u32; 4];
                let mut count = 0u32;
                for source_y in (y * 2)..((y * 2 + 2).min(height)) {
                    for source_x in (x * 2)..((x * 2 + 2).min(width)) {
                        let index = ((source_y * width + source_x) * 4) as usize;
                        for channel in 0..4 {
                            sum[channel] += u32::from(level[index + channel]);
                        }
                        count += 1;
                    }
                }
                let index = ((y * next_width + x) * 4) as usize;
                for channel in 0..4 {
                    next[index + channel] = ((sum[channel] + count / 2) / count) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        level = next;
        width = next_width;
        height = next_height;
        mip_level_count += 1;
    }
    (data, mip_level_count)
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
        let (data, levels) = rgba8_mip_chain(&decoded);
        assert_eq!(levels, 2);
        assert_eq!(data.len(), 20);
        assert_eq!(&data[16..20], &[138, 145, 152, 159]);
    }

    #[test]
    fn texture_quality_maps_to_web_sampler_profiles() {
        for (quality, filter, anisotropy, lod_max) in [
            (0, ImageFilterMode::Linear, 8, 32.0),
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
    fn mask_requires_same_pak_colour_pair() {
        let stage = zip_pak(&[("sn09/wpnFR02.jpg", b"colour")]);
        let ctex = zip_pak(&[("ctexture/kwpnFR02.bmp", &bmp8()[..])]);
        let chain = [&stage, &ctex];
        assert_eq!(resolve_mask_in_chain(&chain, "wpnFR02"), None);
    }

    #[test]
    fn mask_pairs_with_colour_in_same_pak() {
        let stage = zip_pak(&[
            ("sn03/sn3tr.JPG", b"colour"),
            ("sn03/ksn3tr.bmp", &bmp8()[..]),
        ]);
        let chain = [&stage];
        let (name, bytes) = resolve_mask_in_chain(&chain, "sn3tr").expect("paired mask");
        assert_eq!(name, "ksn3tr.bmp");
        assert_eq!(bytes, bmp8());
    }

    #[test]
    fn mask_requires_8bit_bmp() {
        let stage = zip_pak(&[
            ("sn03/mtr4.jpg", b"colour"),
            ("sn03/kmtr4.bmp", &bmp24()[..]),
        ]);
        let chain = [&stage];
        assert_eq!(resolve_mask_in_chain(&chain, "mtr4"), None);
    }

    #[test]
    fn mask_fetch_is_pak_outer_then_k_before_underscore_k() {
        let b = zip_pak(&[("sn03/ksn3tr.bmp", b"mask-b")]);

        let a = zip_pak(&[("sn03/sn3tr.jpg", b"c"), ("sn03/_ksn3tr.bmp", &bmp8()[..])]);
        let chain = [&a, &b];
        let (name, bytes) = resolve_mask_in_chain(&chain, "sn3tr").expect("mask");
        assert_eq!(name, "_ksn3tr.bmp");
        assert_eq!(bytes, bmp8());
    }

    fn bmp8() -> Vec<u8> {
        let mut bytes = vec![0u8; 54];
        bytes[0] = b'B';
        bytes[1] = b'M';
        bytes[28..30].copy_from_slice(&8u16.to_le_bytes());
        bytes
    }

    fn bmp24() -> Vec<u8> {
        let mut bytes = bmp8();
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        bytes
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
