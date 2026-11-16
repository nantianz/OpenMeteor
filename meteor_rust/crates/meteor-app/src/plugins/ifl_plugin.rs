use crate::plugins::pak_textures::{punch_color_key, PakCache};
use crate::plugins::stage_material::scene_textures::SceneTexturePack;
use crate::plugins::stage_material::GmdlMaterial;
use bevy::image::Image;
use bevy::prelude::*;
use meteor_asset::texture::decode_image;

pub const IFL_FPS: f32 = 12.0;

pub fn parse_ifl(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("//"))
        .map(str::to_string)
        .collect()
}

pub fn infer_ifl_frames(ifl_name: &str) -> Vec<String> {
    let base = ifl_name.trim_end_matches(".ifl");
    if base.is_empty() {
        return Vec::new();
    }
    if base.to_ascii_lowercase().ends_with("s53f") {
        return (100..=114).map(|i| format!("{base}{i}.jpg")).collect();
    }

    let digits_start = base.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1);
    let Some(start) = digits_start else {
        return Vec::new();
    };
    if start >= base.len() {
        return Vec::new();
    }
    let (prefix, suffix) = base.split_at(start);
    let Ok(start_num) = suffix.parse::<u32>() else {
        return Vec::new();
    };
    let width = suffix.len();
    (0..32)
        .map(|i| format!("{prefix}{:0width$}.JPG", start_num + i, width = width))
        .collect()
}

pub fn load_ifl_frames(
    pak_cache: &mut PakCache,
    root: &std::path::Path,
    images: &mut Assets<Image>,
    ifl_name: &str,
    current_scope: &str,
    color_key: bool,
    texture_quality: u8,
) -> Option<Vec<Handle<Image>>> {
    load_ifl_frames_inner(
        pak_cache,
        root,
        images,
        ifl_name,
        current_scope,
        color_key,
        texture_quality,
        None,
    )
}

pub(crate) fn load_scene_ifl_frames(
    pak_cache: &mut PakCache,
    root: &std::path::Path,
    images: &mut Assets<Image>,
    ifl_name: &str,
    current_scope: &str,
    color_key: bool,
    texture_quality: u8,
    hd_pack: &SceneTexturePack,
) -> Option<Vec<Handle<Image>>> {
    load_ifl_frames_inner(
        pak_cache,
        root,
        images,
        ifl_name,
        current_scope,
        color_key,
        texture_quality,
        Some(hd_pack),
    )
}

#[allow(clippy::too_many_arguments)]
fn load_ifl_frames_inner(
    pak_cache: &mut PakCache,
    root: &std::path::Path,
    images: &mut Assets<Image>,
    ifl_name: &str,
    current_scope: &str,
    color_key: bool,
    texture_quality: u8,
    hd_pack: Option<&SceneTexturePack>,
) -> Option<Vec<Handle<Image>>> {
    let normalized = ifl_name.replace('\\', "/");
    let (scope, file) = if let Some((head, tail)) = normalized.split_once('/') {
        if head.len() == 4 && head.starts_with("sn") {
            (head.to_string(), tail.to_string())
        } else {
            (current_scope.to_string(), normalized.clone())
        }
    } else {
        (current_scope.to_string(), normalized.clone())
    };

    let list_text = pak_cache
        .resolve(root, &scope, &file)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string());
    let names = list_text
        .as_deref()
        .map(parse_ifl)
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| infer_ifl_frames(&file));
    if names.is_empty() {
        return None;
    }
    let mut frames = Vec::with_capacity(names.len());
    for name in &names {
        let Some(bytes) = pak_cache.resolve(root, &scope, name) else {
            continue;
        };
        if let Some(handle) = hd_pack
            .and_then(|pack| pack.diffuse(&scope, name))
            .and_then(|hd_bytes| {
                crate::plugins::pak_textures::hd_image_handle_with_legacy_style_quality(
                    images,
                    hd_bytes,
                    Some(&bytes),
                    None,
                    false,
                    color_key,
                    texture_quality,
                )
            })
        {
            frames.push(handle);
            continue;
        }
        let Ok(mut decoded) = decode_image(&bytes) else {
            continue;
        };
        if color_key {
            punch_color_key(&mut decoded.rgba, 8, 26);
        }
        frames.push(
            crate::plugins::pak_textures::image_from_decoded_with_quality(
                images,
                &decoded,
                texture_quality,
            ),
        );
    }
    if frames.is_empty() {
        None
    } else {
        Some(frames)
    }
}

#[derive(Component)]
pub struct IflAnim {
    pub frames: Vec<Handle<Image>>,

    pub material: Handle<GmdlMaterial>,

    pub cursor: usize,

    pub timer: f32,
}

