#![cfg(feature = "original-assets")]
use super::*;
use bevy::image::Image;
use std::collections::HashMap;

fn source_uvs(root: &std::path::Path) -> Vec<[f32; 2]> {
    let bytes = std::fs::read(root.join("assets/source/p0-remake/source.glb")).unwrap();
    let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    let json_end = 20 + word(12);
    let document: serde_json::Value = serde_json::from_slice(&bytes[20..json_end]).unwrap();
    let index = document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"]
        .as_u64()
        .unwrap() as usize;
    let accessor = &document["accessors"][index];
    assert_eq!(accessor["componentType"], 5126);
    assert_eq!(accessor["type"], "VEC2");
    let view = &document["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
    let start = json_end
        + 8
        + view["byteOffset"].as_u64().unwrap_or(0) as usize
        + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
    let stride = view["byteStride"].as_u64().unwrap_or(8) as usize;
    (0..accessor["count"].as_u64().unwrap() as usize)
        .map(|i| {
            let offset = start + i * stride;
            [
                f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
                f32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()),
            ]
        })
        .collect()
}

fn sample(image: &Image, uv: [f32; 2]) -> [f64; 3] {
    let w = image.texture_descriptor.size.width as i64;
    let h = image.texture_descriptor.size.height as i64;
    let x = f64::from(uv[0]) * w as f64 - 0.5;
    let y = f64::from(uv[1]) * h as f64 - 0.5;
    let data = image.data.as_ref().unwrap();
    let tx = x - x.floor();
    let ty = y - y.floor();
    let mut rgb = [0.0; 3];
    for dy in 0..2 {
        for dx in 0..2 {
            let weight =
                (if dx == 0 { 1.0 - tx } else { tx }) * (if dy == 0 { 1.0 - ty } else { ty });
            let pixel = ((y.floor() as i64 + dy).rem_euclid(h) * w
                + (x.floor() as i64 + dx).rem_euclid(w)) as usize
                * 4;
            for channel in 0..3 {
                rgb[channel] += f64::from(data[pixel + channel]) * weight;
            }
        }
    }
    rgb
}

#[test]
fn remake_export_and_uploaded_atlas_preserve_glb_uvs_for_both_lods_and_all_camps() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools");
    let generated = root.join("generated/p0-remake");
    let key = |uv: [f32; 2]| [(uv[0] * 1e6).round() as i32, (uv[1] * 1e6).round() as i32];
    let mut source = HashMap::<[i32; 2], Vec<[f32; 2]>>::new();
    for uv in source_uvs(&root) {
        source.entry(key(uv)).or_default().push(uv);
    }
    for lod in ["high", "low"] {
        let skin =
            parse_skin(&std::fs::read_to_string(generated.join(format!("{lod}.skc"))).unwrap());
        let mesh = build_group_mesh(&skin, &skin.indices);
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("prepared mesh must upload its texture coordinates");
        };
        let originals: Vec<_> = uvs.iter().enumerate().map(|(i, uv)| {
            let glb_uv = [uv[0], 1.0 - uv[1]];
            let bin = key(glb_uv);
            (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| [bin[0]+dx, bin[1]+dy]))
                .filter_map(|bin| source.get(&bin)).flatten()
                .find(|p| (p[0] - glb_uv[0]).abs() < 2e-7 && (p[1] - glb_uv[1]).abs() < 2e-7)
                .copied().unwrap_or_else(|| panic!("{lod} vertex {i}: uploaded UV {uv:?} lost the source GLB atlas location"))
        }).collect();
        for camp in ["neutral", "red", "blue"] {
            let file = if lod == "high" {
                format!("{camp}.jpg")
            } else {
                format!("low-{camp}.jpg")
            };
            let bytes = std::fs::read(generated.join(file)).unwrap();

            let reference = Image::from_dynamic(
                image::load_from_memory(&bytes).unwrap(),
                false,
                RenderAssetUsages::MAIN_WORLD,
            );
            let mut images = Assets::<Image>::default();
            let handle =
                crate::plugins::pak_textures::image_handle_with_quality(&mut images, &bytes, 0)
                    .unwrap();
            let uploaded = images.get(&handle).unwrap();
            for i in (0..uvs.len()).step_by(7) {
                let expected = sample(&reference, originals[i]);
                let actual = sample(uploaded, uvs[i]);
                for channel in 0..3 {
                    assert!(
                        (expected[channel] - actual[channel]).abs() < 0.2,
                        "{lod}/{camp} vertex {i}: source {expected:?}, uploaded {actual:?}"
                    );
                }
            }
        }
    }
}