pub(crate) fn advance_ifl_anim(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut query: Query<&mut IflAnim>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    for mut anim in &mut query {
        if anim.frames.len() < 2 {
            continue;
        }
        anim.timer += crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
        let step = 1.0 / IFL_FPS;
        let mut steps = 0usize;
        while anim.timer >= step {
            anim.timer -= step;
            steps += 1;
        }
        if steps > 0 {
            anim.cursor = (anim.cursor + steps) % anim.frames.len();
            let next = &anim.frames[anim.cursor];
            if materials
                .get(&anim.material)
                .is_some_and(|m| m.texture.as_ref() != Some(next))
            {
                if let Some(mut material) = materials.get_mut(&anim.material) {
                    material.texture = Some(next.clone());
                    #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
                    super::perf_plugin::count_event(4);
                }
            }
        }
    }
}

pub struct IflPlugin;

impl Plugin for IflPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, advance_ifl_anim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "original-assets")]
    #[test]
    fn hd_scene_ifl_preserves_frame_order_count_and_authored_alpha() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let hd_bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../asset-tools/generated/scene-textures/sn06/sn06mian5.jpg"),
        )
        .unwrap();
        let pack = SceneTexturePack::load_with_reader("sn01", true, |path| {
            if path.ends_with("manifest.json") {
                Some(
                    br#"{"version":1,"textures":{"s53f100.jpg":{"diffuse":"hd-fire.jpg"}}}"#
                        .to_vec(),
                )
            } else {
                Some(hd_bytes.clone())
            }
        });
        let mut images = Assets::<Image>::default();
        let mut cache = PakCache::default();
        let old = load_ifl_frames(
            &mut cache,
            &origin,
            &mut images,
            "s53f.ifl",
            "sn01",
            true,
            0,
        )
        .unwrap();
        let hd = load_scene_ifl_frames(
            &mut cache,
            &origin,
            &mut images,
            "s53f.ifl",
            "sn01",
            true,
            0,
            &pack,
        )
        .unwrap();
        assert_eq!(old.len(), hd.len());
        assert_eq!(old.len(), 15);
        for (old, hd) in old.iter().zip(&hd).skip(1) {
            assert_eq!(images.get(old).unwrap().data, images.get(hd).unwrap().data);
        }
        let old = images.get(&old[0]).unwrap();
        let hd = images.get(&hd[0]).unwrap();
        let os = old.texture_descriptor.size;
        let hs = hd.texture_descriptor.size;
        assert!(hs.width > os.width);
        for y in (0..hs.height).step_by(31) {
            for x in (0..hs.width).step_by(31) {
                let sy = y * os.height / hs.height;
                let sx = x * os.width / hs.width;
                assert_eq!(
                    hd.data.as_ref().unwrap()[((y * hs.width + x) * 4 + 3) as usize],
                    old.data.as_ref().unwrap()[((sy * os.width + sx) * 4 + 3) as usize]
                );
            }
        }
    }

    #[test]
    fn parses_ifl_lines() {
        let text = "s53f100.jpg\n\n# comment\n// note\ns53f101.jpg\n";
        assert_eq!(parse_ifl(text), vec!["s53f100.jpg", "s53f101.jpg"]);
    }

    #[test]
    fn infers_s53f_sequence() {
        let frames = infer_ifl_frames("s53f.ifl");
        assert_eq!(frames.len(), 15);
        assert_eq!(frames[0], "s53f100.jpg");
        assert_eq!(frames[14], "s53f114.jpg");
    }

    #[test]
    fn infers_numeric_suffix_sequence() {
        let frames = infer_ifl_frames("Nay0100.ifl");
        assert_eq!(frames.len(), 32);
        assert_eq!(frames[0], "Nay0100.JPG");
        assert_eq!(frames[1], "Nay0101.JPG");
        assert_eq!(frames[31], "Nay0131.JPG");
    }

    #[test]
    fn infers_unknown_returns_empty() {
        assert!(infer_ifl_frames("").is_empty());
        assert!(infer_ifl_frames("plain.ifl").is_empty());
    }

    #[test]
    fn punch_keys_near_black_and_corner() {
        let mut rgba = vec![8u8, 8, 8, 255, 255, 255, 255, 255];
        punch_color_key(&mut rgba, 8, 26);
        assert_eq!(rgba[3], 0);
        assert_eq!(rgba[7], 255);
    }

    #[test]
    fn punch_keys_corner_band() {
        let mut rgba = vec![10u8, 20, 30, 255, 30, 40, 50, 255];
        punch_color_key(&mut rgba, 8, 26);
        assert_eq!(rgba[3], 0);
        assert_eq!(rgba[7], 0);
    }
}
