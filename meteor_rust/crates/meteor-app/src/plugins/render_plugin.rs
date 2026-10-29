use crate::plugins::ifl_plugin::{load_ifl_frames, IflAnim};
use crate::plugins::pak_textures::{
    image_handle_with_legacy_style_quality, image_handle_with_quality, legacy_alpha_companion_name,
    pak_files_root, PakCache,
};
use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
use crate::plugins::stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::{
    mesh::Indices,
    render_resource::{Extent3d, Face, PrimitiveTopology, TextureDimension, TextureFormat},
};
use meteor_asset::gmdl::{decode_gmdl, GmdlMaterial as RawGmdlMaterial, GmdlMesh};
use meteor_asset::pak::PakArchive;
use meteor_core::constants::MODEL_SCALE;
use meteor_physics::collision::{CollisionSystem, Point3};
use std::collections::{BTreeSet, HashMap, HashSet};

mod gem_visual;
#[cfg(test)]
mod weapon_drop_tests;

#[derive(Resource, Debug, Clone, Default)]
pub struct StageCollision {
    pub stage_id: String,

    pub face_ids: BTreeSet<u32>,

    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
}

#[derive(Resource, Default)]
struct StageTextureHandles(Vec<Handle<bevy::image::Image>>);

fn clear_stage_collision(mut collision: ResMut<StageCollision>) {
    *collision = StageCollision::default();
}

fn clear_stage_mesh(mut collider: ResMut<StageCollider>) {
    collider.mesh = None;
}

fn clear_stage_texture_handles(mut handles: ResMut<StageTextureHandles>) {
    handles.0.clear();
}

fn sync_stage_texture_quality(
    settings: Res<crate::app_state::Settings>,
    handles: Res<StageTextureHandles>,
    mut images: ResMut<Assets<bevy::image::Image>>,
) {
    if !settings.is_changed() {
        return;
    }
    for handle in &handles.0 {
        if let Some(mut image) = images.get_mut(handle) {
            crate::plugins::pak_textures::apply_texture_quality(
                &mut image,
                settings.texture_quality,
            );
        }
    }
}

fn load_stage_collision(
    candidates: &[std::path::PathBuf],
    loaded: &crate::app_state::LoadedStageInfo,
    authoritative: Option<&[u8]>,
    collision: &mut StageCollision,
) {
    let cob_path = format!("{}/{}.cob", loaded.stage_id, loaded.stage_id);
    let fallback = candidates
        .iter()
        .find_map(|base| crate::resource_io::read(base.join(&cob_path)).ok());
    let Some(bytes) = authoritative.or(fallback.as_deref()) else {
        info!("stage cob missing for {}", loaded.stage_id);
        return;
    };
    match meteor_asset::cob::parse_cob(bytes) {
        Ok(grid) => {
            collision.stage_id = loaded.stage_id.clone();
            collision.face_ids = grid.face_ids.clone();
            collision.nx = grid.nx;
            collision.ny = grid.ny;
            collision.nz = grid.nz;
            info!(
                "stage collision parsed: {} faces, grid {}x{}x{}",
                grid.face_ids.len(),
                grid.nx,
                grid.ny,
                grid.nz
            );
        }
        Err(error) => warn!("unable to decode COB for {}: {error}", loaded.stage_id),
    }
}

pub(crate) fn model_stems(model: &str) -> Vec<String> {
    let trimmed = model.trim();
    let stem = trimmed.split('.').next().unwrap_or(trimmed).to_string();
    let mut stems = vec![stem.clone(), stem.to_lowercase(), stem.to_uppercase()];
    let lower = stem.to_ascii_lowercase();
    let looks_like_weapon_part = lower
        .strip_prefix('w')
        .and_then(|tail| tail.split_once('_'))
        .is_some_and(|(class, variant)| {
            !class.is_empty()
                && class.chars().all(|c| c.is_ascii_digit())
                && !variant.is_empty()
                && variant.chars().all(|c| c.is_ascii_digit())
        });
    if looks_like_weapon_part {
        stems.push(format!("{stem}r"));
        stems.push(format!("{stem}l"));
        stems.push(format!("{lower}r"));
        stems.push(format!("{lower}l"));
    }
    stems
}

fn gmc_to_gmdl(gmc: meteor_asset::gmc::GmcMesh, descriptor: Option<&str>) -> GmdlMesh {
    let placements = descriptor
        .map(meteor_asset::des::parse_stage_description)
        .map(|desc| {
            desc.scene_objects
                .into_iter()
                .chain(desc.dummies)
                .map(|object| {
                    (
                        object.name.to_ascii_lowercase(),
                        (
                            Vec3::new(object.position.x, object.position.y, object.position.z),
                            Quat::from_xyzw(
                                object.quaternion.x,
                                object.quaternion.y,
                                object.quaternion.z,
                                object.quaternion.w,
                            ),
                            object.visible,
                        ),
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let max_shader = gmc
        .shaders
        .iter()
        .map(|shader| shader.id)
        .chain(
            gmc.objects
                .iter()
                .flat_map(|object| object.faces.iter())
                .filter_map(|face| usize::try_from(face.shader).ok()),
        )
        .max()
        .unwrap_or(0);
    let materials = (0..=max_shader)
        .map(|id| {
            let shader = gmc.shaders.iter().find(|shader| shader.id == id);
            let tex_index = shader.map_or_else(
                || {
                    if id < gmc.textures.len() {
                        id as i32
                    } else {
                        -1
                    }
                },
                |shader| shader.texture_index,
            );
            let blend = shader
                .map(|shader| shader.texture_mode.clone())
                .filter(|mode| !mode.is_empty())
                .unwrap_or_else(|| "NORMAL".into());
            let state = shader.map_or_else(
                || "DISABLE 1 0".into(),
                |shader| {
                    if shader.blend_enabled {
                        format!(
                            "ENABLE {} {}",
                            shader.blend_source, shader.blend_destination
                        )
                    } else {
                        "DISABLE 1 0".into()
                    }
                },
            );
            RawGmdlMaterial {
                tex_index,
                blend,
                two_side: shader.is_some_and(|shader| shader.two_side),
                state,
                alpha: shader.map_or(1.0, |shader| shader.opacity),
            }
        })
        .collect();
    let objects = gmc
        .objects
        .into_iter()
        .filter_map(|object| {
            let (translation, rotation, visible) = placements
                .get(&object.name.to_ascii_lowercase())
                .copied()
                .unwrap_or((Vec3::ZERO, Quat::IDENTITY, true));
            if !visible || object.positions.is_empty() || object.faces.is_empty() {
                return None;
            }
            let positions = object
                .positions
                .chunks_exact(3)
                .flat_map(|point| {
                    let point = rotation * Vec3::from_slice(point) + translation;
                    [point.x, point.y, point.z]
                })
                .collect::<Vec<_>>();
            let normals = object
                .normals
                .chunks_exact(3)
                .flat_map(|normal| {
                    let normal = rotation * Vec3::from_slice(normal);
                    [normal.x, normal.y, normal.z]
                })
                .collect::<Vec<_>>();
            let vertex_count = positions.len() / 3;
            let mut face_mat = Vec::new();
            let mut indices = Vec::new();
            for face in object.faces {
                if face
                    .indices
                    .iter()
                    .any(|index| *index as usize >= vertex_count)
                {
                    continue;
                }
                face_mat.push(face.shader.max(0));
                indices.extend_from_slice(&face.indices);
            }
            (!indices.is_empty()).then_some(meteor_asset::gmdl::GmdlObject {
                name: object.name,
                num_verts: vertex_count,
                num_faces: face_mat.len(),
                positions,
                normals,
                uvs: object.uvs,
                colors: object.colors,
                face_mat,
                indices,
            })
        })
        .collect();
    GmdlMesh {
        textures: gmc
            .textures
            .into_iter()
            .map(|name| meteor_asset::gmdl::GmdlTexture { name })
            .collect(),
        materials,
        objects,
    }
}

fn load_prop_model(candidates: &[std::path::PathBuf], stems: &[String]) -> Option<GmdlMesh> {
    load_prop_model_with(stems, |path| {
        meteor_asset::runtime_assets::read_legacy_path(path).or_else(|| {
            candidates
                .iter()
                .find_map(|root| crate::resource_io::read(root.join(path)).ok())
        })
    })
}

pub(crate) fn load_prop_model_with(
    stems: &[String],
    mut read: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Option<GmdlMesh> {
    for stem in stems {
        for dir in ["cmodel", "pmodel"] {
            if let Some(model) =
                read(&format!("{dir}/{stem}.gmb")).and_then(|bytes| decode_gmdl(&bytes).ok())
            {
                return Some(model);
            }
        }
    }
    for stem in stems {
        for dir in ["cmodel", "pmodel"] {
            let gmc = ["GMC", "gmc"]
                .into_iter()
                .find_map(|extension| read(&format!("{dir}/{stem}.{extension}")));
            let Some(gmc) = gmc else {
                continue;
            };
            let descriptor = ["des", "DES"].into_iter().find_map(|extension| {
                read(&format!("{dir}/{stem}.{extension}"))
                    .map(|bytes| crate::text_codec::decode_text(&bytes))
            });
            return Some(gmc_to_gmdl(
                meteor_asset::gmc::parse_gmc(&crate::text_codec::decode_text(&gmc)),
                descriptor.as_deref(),
            ));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn spawn_stage_props(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
    pak_cache: &mut PakCache,
    root: &std::path::Path,
    candidates: &[std::path::PathBuf],
    loaded: &crate::app_state::LoadedStageInfo,
    des_override: Option<&str>,
    texture_quality: u8,
    texture_handles: &mut Vec<Handle<bevy::image::Image>>,
    collider: &mut StageMeshCollider,
    script: Option<&meteor_scene::scene_script_runtime::SceneScriptRuntime>,
    escort: Option<&meteor_network::escort_definition::EscortDefinition>,
) {
    let des_path = format!("{}/{}.des", loaded.stage_id, loaded.stage_id);
    let disk_des = candidates
        .iter()
        .find_map(|base| crate::resource_io::read_to_string(base.join(&des_path)).ok());
    let Some(des_text) = des_override.or(disk_des.as_deref()) else {
        info!("stage des missing for {}", loaded.stage_id);
        return;
    };
    let desc = meteor_asset::des::parse_stage_description(des_text);
    info!(
        "stage des parsed: {} scene objects, {} dummies",
        desc.scene_objects.len(),
        desc.dummies.len()
    );
    let asset_reader = super::escort_assets::StageAssetReader::new(&loaded.stage_id);
    let mut model_cache: HashMap<String, Option<GmdlMesh>> = HashMap::new();
    let mut material_cache: HashMap<(String, i32, Option<usize>), Handle<GmdlMaterial>> =
        HashMap::new();
    let mut image_cache: HashMap<String, LoadedStageTexture> = HashMap::new();
    let mut ifl_cache: HashMap<(String, i32), Vec<Handle<Image>>> = HashMap::new();
    let mut ifl_tracks = HashSet::new();
    let mut gem_scroll_tracks = HashSet::new();
    let mut spawned = 0usize;
    let mut missing = 0usize;
    let mut textured = 0usize;
    for dummy in &desc.dummies {
        let Some(model) = dummy.model.as_ref() else {
            continue;
        };
        let stems = model_stems(model);
        let key = stems[0].to_lowercase();
        let cached = model_cache
            .entry(key.clone())
            .or_insert_with(|| load_prop_model_with(&stems, |path| asset_reader.read(path)));
        let Some(model_mesh) = cached.as_ref() else {
            missing += 1;
            continue;
        };
        let gem = gem_visual::is_attribute_gem(&key)
            && dummy
                .name_attr
                .as_deref()
                .is_none_or(|name| name.eq_ignore_ascii_case("treasure"));
        let model_desc = gem
            .then(|| {
                stems.iter().find_map(|stem| {
                    asset_reader
                        .read(&format!("cmodel/{stem}.des"))
                        .map(|bytes| {
                            meteor_asset::des::parse_stage_description(
                                &crate::text_codec::decode_text(&bytes),
                            )
                        })
                })
            })
            .flatten();

        let fmc = stems.iter().find_map(|stem| {
            ["cmodel", "pmodel"].iter().find_map(|dir| {
                asset_reader
                    .read(&format!("{dir}/{stem}.fmc"))
                    .map(|bytes| crate::text_codec::decode_text(&bytes))
                    .map(|text| meteor_asset::fmc::parse_fmc(&text))
            })
        });
        let poses = if let Some(pos) = stems.iter().find_map(|stem| {
            ["cmodel", "pmodel"].iter().find_map(|dir| {
                asset_reader
                    .read(&format!("{dir}/{stem}.pos"))
                    .map(|bytes| crate::text_codec::decode_text(&bytes))
            })
        }) {
            meteor_asset::fmc::parse_prop_poses(&pos)
        } else {
            meteor_asset::fmc::parse_prop_poses(des_text)
        };
        let prop_transform = prop_transform(dummy, loaded.center);
        let (center_offset, half_extents) = prop_bounds(model_mesh, dummy.scale);
        let runtime_instance = script.and_then(|runtime| {
            runtime
                .host()
                .instances()
                .iter()
                .find(|instance| instance.name.eq_ignore_ascii_case(&dummy.name))
        });

        let objective_model =
            escort.is_some_and(|e| e.scene_name.eq_ignore_ascii_case(&dummy.name));
        let script_type =
            runtime_instance.and_then(|instance| match instance.props.get("script_type") {
                Some(meteor_scene::pst_runtime::PstValue::Str(name)) => Some(name.as_str()),
                _ => None,
            });
        let treasure = !objective_model
            && gem_visual::is_treasure_type(script_type.or(dummy.name_attr.as_deref()));
        let collision_enabled = !objective_model
            && runtime_instance.map_or_else(
                || is_solid_prop(dummy),
                |instance| instance.collision_enabled(),
            );
        let active = runtime_instance.map_or(dummy.visible, |instance| {
            instance.get("active").as_f64() != 0.0
        });
        let interactive =
            runtime_instance.is_none_or(|instance| instance.get("interactive").as_f64() != 0.0);
        let collision_capable =
            !objective_model && (collision_enabled || is_collision_capable_prop(dummy));

        let prop_marker = crate::plugins::item_plugin::StageProp {
            name: dummy.name.clone(),
            model: key.clone(),
            center_offset,
            half_extents,
            hit_count: 0,
            respawn_timer: 0.0,
            interactive,
            collision_enabled,
            breakable: !objective_model && crate::plugins::item_plugin::is_breakable_model(&key),
            broken: false,
        };

        let mut prop_collider_owner = None;
        let mut collider_parts: Vec<Option<(usize, Vec<[f32; 3]>)>> =
            vec![None; model_mesh.objects.len()];
        if collision_capable {
            let (verts, prop_indices, parts) =
                prop_collision_geometry(model_mesh, dummy, prop_transform);
            collider_parts = parts;
            prop_collider_owner = Some(collider.append_prop_world_triangles(&verts, &prop_indices));
            if let Some(owner) = prop_collider_owner {
                collider.set_owner_enabled(owner, collision_enabled);
            }
        }

        let billboard = dummy.billboard == 1;
        let mut collider_owner_attached = false;
        let mut collider_part_attached = std::collections::HashSet::new();
        let mut prop_entity_spawned = false;
        for (obj_idx, object) in model_mesh.objects.iter().enumerate() {
            if object.indices.is_empty() || object.positions.is_empty() {
                continue;
            }
            let mut groups: HashMap<i32, Vec<u32>> = HashMap::new();
            for (face, mat_idx) in object.face_mat.iter().enumerate() {
                if !is_drawable_material_index(*mat_idx, model_mesh.materials.len()) {
                    continue;
                }
                groups
                    .entry(*mat_idx)
                    .or_default()
                    .extend_from_slice(&object.indices[face * 3..face * 3 + 3]);
            }

            if groups.is_empty() {
                groups.insert(-1, Vec::new());
            }
            for (mat_idx, indices) in groups {
                let mut bundle = commands.spawn((
                    PlayingSceneEntity,
                    prop_transform,
                    prop_marker.clone(),
                    super::hazard_plugin::HazardPart::from_positions(&object.positions),
                    prop_visibility(active),
                ));
                if mat_idx >= 0 {
                    let mat = &model_mesh.materials[mat_idx as usize];
                    let (mode, opacity, mut alpha_cutoff) = classify_gmdl_material(mat);
                    let animation_key = (key.clone(), mat_idx);
                    if let Some(name) = model_mesh
                        .resolve_texture_name(mat)
                        .filter(|name| name.to_ascii_lowercase().ends_with(".ifl"))
                    {
                        if !ifl_cache.contains_key(&animation_key) {
                            let frames = load_ifl_frames(
                                pak_cache,
                                root,
                                images,
                                name,
                                &loaded.stage_id,
                                mat.blend.eq_ignore_ascii_case("COLORKEY"),
                                texture_quality,
                            )
                            .unwrap_or_default();
                            texture_handles.extend(frames.iter().cloned());
                            ifl_cache.insert(animation_key.clone(), frames);
                        }
                    }
                    let loaded_texture = model_mesh
                        .resolve_texture_name(mat)
                        .filter(|name| !name.to_ascii_lowercase().ends_with(".ifl"))
                        .map(|name| {
                            load_or_cache_texture(
                                images,
                                &mut image_cache,
                                pak_cache,
                                root,
                                &loaded.stage_id,
                                name,
                                mat.blend.eq_ignore_ascii_case("COLORKEY"),
                                mat.blend.eq_ignore_ascii_case("ALPHACHANNEL"),
                                texture_quality,
                            )
                        })
                        .unwrap_or_default();
                    if loaded_texture.alpha_test && mode == GmdlMode::Opaque {
                        alpha_cutoff = 127.0 / 255.0;
                    }
                    let texture = ifl_cache
                        .get(&animation_key)
                        .and_then(|frames| frames.first().cloned())
                        .or(loaded_texture.handle);
                    if let Some(handle) = &texture {
                        texture_handles.push(handle.clone());
                    }
                    if texture.is_some() {
                        textured += 1;
                    }
                    let handle = material_cache
                        .entry((key.clone(), mat_idx, gem.then_some(obj_idx)))
                        .or_insert_with(|| {
                            materials.add(GmdlMaterial::new_scene(
                                GmdlMaterialParams {
                                    opacity,
                                    alpha_cutoff,
                                    mode: mode.as_u32(),
                                    has_texture: texture.is_some() as u32,

                                    uv_offset: Vec2::ZERO,
                                },
                                texture,
                                mat.two_side,
                                mode,
                            ))
                        })
                        .clone();
                    let mesh = build_prop_mesh(object, &indices);
                    let mesh = meshes.add(mesh);
                    if treasure {
                        bundle.with_children(|children| {
                            children.spawn((
                                gem_visual::GemVisual,
                                Mesh3d(mesh),
                                MeshMaterial3d(handle.clone()),
                                Transform::default(),
                            ));
                        });
                        if gem && gem_scroll_tracks.insert((key.clone(), obj_idx, mat_idx)) {
                            if let Some(anim) = model_desc
                                .as_ref()
                                .and_then(|desc| {
                                    desc.scene_objects
                                        .iter()
                                        .find(|part| part.name == object.name)
                                })
                                .map(|part| &part.texture_animation)
                                .filter(|anim| anim.enabled)
                            {
                                bundle.insert(UvScroll {
                                    material: handle.clone(),
                                    u: anim.u,
                                    v: anim.v,
                                });
                            }
                        }
                    } else {
                        bundle.insert((Mesh3d(mesh), MeshMaterial3d(handle.clone())));
                    }
                    if let Some(frames) = ifl_cache
                        .get(&animation_key)
                        .filter(|frames| !frames.is_empty())
                    {
                        if ifl_tracks.insert(animation_key) {
                            bundle.insert(IflAnim {
                                frames: frames.clone(),
                                material: handle,
                                cursor: 0,
                                timer: 0.0,
                            });
                        }
                    }
                }
                if objective_model {
                    bundle.insert(super::escort_assets::EscortPropVisual {
                        home: prop_transform.translation,
                        applied_offset: Vec3::ZERO,
                    });
                }
                prop_entity_spawned = true;
                if billboard {
                    bundle.insert(PropBillboard);
                }

                if !collider_owner_attached {
                    if let Some(owner) = prop_collider_owner {
                        bundle.insert(PropColliderOwner(owner));
                        collider_owner_attached = true;
                    }
                }

                if let Some(fmc) = &fmc {
                    if let Some(pose) = poses.first() {
                        let collision_part = collider_part_attached
                            .insert(obj_idx)
                            .then(|| collider_parts.get(obj_idx).cloned().flatten())
                            .flatten();

                        bundle.insert(FmcClip {
                            name: dummy.name.clone(),
                            clip: fmc.clone(),
                            part_index: obj_idx,
                            poses: poses.clone(),
                            pose_index: 0,
                            pose_start: pose.start,
                            pose_end: prop_pose_clip_end(pose),
                            pose_loop: 0,
                            pose_revision: 0,
                            anim_time: 0.0,
                            state: 3,
                            frame_set: None,

                            multi: fmc.frames.first().is_some_and(|f| f.parts.len() > 1),
                            base_translation: prop_transform.translation,
                            base_rotation: prop_transform.rotation,

                            collider_owner: collision_part.as_ref().and(prop_collider_owner),
                            collider_vertex_offset: collision_part
                                .as_ref()
                                .map_or(0, |(offset, _)| *offset),
                            local_verts: collision_part
                                .map_or_else(Vec::new, |(_, vertices)| vertices),
                            last_reported_state: 1,
                        });
                    }
                }
                let _ = obj_idx;
            }
        }

        if !prop_entity_spawned {
            let mut bundle = commands.spawn((
                PlayingSceneEntity,
                prop_transform,
                prop_marker,
                prop_visibility(active),
            ));
            if objective_model {
                bundle.insert(super::escort_assets::EscortPropVisual {
                    home: prop_transform.translation,
                    applied_offset: Vec3::ZERO,
                });
            }
            if let Some(owner) = prop_collider_owner {
                bundle.insert(PropColliderOwner(owner));
            }
        }
        spawned += 1;
    }
    info!(
        "spawned {} stage props ({} models missing, {} textured material batches)",
        spawned, missing, textured
    );
}

type PropCollisionPart = Option<(usize, Vec<[f32; 3]>)>;

fn prop_visibility(active: bool) -> Visibility {
    if active {
        Visibility::Visible
    } else {
        Visibility::Hidden
    }
}

#[cfg(test)]
pub(crate) fn append_solid_props_for_test(
    root: &std::path::Path,
    desc: &meteor_asset::des::StageDescription,
    center: [f32; 3],
    collider: &mut StageMeshCollider,
) {
    for dummy in desc.dummies.iter().filter(|dummy| is_solid_prop(dummy)) {
        let Some(model) = dummy.model.as_deref() else {
            continue;
        };
        let mesh = load_prop_model_with(&model_stems(model), |path| {
            crate::resource_io::read(root.join(path)).ok()
        })
        .expect("original solid prop model");
        let (vertices, indices, _) =
            prop_collision_geometry(&mesh, dummy, prop_transform(dummy, center));
        collider.append_prop_world_triangles(&vertices, &indices);
    }
}

fn prop_collision_geometry(
    model_mesh: &GmdlMesh,
    dummy: &meteor_asset::des::StageObject,
    transform: Transform,
) -> (Vec<[f32; 3]>, Vec<u32>, Vec<PropCollisionPart>) {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    let mut parts = vec![None; model_mesh.objects.len()];
    for (object_index, object) in model_mesh.objects.iter().enumerate() {
        if object.indices.is_empty() || object.positions.is_empty() {
            continue;
        }
        let base = verts.len() as u32;
        let mut part_local = Vec::with_capacity(object.num_verts);
        for vertex in object.positions.chunks_exact(3) {
            let point = transform.rotation
                * (Vec3::from_slice(vertex) * (MODEL_SCALE * dummy.scale))
                + transform.translation;
            verts.push(point.to_array());

            part_local.push([vertex[0], vertex[1], vertex[2]]);
        }
        parts[object_index] = Some((base as usize, part_local));

        indices.extend(object.indices.iter().map(|index| base + *index));
    }
    (verts, indices, parts)
}

pub(crate) fn is_solid_prop(dummy: &meteor_asset::des::StageObject) -> bool {
    if dummy.collision_attr == Some(0) || !dummy.visible {
        return false;
    }

    if dummy.collision_attr == Some(1) {
        return true;
    }
    if dummy.is_hell || dummy.damage_name > 0 {
        return false;
    }
    if custom_field_equals_ci(&dummy.custom, "blockplayer", "yes") {
        return true;
    }
    let Some(model) = dummy.model.as_deref() else {
        return false;
    };
    is_solid_model_pattern(&model.to_lowercase())
}

fn is_collision_capable_prop(dummy: &meteor_asset::des::StageObject) -> bool {
    if dummy.collision_attr == Some(1) {
        return true;
    }
    if dummy.is_hell || dummy.damage_name > 0 {
        return false;
    }
    if custom_field_equals_ci(&dummy.custom, "blockplayer", "yes") {
        return true;
    }
    dummy
        .model
        .as_deref()
        .is_some_and(|model| is_solid_model_pattern(&model.to_ascii_lowercase()))
}

fn custom_field_equals_ci(custom: &str, key: &str, want: &str) -> bool {
    let lower = custom.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let want = want.to_ascii_lowercase();
    let mut start = 0usize;
    while let Some(rel) = lower[start..].find(key.as_str()) {
        let abs = start + rel;
        if abs > 0 {
            let prev = lower.as_bytes()[abs - 1] as char;
            if prev.is_ascii_alphanumeric() || prev == '_' {
                start = abs + 1;
                continue;
            }
        }
        let after = lower[abs + key.len()..].trim_start();
        let Some(rest) = after.strip_prefix('=') else {
            start = abs + 1;
            continue;
        };
        let rest = rest.trim_start().trim_start_matches('"');
        if rest.starts_with(want.as_str()) {
            let after_val = &rest[want.len()..];
            if after_val.is_empty()
                || after_val.starts_with(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            {
                return true;
            }
        }
        start = abs + 1;
    }
    false
}

fn prop_pose_clip_end(pose: &meteor_asset::fmc::PropPose) -> u32 {
    if pose.end > 0 {
        pose.end
    } else {
        pose.start
    }
}

fn requested_prop_pose(
    poses: &[meteor_asset::fmc::PropPose],
    requested: u32,
    has_frames: bool,
) -> Option<(u32, u32, u32)> {
    let index = (requested as usize).min(poses.len().checked_sub(1)?);
    let pose = poses[index];
    let end = prop_pose_clip_end(&pose);
    let state = if has_frames && end > pose.start { 1 } else { 3 };
    Some((pose.start, end, state))
}

fn is_solid_model_pattern(key: &str) -> bool {
    let key = key.to_ascii_lowercase();

    if key.len() >= 5 && key.starts_with("sn0") && key.as_bytes()[3].is_ascii_digit() {
        let rest = &key[4..];
        if rest.starts_with("st") || rest.starts_with('b') || rest.starts_with("door") {
            return true;
        }
    }

    if let Some(after) = key.strip_prefix("sn") {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let rest = &after[digits..];
            if rest.starts_with("door") || rest.starts_with("st") {
                return true;
            }
        }
    }

    if key.starts_with("st") && key.len() <= 3 && key[2..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    key.starts_with("bbox")
        || key.starts_with("box")
        || key.starts_with("chair")
        || key.starts_with("desk")
        || key.starts_with("jug")
        || key.starts_with("knife")
        || key.starts_with("floor")
        || key.starts_with("pdoor")
        || key.starts_with("button")
}

fn prop_bounds(mesh: &meteor_asset::gmdl::GmdlMesh, scale: f32) -> (Vec3, Vec3) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for object in &mesh.objects {
        for v in object.positions.chunks_exact(3) {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
    }
    if !min[0].is_finite() {
        return (Vec3::ZERO, Vec3::splat(0.5));
    }
    let factor = MODEL_SCALE * scale;
    let center = Vec3::new(
        (max[0] + min[0]) / 2.0,
        (max[1] + min[1]) / 2.0,
        (max[2] + min[2]) / 2.0,
    ) * factor;
    let half_extents = Vec3::new(
        (max[0] - min[0]) / 2.0,
        (max[1] - min[1]) / 2.0,
        (max[2] - min[2]) / 2.0,
    ) * factor;
    (center, half_extents)
}

fn prop_transform(dummy: &meteor_asset::des::StageObject, center: [f32; 3]) -> Transform {
    let p = dummy.position;
    let q = dummy.quaternion;
    Transform {
        translation: Vec3::new(
            (p.x - center[0]) * MODEL_SCALE,
            (p.z - center[2]) * MODEL_SCALE,
            -(p.y - center[1]) * MODEL_SCALE,
        ),
        rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
            * Quat::from_xyzw(q.x, q.y, q.z, q.w),
        scale: Vec3::splat(0.12 * dummy.scale),
    }
}

fn build_prop_mesh(object: &meteor_asset::gmdl::GmdlObject, indices: &[u32]) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        object
            .positions
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        object
            .normals
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        object
            .uvs
            .chunks_exact(2)
            .map(|v| [v[0], v[1]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_COLOR,
        object
            .colors
            .chunks_exact(4)
            .map(gmdl_vertex_color)
            .collect::<Vec<_>>(),
    )
    .with_inserted_indices(Indices::U32(indices.to_vec()))
}

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<super::stage_material::FrameUniforms>();
        app.insert_resource(ClearColor(Color::BLACK))
            .insert_resource(StageCollision::default())
            .insert_resource(StageCollider::default())
            .init_resource::<StageTextureHandles>()
            .init_resource::<OfflineWeaponDropSerial>()
            .add_message::<OfflineWeaponDeathDrop>()
            .add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin)
            .add_systems(
                OnEnter(crate::app_state::AppState::Playing),
                setup_stage_scene.after(crate::plugins::stage_plugin::start_stage_script),
            )
            .add_systems(
                OnExit(crate::app_state::AppState::Playing),
                (
                    clear_playing_scene,
                    clear_stage_collision,
                    clear_stage_mesh,
                    clear_stage_texture_handles,
                ),
            )
            .add_systems(
                Update,
                (
                    spawn_offline_weapon_drop,
                    spawn_offline_weapon_death_drops,
                    sync_network_weapon_drops,
                    advance_network_weapon_drop_flight,
                    advance_dynamic_weapon_drop_pickup_delay,
                    sync_stage_texture_quality,
                )
                    .chain()
                    .after(crate::plugins::input_plugin::capture_local_command_input)
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            )
            .add_systems(
                Update,
                (
                    update_uv_scroll,
                    gem_visual::update_gem_visuals.after(update_uv_scroll),
                    (update_fmc_clips, sync_fmc_with_script).chain(),
                    update_scene_deform,
                )
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            )
            .add_systems(
                PostUpdate,
                (update_scene_billboard, update_prop_billboard)
                    .before(bevy::transform::TransformSystems::Propagate)
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            );
    }
}

#[derive(Component)]
pub struct UvScroll {
    pub material: Handle<GmdlMaterial>,
    pub u: f32,
    pub v: f32,
}

#[derive(Component)]
pub struct PropBillboard;

#[derive(Component)]
struct SceneBillboard;

#[derive(Component, Debug, Clone)]
pub struct DynamicWeaponDrop {
    pub reference: String,
}

#[derive(Resource, Default)]
struct OfflineWeaponDropSerial(u32);

#[derive(Message, Debug, Clone)]
pub struct OfflineWeaponDeathDrop {
    pub model: String,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
}

#[derive(Component, Debug, Clone, Copy)]
struct DynamicWeaponDropFlight {
    start: Vec3,
    velocity: Vec3,
    elapsed: f32,
}

#[derive(Component, Debug, Clone, Copy, Default)]
struct DynamicWeaponDropPickupDelay {
    elapsed: f32,
}

const NETWORK_WEAPON_DROP_HEIGHT: f32 = 50.0 * MODEL_SCALE;
const NETWORK_WEAPON_DROP_GRAVITY: f32 = 882.0 * MODEL_SCALE;
const NETWORK_WEAPON_DROP_TIMEOUT: f32 = 5.0;

fn dynamic_weapon_drop_pickup_ready(elapsed: f32, in_flight: bool) -> bool {
    elapsed <= NETWORK_WEAPON_DROP_TIMEOUT && !in_flight
}

fn prepare_offline_weapon_drop(
    fighter: &mut crate::components::fighter::Fighter,
    remove_spare_slot: bool,
) -> Option<String> {
    let (weapon, variant) = if remove_spare_slot {
        let spare_slot = 1 - usize::from(fighter.weapon_slot.min(1));
        (
            fighter.weapon_slots[spare_slot],
            fighter.weapon_slot_variants[spare_slot],
        )
    } else {
        (fighter.weapon, fighter.weapon_variant)
    };
    let model = crate::plugins::character_plugin::weapon_model_stem(weapon, variant)?;
    if remove_spare_slot && crate::plugins::item_plugin::drop_spare_weapon(fighter).is_none() {
        return None;
    }
    Some(model)
}

#[allow(clippy::too_many_arguments)]
fn spawn_offline_weapon_drop(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<crate::app_state::Keymap>,
    combat_input: Option<Res<crate::plugins::input_plugin::LocalCombatInput>>,
    mut dialogue: Option<ResMut<crate::plugins::ui_plugin::DialogueState>>,
    player_performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    net: Res<crate::plugins::network_plugin::NetState>,
    mut serial: ResMut<OfflineWeaponDropSerial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    loaded: Res<crate::app_state::LoadedStageInfo>,
    mut local: Query<
        (
            Entity,
            &mut crate::components::fighter::Fighter,
            &Transform,
            &crate::components::fighter::MotionState,
        ),
        With<crate::plugins::stage_plugin::LocalFighter>,
    >,
) {
    let input_blocked = player_performance
        .as_deref()
        .is_some_and(|performance| performance.input_blocked);
    use crate::plugins::input_plugin::LocalCombatInputEdge as Edge;
    let drop_actions = crate::plugins::input_plugin::ordered_local_discrete_actions(
        combat_input.as_deref(),
        &[
            (Edge::DropWeapon, keymap.drop_weapon),
            (Edge::DropItem, keymap.drop_item),
        ],
        &keys,
        &mouse,
    );
    if input_blocked
        || net.status == crate::plugins::network_plugin::NetStatus::InRoom
        || drop_actions.is_empty()
    {
        return;
    }
    let Ok((fighter_entity, mut fighter, transform, motion)) = local.single_mut() else {
        return;
    };
    let forward = meteor_network::role_state::forward_from_yaw(motion.facing);
    let velocity = [forward.x * 12.0, 0.0, forward.z * 12.0];
    let position = (transform.translation + Vec3::Y * NETWORK_WEAPON_DROP_HEIGHT).to_array();
    for action in drop_actions {
        let drop_weapon = action == Edge::DropWeapon;
        let Some(model) = prepare_offline_weapon_drop(&mut fighter, drop_weapon) else {
            if let Some(dialogue) = dialogue.as_deref_mut() {
                dialogue.text = Some(
                    if drop_weapon {
                        "丢武器需要先拾取一把备用武器"
                    } else {
                        "当前没有可丢弃的手持武器"
                    }
                    .into(),
                );
                dialogue.timer = 2.5;
            }
            continue;
        };
        serial.0 = serial.0.wrapping_add(1);
        spawn_network_weapon_drop(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &loaded,
            &crate::plugins::network_plugin::AuthorityWeaponDrop {
                reference: format!("D_wpDropped{}", serial.0),
                model,
                position,
                velocity: Some(velocity),
                settled: false,
                pickup_ready: false,
            },
        );
        if drop_weapon {
            commands
                .entity(fighter_entity)
                .insert(crate::components::fighter::WeaponVariantChanged);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_offline_weapon_death_drops(
    mut commands: Commands,
    net: Res<crate::plugins::network_plugin::NetState>,
    mut events: MessageReader<OfflineWeaponDeathDrop>,
    mut serial: ResMut<OfflineWeaponDropSerial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    loaded: Res<crate::app_state::LoadedStageInfo>,
) {
    for drop in events.read() {
        if net.status == crate::plugins::network_plugin::NetStatus::InRoom {
            continue;
        }
        serial.0 = serial.0.wrapping_add(1);
        spawn_network_weapon_drop(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &loaded,
            &crate::plugins::network_plugin::AuthorityWeaponDrop {
                reference: format!("D_wpDropped{}", serial.0),
                model: drop.model.clone(),
                position: drop.position,
                velocity: Some(drop.velocity),
                settled: false,
                pickup_ready: false,
            },
        );
    }
}

fn stage_asset_roots(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    crate::plugins::audio_plugin::resource_roots(root)
}

#[allow(clippy::too_many_arguments)]
fn spawn_network_weapon_drop(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
    loaded: &crate::app_state::LoadedStageInfo,
    drop: &crate::plugins::network_plugin::AuthorityWeaponDrop,
) {
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let candidates = stage_asset_roots(&root);
    let pak_root = pak_files_root(&candidates, &loaded.stage_id);
    let stems = model_stems(&drop.model);
    let key = stems[0].to_ascii_lowercase();
    let model_mesh = load_prop_model(&candidates, &stems);
    let position = Vec3::from_array(drop.position);
    let flight = drop.velocity.and_then(|velocity| {
        let velocity = Vec3::from_array(velocity);
        velocity.is_finite().then_some(DynamicWeaponDropFlight {
            start: position,
            velocity,
            elapsed: 0.0,
        })
    });
    let (center_offset, half_extents) = model_mesh
        .as_ref()
        .map_or((Vec3::ZERO, Vec3::splat(0.5)), |mesh| {
            prop_bounds(mesh, 1.0)
        });
    let root_entity = commands
        .spawn((
            PlayingSceneEntity,
            DynamicWeaponDrop {
                reference: drop.reference.clone(),
            },
            crate::plugins::item_plugin::StageProp {
                name: drop.reference.clone(),
                model: key.clone(),
                center_offset,
                half_extents,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: drop.pickup_ready,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
            Visibility::Visible,
            Transform {
                translation: position,
                rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                scale: Vec3::splat(MODEL_SCALE),
            },
        ))
        .id();
    if let Some(flight) = flight {
        commands.entity(root_entity).insert(flight);
    }
    if !drop.pickup_ready {
        commands
            .entity(root_entity)
            .insert(DynamicWeaponDropPickupDelay::default());
    }
    let Some(model_mesh) = model_mesh else {
        warn!("network dropped weapon model missing: {}", drop.model);
        return;
    };
    let mut pak_cache = PakCache::default();
    let mut image_cache = HashMap::<String, LoadedStageTexture>::new();
    let mut material_cache = HashMap::<i32, Handle<GmdlMaterial>>::new();
    for object in &model_mesh.objects {
        if object.indices.is_empty() || object.positions.is_empty() {
            continue;
        }
        let mut groups = HashMap::<i32, Vec<u32>>::new();
        for (face, material_index) in object.face_mat.iter().enumerate() {
            if !is_drawable_material_index(*material_index, model_mesh.materials.len()) {
                continue;
            }
            groups
                .entry(*material_index)
                .or_default()
                .extend_from_slice(&object.indices[face * 3..face * 3 + 3]);
        }
        for (material_index, indices) in groups {
            let material = &model_mesh.materials[material_index as usize];
            let (mode, opacity, mut alpha_cutoff) = classify_gmdl_material(material);
            let loaded_texture = model_mesh
                .resolve_texture_name(material)
                .filter(|name| !name.to_ascii_lowercase().ends_with(".ifl"))
                .map(|name| {
                    load_or_cache_texture(
                        images,
                        &mut image_cache,
                        &mut pak_cache,
                        &pak_root,
                        &loaded.stage_id,
                        name,
                        material.blend.eq_ignore_ascii_case("COLORKEY"),
                        material.blend.eq_ignore_ascii_case("ALPHACHANNEL"),
                        0,
                    )
                })
                .unwrap_or_default();
            if loaded_texture.alpha_test && mode == GmdlMode::Opaque {
                alpha_cutoff = 127.0 / 255.0;
            }
            let texture = loaded_texture.handle;
            let material_handle = material_cache
                .entry(material_index)
                .or_insert_with(|| {
                    materials.add(GmdlMaterial::new(
                        GmdlMaterialParams {
                            opacity,
                            alpha_cutoff,
                            mode: mode.as_u32(),
                            has_texture: texture.is_some() as u32,
                            uv_offset: Vec2::ZERO,
                        },
                        texture,
                        material.two_side,
                        mode,
                    ))
                })
                .clone();
            commands
                .spawn((
                    PlayingSceneEntity,
                    Mesh3d(meshes.add(build_prop_mesh(object, &indices))),
                    MeshMaterial3d(material_handle),
                    Transform::IDENTITY,
                ))
                .insert(ChildOf(root_entity));
        }
    }
}

#[allow(clippy::too_many_arguments)]
type DynamicWeaponDropSyncQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static DynamicWeaponDrop,
        Option<&'static DynamicWeaponDropFlight>,
        Option<&'static DynamicWeaponDropPickupDelay>,
        &'static mut Transform,
    ),
>;

#[allow(clippy::too_many_arguments)]
fn sync_network_weapon_drops(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    loaded: Res<crate::app_state::LoadedStageInfo>,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    mut existing: DynamicWeaponDropSyncQuery<'_, '_>,
    mut props: Query<&mut crate::plugins::item_plugin::StageProp, With<DynamicWeaponDrop>>,
) {
    if net.status != crate::plugins::network_plugin::NetStatus::InRoom {
        return;
    }
    let throw_events = std::mem::take(&mut net.pending_weapon_drops);
    let mut known = existing
        .iter()
        .map(|(_, drop, _, _, _)| drop.reference.clone())
        .collect::<std::collections::HashSet<_>>();
    for drop in throw_events {
        if !net.dynamic_weapon_drops.contains_key(&drop.reference) {
            continue;
        }
        if let Some((entity, _, _, _, mut transform)) = existing
            .iter_mut()
            .find(|(_, existing, _, _, _)| existing.reference == drop.reference)
        {
            let position = Vec3::from_array(drop.position);
            if let Some(velocity) = drop
                .velocity
                .map(Vec3::from_array)
                .filter(|v| v.is_finite())
            {
                if let Ok(mut prop) = props.get_mut(entity) {
                    prop.interactive = false;
                }
                commands.entity(entity).insert((
                    Transform {
                        translation: position,
                        rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                        scale: Vec3::splat(MODEL_SCALE),
                    },
                    DynamicWeaponDropFlight {
                        start: position,
                        velocity,
                        elapsed: 0.0,
                    },
                    DynamicWeaponDropPickupDelay::default(),
                ));
            } else {
                if let Ok(mut prop) = props.get_mut(entity) {
                    prop.interactive = drop.pickup_ready;
                }
                transform.translation = position;
                transform.rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                transform.scale = Vec3::splat(MODEL_SCALE);
                commands.entity(entity).remove::<DynamicWeaponDropFlight>();
            }
            continue;
        }
        spawn_network_weapon_drop(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &loaded,
            &drop,
        );
        known.insert(drop.reference.clone());
    }
    for drop in net.dynamic_weapon_drops.values() {
        if known.contains(&drop.reference) {
            continue;
        }
        spawn_network_weapon_drop(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &loaded,
            drop,
        );
    }
    for (entity, drop, flight, pickup_delay, mut transform) in &mut existing {
        let Some(authority) = net.dynamic_weapon_drops.get(&drop.reference) else {
            commands.entity(entity).despawn();
            continue;
        };
        if authority.settled {
            transform.translation = Vec3::from_array(authority.position);
            if let Ok(mut prop) = props.get_mut(entity) {
                prop.interactive = authority.pickup_ready;
            }
            if flight.is_some() {
                commands.entity(entity).remove::<DynamicWeaponDropFlight>();
            }
            if authority.pickup_ready && pickup_delay.is_some() {
                commands
                    .entity(entity)
                    .remove::<DynamicWeaponDropPickupDelay>();
            }
        } else if flight.is_none() {
            if let Ok(mut prop) = props.get_mut(entity) {
                prop.interactive = false;
            }
        }
    }
}

fn advance_network_weapon_drop_flight(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    collider: Res<StageCollider>,
    mut commands: Commands,
    mut net: Option<ResMut<crate::plugins::network_plugin::NetState>>,
    tx: Option<Res<crate::plugins::network_plugin::NetTx>>,
    mut drops: Query<(
        Entity,
        &mut Transform,
        &mut crate::plugins::item_plugin::StageProp,
        &mut DynamicWeaponDropFlight,
        &DynamicWeaponDrop,
    )>,
) {
    let mut stage = CollisionSystem::new(0.55, 1.8);
    if let Some(mesh) = collider.mesh.as_ref() {
        stage.set_mesh_collider(mesh);
    }
    for (entity, mut transform, mut prop, mut flight, drop) in &mut drops {
        flight.elapsed += crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
        if flight.elapsed > NETWORK_WEAPON_DROP_TIMEOUT {
            commands.entity(entity).despawn();
            if let Some(net) = net.as_deref_mut() {
                net.dynamic_weapon_drops.remove(&drop.reference);
            }
            continue;
        }
        let next = flight.start + flight.velocity * flight.elapsed
            - Vec3::Y * (0.5 * NETWORK_WEAPON_DROP_GRAVITY * flight.elapsed.powi(2));

        let previous = transform.translation;
        let hit = stage.segment_hit(
            Point3::new(previous.x, previous.y, previous.z),
            Point3::new(next.x, next.y, next.z),
        );
        if let Some(hit) = hit {
            let rest = previous + (next - previous).normalize_or_zero() * hit.t;
            transform.translation = rest;
            if let Some(net) = net.as_deref_mut() {
                use crate::plugins::network_plugin::NetStatus;
                if net.status == NetStatus::InRoom {
                    if let Some(tx) = tx.as_ref() {
                        if net.is_stock_udp() {
                            #[cfg(not(target_arch = "wasm32"))]
                            if let Some(id) = drop
                                .reference
                                .strip_prefix("D_wpStockThrow")
                                .and_then(|id| id.parse().ok())
                            {
                                tx.send_stock_weapon_drop_rest(id, rest.to_array());
                            }
                        } else if net.is_scene_authority() {
                            tx.send_control(meteor_network::protocol::Message::WeaponDropRest {
                                match_epoch: net.match_epoch,
                                round_epoch: net.round_epoch,
                                reference: drop.reference.clone(),
                                position: rest.to_array(),
                            });
                        }
                    }

                    if let Some(view) = net.dynamic_weapon_drops.get_mut(&drop.reference) {
                        view.position = rest.to_array();
                        view.velocity = None;
                        view.settled = true;
                    }
                }
            }
            prop.interactive = false;
            commands.entity(entity).remove::<DynamicWeaponDropFlight>();
            continue;
        }
        transform.translation = next;
    }
}

fn advance_dynamic_weapon_drop_pickup_delay(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut commands: Commands,
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    mut drops: Query<(
        Entity,
        &mut crate::plugins::item_plugin::StageProp,
        &mut DynamicWeaponDropPickupDelay,
        Option<&DynamicWeaponDropFlight>,
    )>,
) {
    if net.is_some_and(|net| net.status == crate::plugins::network_plugin::NetStatus::InRoom) {
        return;
    }
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (entity, mut prop, mut delay, flight) in &mut drops {
        delay.elapsed += dt;

        if delay.elapsed > NETWORK_WEAPON_DROP_TIMEOUT {
            commands.entity(entity).despawn();
        } else if dynamic_weapon_drop_pickup_ready(delay.elapsed, flight.is_some()) {
            prop.interactive = true;
            commands
                .entity(entity)
                .remove::<DynamicWeaponDropPickupDelay>();
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct PropColliderOwner(pub i32);

#[derive(Component)]
pub struct SceneDeform {
    pub freq: f32,
    pub size: f32,
    pub range: f32,
    pub references: Vec<f32>,
    pub base_positions: Vec<[f32; 3]>,
    pub offset_axis: Vec3,
    pub time: f32,
}

fn compile_deform_ref_mask(ref_text: Option<&str>) -> u32 {
    let text = ref_text.unwrap_or("").to_lowercase();
    if text.is_empty() {
        return 1 | 2 | 4;
    }
    let mut mask = 0;
    if text.contains('x') {
        mask |= 1;
    }
    if text.contains('y') {
        mask |= 2;
    }
    if text.contains('z') {
        mask |= 4;
    }
    if mask == 0 {
        1 | 2 | 4
    } else {
        mask
    }
}

fn deform_reference_value(pos: [f32; 3], ref_text: Option<&str>) -> f32 {
    let mask = compile_deform_ref_mask(ref_text);
    let mut value = 0.0;
    if mask & 1 != 0 {
        value += pos[0];
    }
    if mask & 2 != 0 {
        value += pos[1];
    }
    if mask & 4 != 0 {
        value += pos[2];
    }
    value
}

#[derive(Component)]
pub struct FmcClip {
    pub name: String,
    pub clip: meteor_asset::fmc::FmcClip,
    pub part_index: usize,

    pub poses: Vec<meteor_asset::fmc::PropPose>,

    pub pose_index: u32,

    pub pose_start: u32,
    pub pose_end: u32,

    pub pose_loop: u32,

    pub pose_revision: u64,

    pub anim_time: f32,

    pub state: u32,

    pub frame_set: Option<u32>,

    pub multi: bool,

    pub base_translation: Vec3,
    pub base_rotation: Quat,

    pub collider_owner: Option<i32>,

    pub collider_vertex_offset: usize,

    pub local_verts: Vec<[f32; 3]>,

    pub last_reported_state: u32,
}

fn uv_scroll_offset(base: Vec2, scroll: Vec4, time: Vec2) -> Vec2 {
    if scroll.xy() == Vec2::ZERO {
        return base;
    }
    let elapsed = time - scroll.zw();
    let wrap = |v: Vec2| Vec2::new(v.x.rem_euclid(1.0), v.y.rem_euclid(1.0));
    wrap(base + wrap(scroll.xy() * elapsed.x) + wrap(scroll.xy() * elapsed.y))
}

fn update_uv_scroll(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    query: Query<&UvScroll>,
    mut frame: ResMut<super::stage_material::FrameUniforms>,
    mut previous: Local<std::collections::HashMap<AssetId<GmdlMaterial>, Vec2>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let epoch = super::stage_material::split_time(frame.seconds);

    let mut velocities = std::collections::HashMap::<AssetId<GmdlMaterial>, Vec2>::new();
    for scroll in &query {
        *velocities.entry(scroll.material.id()).or_default() += Vec2::new(scroll.u, scroll.v);
    }
    for (&id, &old_speed) in previous.iter() {
        if old_speed != Vec2::ZERO {
            velocities.entry(id).or_insert(Vec2::ZERO);
        }
    }
    for (&id, &speed) in &velocities {
        if previous.get(&id) == Some(&speed) {
            continue;
        }
        if let Some(mut mat) = materials.get_mut(id) {
            let old = mat.uv_scroll;
            mat.params.uv_offset = uv_scroll_offset(mat.params.uv_offset, old, epoch);
            mat.uv_scroll = Vec4::new(speed.x, speed.y, epoch.x, epoch.y);
            #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
            super::perf_plugin::count_event(3);
        }
    }
    velocities.retain(|id, _| materials.get(*id).is_some());
    *previous = velocities;
    frame.seconds += f64::from(dt);
}

pub(crate) fn update_fmc_clips(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut collider: ResMut<StageCollider>,
    mut query: Query<(&mut Transform, &mut FmcClip)>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(8);
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (mut transform, mut clip) in &mut query {
        let start = clip.pose_start;
        let end = clip.pose_end;
        if clip.clip.frames.is_empty() {
            clip.state = 3;
            continue;
        }

        if end <= start {
            clip.frame_set = None;
            clip.anim_time = 0.0;
        } else if let Some(frame_set) = clip.frame_set.take() {
            clip.anim_time = frame_set as f32 / clip.clip.fps.max(1.0);
        } else {
            clip.anim_time += dt;
        }
        let fps = clip.clip.fps.max(1.0);
        let span = end.saturating_sub(start);
        let raw = clip.anim_time * fps;
        let frame_offset = if clip.pose_loop == 1 && span > 0 && raw > span as f32 {
            let remainder = raw % span as f32;
            if remainder == 0.0 {
                span as f32
            } else {
                remainder
            }
        } else {
            raw.min(span as f32)
        };
        clip.state = if span > 0 && (clip.pose_loop == 1 || raw < span as f32) {
            1
        } else {
            3
        };
        let rest = &clip.clip.frames[0];
        let Some(part) = clip
            .clip
            .sample_part(start as f32 + frame_offset, clip.part_index)
        else {
            continue;
        };
        let rest_part = rest
            .parts
            .get(clip.part_index)
            .cloned()
            .unwrap_or_else(|| rest.parts[0].clone());

        let q_delta = Quat::from_xyzw(part.q.x, part.q.y, part.q.z, part.q.w)
            * Quat::from_xyzw(rest_part.q.x, rest_part.q.y, rest_part.q.z, rest_part.q.w).inverse();
        if clip.multi {
            let pivot = q_delta * Vec3::new(rest_part.t.x, rest_part.t.y, rest_part.t.z);
            let part_pos = Vec3::new(part.t.x - pivot.x, part.t.y - pivot.y, part.t.z - pivot.z);
            let root = Transform {
                translation: clip.base_translation,
                rotation: clip.base_rotation,
                scale: transform.scale,
            };
            let part_t = Transform::from_translation(part_pos).with_rotation(q_delta);
            let combined = root * part_t;
            transform.translation = combined.translation;
            transform.rotation = combined.rotation;
        } else {
            transform.translation = clip.base_translation
                + Vec3::new(
                    (part.t.x - rest_part.t.x) * MODEL_SCALE,
                    (part.t.z - rest_part.t.z) * MODEL_SCALE,
                    -(part.t.y - rest_part.t.y) * MODEL_SCALE,
                );
            transform.rotation = clip.base_rotation * q_delta;
        }

        if let Some(owner) = clip.collider_owner {
            if !clip.local_verts.is_empty() {
                let mut world: Vec<[f32; 3]> = Vec::with_capacity(clip.local_verts.len());
                for v in &clip.local_verts {
                    let p = transform.rotation * (Vec3::from_slice(v) * transform.scale)
                        + transform.translation;
                    world.push([p.x, p.y, p.z]);
                }
                if let Some(mesh) = &mut collider.mesh {
                    mesh.update_owner_vertices_range(owner, clip.collider_vertex_offset, &world);
                }
            }
        }
    }
}

pub(crate) fn sync_fmc_with_script(
    mut script: ResMut<crate::plugins::stage_plugin::StageScriptRuntime>,
    mut query: Query<(Entity, &mut FmcClip)>,
) {
    let Some(runtime) = script.script_mut() else {
        return;
    };

    let by_name: std::collections::HashMap<String, usize> = runtime
        .host()
        .instances()
        .iter()
        .enumerate()
        .map(|(i, inst)| (inst.name.to_lowercase(), i))
        .collect();

    let frame_commands: std::collections::HashMap<String, u32> = runtime
        .host()
        .instances()
        .iter()
        .filter_map(|instance| {
            let frame = instance.get("frame").as_f64();
            (frame.is_finite() && frame >= 0.0)
                .then_some((instance.name.to_lowercase(), frame as u32))
        })
        .collect();
    for (_entity, mut clip) in &mut query {
        let Some(&inst_index) = by_name.get(&clip.name.to_lowercase()) else {
            continue;
        };
        runtime.host_mut().instances_mut()[inst_index].set(
            "_has_fmc",
            meteor_scene::pst_runtime::PstValue::Num(if clip.clip.frames.is_empty() {
                0.0
            } else {
                1.0
            }),
        );

        let (pose, pose_loop, pose_revision) = {
            let inst = &runtime.host().instances()[inst_index];
            (
                inst.get("pose").as_f64() as u32,
                inst.get("pose_loop").as_f64() as u32,
                inst.get("_pose_revision").as_f64().max(0.0) as u64,
            )
        };

        if pose != clip.pose_index || pose_revision != clip.pose_revision {
            clip.pose_index = pose;
            clip.pose_revision = pose_revision;
            clip.anim_time = 0.0;
            if let Some((start, end, state)) =
                requested_prop_pose(&clip.poses, pose, !clip.clip.frames.is_empty())
            {
                clip.pose_start = start;
                clip.pose_end = end;
                clip.state = state;
            }
        }
        clip.pose_loop = pose_loop;

        if let Some(frame) = frame_commands.get(&clip.name.to_lowercase()) {
            clip.frame_set = Some(*frame);
        }

        let inst = &mut runtime.host_mut().instances_mut()[inst_index];
        inst.set(
            "anim_time",
            meteor_scene::pst_runtime::PstValue::Num(clip.anim_time as f64),
        );
        if clip.state != clip.last_reported_state {
            inst.set(
                "state",
                meteor_scene::pst_runtime::PstValue::Num(clip.state as f64),
            );
            clip.last_reported_state = clip.state;
        }
    }
    for name in frame_commands.keys() {
        if let Some(index) = by_name.get(name) {
            runtime.host_mut().instances_mut()[*index]
                .set("frame", meteor_scene::pst_runtime::PstValue::Num(-1.0));
        }
    }
}
fn update_scene_deform(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut query: Query<(&Mesh3d, &mut SceneDeform)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(9);
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (mesh, mut deform) in &mut query {
        deform.time += dt;
        if deform.size == 0.0 || dt == 0.0 {
            continue;
        }
        let Some(mut mesh) = meshes.get_mut(&mesh.0) else {
            continue;
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        deform.write_positions(positions);
    }
}

impl SceneDeform {
    fn from_mesh(
        object: &meteor_asset::gmdl::GmdlObject,
        mesh: &Mesh,
        obj: &meteor_asset::des::StageObject,
    ) -> Option<Self> {
        let d = obj.deform.as_ref()?;
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            unreachable!("scene positions must be Float32x3");
        };
        let offset_axis = if obj.billboard == 1 {
            Quat::from_xyzw(
                obj.quaternion.x,
                obj.quaternion.y,
                obj.quaternion.z,
                obj.quaternion.w,
            )
            .normalize()
            .inverse()
                * Vec3::ONE
        } else {
            Vec3::ONE
        };
        Some(Self {
            freq: d.freq,
            size: d.size,
            range: d.range,
            references: object
                .positions
                .chunks_exact(3)
                .map(|v| deform_reference_value([v[0], v[1], v[2]], d.r#ref.as_deref()))
                .collect(),
            base_positions: positions.clone(),
            offset_axis,
            time: 0.0,
        })
    }

    fn write_positions(&self, positions: &mut [[f32; 3]]) {
        for ((position, base), reference) in positions
            .iter_mut()
            .zip(&self.base_positions)
            .zip(&self.references)
        {
            let off = (self.time * self.freq * std::f32::consts::TAU
                + reference / (self.size * std::f32::consts::TAU))
                .sin()
                * self.range;
            *position = (Vec3::from_array(*base) + self.offset_axis * off).to_array();
        }
    }
}

fn update_prop_billboard(
    camera: Query<&Transform, With<crate::plugins::camera_plugin::GameplayCamera>>,
    mut query: Query<
        &mut Transform,
        (
            With<PropBillboard>,
            Without<crate::plugins::camera_plugin::GameplayCamera>,
        ),
    >,
) {
    let Ok(cam) = camera.single() else {
        return;
    };
    let cam_pos = cam.translation;
    for mut transform in &mut query {
        transform.look_at(cam_pos, Vec3::Y);
    }
}

fn update_scene_billboard(
    camera: Query<&Transform, With<crate::plugins::camera_plugin::GameplayCamera>>,
    mut query: Query<
        &mut Transform,
        (
            With<SceneBillboard>,
            Without<crate::plugins::camera_plugin::GameplayCamera>,
        ),
    >,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    for mut transform in &mut query {
        let away = transform.translation - camera.translation;
        if away.length_squared() > f32::EPSILON {
            transform.look_to(away, Vec3::Y);
        }
    }
}

#[derive(Component)]
struct PlayingSceneEntity;

#[derive(Component)]
struct StageSkyDome;

fn clear_playing_scene(mut commands: Commands, query: Query<Entity, With<PlayingSceneEntity>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn blend_token_index(token: &str) -> Option<u32> {
    match token.to_ascii_lowercase().as_str() {
        "0" => Some(0),
        "1" => Some(1),
        "s" => Some(2),
        "d" => Some(3),
        "1-s" => Some(4),
        "1-d" => Some(5),
        "sa" => Some(6),
        "1-sa" => Some(7),
        "da" => Some(8),
        "1-da" => Some(9),
        _ => None,
    }
}

pub(crate) fn classify_gmdl_material(mat: &RawGmdlMaterial) -> (GmdlMode, f32, f32) {
    let parts: Vec<&str> = mat.state.split_whitespace().collect();
    let enabled = parts
        .first()
        .is_some_and(|s| s.eq_ignore_ascii_case("ENABLE"));
    let opacity = mat.alpha;
    if enabled {
        let src = parts.get(1).and_then(|t| blend_token_index(t));
        let dst = parts.get(2).and_then(|t| blend_token_index(t));
        let mode = match (src, dst) {
            (Some(6), Some(7)) => GmdlMode::Blend,
            (Some(6), Some(1)) => GmdlMode::Add,
            (Some(0), Some(4)) => GmdlMode::MultiplyShadow,
            (Some(2), Some(1)) => GmdlMode::SrcColorOne,
            _ => GmdlMode::Blend,
        };
        (mode, opacity, 0.0)
    } else if mat.blend == "COLORKEY" {
        (GmdlMode::Opaque, 1.0, 127.0 / 255.0)
    } else {
        (GmdlMode::Opaque, 1.0, 0.0)
    }
}

fn is_drawable_material_index(mat_idx: i32, material_count: usize) -> bool {
    mat_idx >= 0 && (mat_idx as usize) < material_count
}

fn sky_name_score(name: &str) -> i32 {
    let value = name.to_ascii_lowercase();
    let mut score = 0;
    if value.contains("sky1") {
        score += 3;
    }
    if value.ends_with(".jpg") || value.ends_with(".jpeg") || value.ends_with(".bmp") {
        score += 1;
    }
    if value.contains('_') {
        score -= 2;
    }
    score
}

fn pick_stage_sky_name(gmdl: &GmdlMesh, stage_id: &str) -> Option<String> {
    let mut authored: Vec<&str> = gmdl
        .textures
        .iter()
        .map(|texture| texture.name.as_str())
        .filter(|name| name.to_ascii_lowercase().contains("sky"))
        .collect();
    authored.sort_by_key(|name| std::cmp::Reverse(sky_name_score(name)));
    if let Some(name) = authored.first() {
        return Some((*name).to_string());
    }
    meteor_scene::stage_registry::is_official_stage_id(stage_id)
        .then(|| format!("{}sky1.jpg", stage_id.to_ascii_lowercase()))
}

fn lerp_srgb(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|axis| {
        (a[axis] as f32 + (b[axis] as f32 - a[axis] as f32) * t).round() as u8
    })
}

fn custom_stage_sky_rgba(size: u32) -> Vec<u8> {
    const STOPS: [(f32, [u8; 3]); 6] = [
        (0.0, [0x1a, 0x2a, 0x4a]),
        (0.3, [0x4a, 0x7a, 0x9a]),
        (0.5, [0x8a, 0x9a, 0xb0]),
        (0.7, [0x6a, 0x7a, 0x7a]),
        (0.85, [0x3a, 0x3a, 0x3a]),
        (1.0, [0x1a, 0x1a, 0x1a]),
    ];
    let mut rgba = vec![0u8; size as usize * size as usize * 4];
    for y in 0..size {
        let t = y as f32 / size.max(1) as f32;
        let upper = STOPS
            .iter()
            .position(|(stop, _)| *stop >= t)
            .unwrap_or(STOPS.len() - 1);
        let lower = upper.saturating_sub(1);
        let span = (STOPS[upper].0 - STOPS[lower].0).max(f32::EPSILON);
        let color = lerp_srgb(STOPS[lower].1, STOPS[upper].1, (t - STOPS[lower].0) / span);
        for x in 0..size {
            let index = ((y * size + x) * 4) as usize;
            rgba[index..index + 3].copy_from_slice(&color);
            rgba[index + 3] = 255;
        }
    }

    let mut seed = 0x4d45_5445u32;
    let mut random = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed as f32 / (u32::MAX as f32 + 1.0)
    };
    for _ in 0..200 {
        let cx = random() * size as f32;
        let cy = size as f32 * (0.2 + random() * 0.4);
        let radius = size as f32 * (0.04 + random() * 0.12);
        let alpha = 0.03 + random() * 0.06;
        let min_x = (cx - radius).floor().max(0.0) as u32;
        let max_x = (cx + radius).ceil().min(size as f32 - 1.0) as u32;
        let min_y = (cy - radius).floor().max(0.0) as u32;
        let max_y = (cy + radius).ceil().min(size as f32 - 1.0) as u32;
        let radius_sq = radius * radius;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if dx * dx + dy * dy > radius_sq {
                    continue;
                }
                let index = ((y * size + x) * 4) as usize;
                for channel in &mut rgba[index..index + 3] {
                    *channel = (*channel as f32 * (1.0 - alpha) + 255.0 * alpha).round() as u8;
                }
            }
        }
    }
    rgba
}

fn set_sky_sampler(image: &mut bevy::image::Image, repeat_u: bool) {
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: if repeat_u {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        },
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
}

fn custom_stage_sky_handle(images: &mut Assets<bevy::image::Image>) -> Handle<bevy::image::Image> {
    const SIZE: u32 = 512;
    let mut image = bevy::image::Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        custom_stage_sky_rgba(SIZE),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    set_sky_sampler(&mut image, true);
    images.add(image)
}

#[allow(clippy::too_many_arguments)]
fn spawn_stage_sky(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<bevy::image::Image>,
    gmdl: &GmdlMesh,
    stage_id: &str,
) {
    if meteor_scene::stage_registry::is_official_stage_id(stage_id)
        || pick_stage_sky_name(gmdl, stage_id).is_some()
    {
        return;
    }
    let texture = custom_stage_sky_handle(images);
    let mesh = Sphere::new(500.0).mesh().uv(32, 16);
    commands.spawn((
        PlayingSceneEntity,
        StageSkyDome,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(texture),
            unlit: true,
            cull_mode: Some(Face::Front),
            ..default()
        })),
        Transform::IDENTITY,
    ));
}

#[allow(clippy::too_many_arguments)]
fn setup_stage_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    loaded: Res<crate::app_state::LoadedStageInfo>,
    loaded_assets: Res<crate::app_state::LoadedStageAssets>,
    escort: Res<super::escort_assets::LoadedEscort>,
    script: Res<crate::plugins::stage_plugin::StageScriptRuntime>,
    settings: Res<crate::app_state::Settings>,
    mut collision: ResMut<StageCollision>,
    mut stage_collider: ResMut<StageCollider>,
    mut hazards: ResMut<super::hazard_plugin::StageHazardRuntime>,
) {
    hazards.0 = Default::default();
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let roots = stage_asset_roots(&root);
    let pak_root = pak_files_root(&roots, &loaded.stage_id);
    let relative = format!("{}/{}.gmb", loaded.stage_id, loaded.stage_id);
    let candidates: Vec<_> = roots.iter().map(|base| base.join(&relative)).collect();
    load_stage_collision(
        &roots,
        &loaded,
        loaded_assets.cob.as_deref(),
        &mut collision,
    );
    let disk_gmb = candidates
        .iter()
        .find_map(|path| crate::resource_io::read(path).ok());
    let Some(bytes) = loaded_assets.gmb.as_deref().or(disk_gmb.as_deref()) else {
        warn!(
            "stage mesh missing for {} candidates={:?}",
            loaded.stage_id, candidates
        );
        return;
    };
    info!(
        "decoding stage mesh for {} ({} bytes)",
        loaded.stage_id,
        bytes.len()
    );
    let gmdl = match decode_gmdl(bytes) {
        Ok(mesh) => mesh,
        Err(error) => {
            warn!("unable to decode GMB for {}: {error}", loaded.stage_id);
            return;
        }
    };

    let des_path = format!("{}/{}.des", loaded.stage_id, loaded.stage_id);
    let mut stage_desc: Option<meteor_asset::des::StageDescription> = None;
    let des_text = loaded_assets
        .des
        .as_deref()
        .map(crate::text_codec::decode_text)
        .or_else(|| {
            roots
                .iter()
                .find_map(|base| crate::resource_io::read_to_string(base.join(&des_path)).ok())
        });
    if let Some(text) = &des_text {
        let desc = meteor_asset::des::parse_stage_description(text);
        let mesh = StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            (!collision.face_ids.is_empty()).then_some(&collision.face_ids),
            loaded.center,
        );
        info!(
            "stage mesh collider built: {} verts, {} tris (wall {}, floor {}, steep {}, ceiling {})",
            mesh.positions.len(),
            mesh.triangle_count(),
            mesh.wall_tris.len(),
            mesh.floor_tris.len(),
            mesh.steep_tris.len(),
            mesh.ceiling_tris.len(),
        );
        stage_collider.mesh = Some(mesh);
        hazards.0.build_from_stage(
            super::hazard_plugin::build_volumes(&gmdl, &desc, loaded.center),
            vec![],
        );
        stage_desc = Some(desc);
    } else {
        warn!(
            "stage des missing for {} (no mesh collider)",
            loaded.stage_id
        );
    }

    let mut pak_cache = PakCache::default();
    let paks = pak_cache.chain(&pak_root, &loaded.stage_id);
    if paks.is_empty() {
        if meteor_asset::runtime_assets::registered_pack_count() > 0 {
            info!(
                "legacy texture paks not mounted for {}; using runtime MPK texture scopes",
                loaded.stage_id
            );
        } else {
            warn!(
                "no texture packs found for {} (textures unavailable)",
                loaded.stage_id
            );
        }
    } else {
        info!(
            "texture pack chain loaded: {} packs ({} entries total)",
            paks.len(),
            paks.iter().map(PakArchive::len).sum::<usize>()
        );
    }
    spawn_stage_sky(
        &mut commands,
        &mut meshes,
        &mut standard_materials,
        &mut images,
        &gmdl,
        &loaded.stage_id,
    );

    let mut world_min = Vec3::splat(f32::MAX);
    let mut world_max = Vec3::splat(f32::MIN);
    let mut image_cache: HashMap<String, LoadedStageTexture> = HashMap::new();
    let mut stage_texture_handles = Vec::new();
    let mut material_cache: HashMap<(usize, i32), Handle<GmdlMaterial>> = HashMap::new();
    let mut drawn_objects = 0usize;
    let mut drawn_faces = 0usize;
    let mut textured_faces = 0usize;
    let mut ifl_materials = 0usize;
    let mut ifl_frames_total = 0usize;

    let skip_render: HashSet<&str> = stage_desc
        .as_ref()
        .map(|d| {
            d.scene_objects
                .iter()
                .filter(|o| o.skip_render())
                .map(|o| o.name.as_str())
                .collect()
        })
        .unwrap_or_default();
    if !skip_render.is_empty() {
        info!(
            "stage {}: {} helper objects skipped from rendering (camerabox/visible=0)",
            loaded.stage_id,
            skip_render.len()
        );
    }

    for (obj_idx, object) in gmdl.objects.iter().enumerate() {
        if skip_render.contains(object.name.as_str()) {
            continue;
        }
        if object.indices.is_empty() || object.positions.is_empty() {
            continue;
        }

        let mut groups: HashMap<i32, Vec<u32>> = HashMap::new();
        for (face, mat_idx) in object.face_mat.iter().enumerate() {
            if !is_drawable_material_index(*mat_idx, gmdl.materials.len()) {
                continue;
            }
            groups
                .entry(*mat_idx)
                .or_default()
                .extend_from_slice(&object.indices[face * 3..face * 3 + 3]);
        }
        for (mat_idx, indices) in groups {
            drawn_objects += 1;
            drawn_faces += indices.len() / 3;
            let mat = &gmdl.materials[mat_idx as usize];
            let (mode, opacity, mut alpha_cutoff) = classify_gmdl_material(mat);

            let color_key = mat.blend == "COLORKEY";
            let mut ifl_frames: Option<Vec<Handle<bevy::image::Image>>> = None;
            let mut mask_alpha_test = false;
            let texture = match gmdl.resolve_texture_name(mat) {
                Some(name) => {
                    if name.to_ascii_lowercase().ends_with(".ifl") {
                        ifl_materials += 1;
                        ifl_frames = load_ifl_frames(
                            &mut pak_cache,
                            &pak_root,
                            &mut images,
                            name,
                            &loaded.stage_id,
                            color_key,
                            settings.texture_quality,
                        );
                        ifl_frames_total += ifl_frames.as_ref().map_or(0, Vec::len);
                        if let Some(frames) = &ifl_frames {
                            stage_texture_handles.extend(frames.iter().cloned());
                        }
                        ifl_frames.as_ref().and_then(|f| f.first().cloned())
                    } else {
                        textured_faces += indices.len() / 3;
                        let loaded_texture = load_or_cache_texture(
                            &mut images,
                            &mut image_cache,
                            &mut pak_cache,
                            &pak_root,
                            &loaded.stage_id,
                            name,
                            color_key,
                            mat.blend.eq_ignore_ascii_case("ALPHACHANNEL"),
                            settings.texture_quality,
                        );
                        mask_alpha_test = loaded_texture.alpha_test;
                        if let Some(handle) = &loaded_texture.handle {
                            stage_texture_handles.push(handle.clone());
                        }
                        loaded_texture.handle
                    }
                }
                None => None,
            };
            if mask_alpha_test && mode == GmdlMode::Opaque {
                alpha_cutoff = 127.0 / 255.0;
            }
            let material_handle = material_cache
                .entry((obj_idx, mat_idx))
                .or_insert_with(|| {
                    materials.add(GmdlMaterial::new_scene(
                        GmdlMaterialParams {
                            opacity,
                            alpha_cutoff,
                            mode: mode.as_u32(),
                            has_texture: texture.is_some() as u32,
                            uv_offset: Vec2::ZERO,
                        },
                        texture,
                        mat.two_side,
                        mode,
                    ))
                })
                .clone();

            for v in object.positions.chunks_exact(3) {
                let p = Vec3::new(
                    0.12 * (v[0] - loaded.center[0]),
                    0.12 * (v[2] - loaded.center[2]),
                    -0.12 * (v[1] - loaded.center[1]),
                );
                world_min = world_min.min(p);
                world_max = world_max.max(p);
            }
            let scene_obj = stage_desc
                .as_ref()
                .and_then(|d| d.scene_objects.iter().find(|obj| obj.name == object.name));
            let (mesh, transform) = build_scene_mesh(object, &indices, loaded.center, scene_obj);
            let deform = scene_obj.and_then(|obj| SceneDeform::from_mesh(object, &mesh, obj));
            let entity = commands
                .spawn((
                    PlayingSceneEntity,
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material_handle.clone()),
                    transform,
                ))
                .id();
            if let Some(frames) = ifl_frames {
                commands.entity(entity).insert(IflAnim {
                    frames,
                    material: material_handle.clone(),
                    cursor: 0,
                    timer: 0.0,
                });
            }

            if let Some(obj) = scene_obj {
                if obj.billboard == 1 {
                    commands.entity(entity).insert(SceneBillboard);
                }
                if let Some(deform) = deform {
                    commands.entity(entity).insert(deform);
                }
            }

            if let Some(anim) = scene_obj
                .filter(|o| o.texture_animation.enabled)
                .map(|o| &o.texture_animation)
            {
                commands.entity(entity).insert(UvScroll {
                    material: material_handle.clone(),
                    u: anim.u,
                    v: anim.v,
                });
            }
        }
    }

    info!(
        "spawned {} stage mesh batches, {} faces (textured {} faces, ifl materials {}, ifl frames {})",
        drawn_objects, drawn_faces, textured_faces, ifl_materials, ifl_frames_total
    );

    if let Some(mesh) = stage_collider.mesh.as_mut() {
        spawn_stage_props(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut pak_cache,
            &pak_root,
            &roots,
            &loaded,
            des_text.as_deref(),
            settings.texture_quality,
            &mut stage_texture_handles,
            mesh,
            script.script(),
            escort.0.as_deref(),
        );
    } else {
        spawn_stage_props(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut pak_cache,
            &pak_root,
            &roots,
            &loaded,
            des_text.as_deref(),
            settings.texture_quality,
            &mut stage_texture_handles,
            &mut StageMeshCollider::default(),
            script.script(),
            escort.0.as_deref(),
        );
    }
    if drawn_objects > 0 {
        info!(
            "stage world AABB min={:?} max={:?} size={:?}",
            world_min,
            world_max,
            world_max - world_min
        );
    } else {
        warn!("stage mesh contains no drawable objects");
    }
    commands.insert_resource(StageTextureHandles(stage_texture_handles));
}

fn build_scene_mesh(
    object: &meteor_asset::gmdl::GmdlObject,
    indices: &[u32],
    center: [f32; 3],
    scene_obj: Option<&meteor_asset::des::StageObject>,
) -> (Mesh, Transform) {
    let axes = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let mut transform = Transform::from_rotation(axes).with_scale(Vec3::splat(MODEL_SCALE));
    let Some(obj) = scene_obj.filter(|obj| obj.billboard == 1) else {
        return (build_mesh(object, indices, center), transform);
    };
    let pivot = Vec3::new(obj.position.x, obj.position.y, obj.position.z);
    let rotation = Quat::from_xyzw(
        obj.quaternion.x,
        obj.quaternion.y,
        obj.quaternion.z,
        obj.quaternion.w,
    )
    .normalize();
    let inverse = rotation.inverse();
    let mut mesh = build_mesh(object, indices, pivot.to_array());
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        object
            .positions
            .chunks_exact(3)
            .map(|v| (inverse * (Vec3::new(v[0], v[1], v[2]) - pivot)).to_array())
            .collect::<Vec<_>>(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        object
            .normals
            .chunks_exact(3)
            .map(|v| (inverse * Vec3::new(v[0], v[1], v[2])).to_array())
            .collect::<Vec<_>>(),
    );
    transform.translation = axes * (pivot - Vec3::from_array(center)) * MODEL_SCALE;
    transform.rotation = axes * rotation;
    (mesh, transform)
}

fn build_mesh(object: &meteor_asset::gmdl::GmdlObject, indices: &[u32], center: [f32; 3]) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        object
            .positions
            .chunks_exact(3)
            .map(|v| [v[0] - center[0], v[1] - center[1], v[2] - center[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        object
            .normals
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        object
            .uvs
            .chunks_exact(2)
            .map(|v| [v[0], v[1]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_COLOR,
        object
            .colors
            .chunks_exact(4)
            .map(gmdl_vertex_color)
            .collect::<Vec<_>>(),
    )
    .with_inserted_indices(Indices::U32(indices.to_vec()))
}

fn gmdl_vertex_color(bytes: &[u8]) -> [f32; 4] {
    [
        f32::from(bytes[0]) / 255.0,
        f32::from(bytes[1]) / 255.0,
        f32::from(bytes[2]) / 255.0,
        f32::from(bytes[3]) / 255.0,
    ]
}

#[derive(Clone, Default)]
struct LoadedStageTexture {
    handle: Option<Handle<bevy::image::Image>>,

    alpha_test: bool,
}

fn sibling_texture_name(name: &str, sibling: &str) -> String {
    let normalized = name.replace('\\', "/");
    normalized.rsplit_once('/').map_or_else(
        || sibling.to_string(),
        |(scope, _)| format!("{scope}/{sibling}"),
    )
}

#[allow(clippy::too_many_arguments)]
fn load_or_cache_texture(
    images: &mut Assets<bevy::image::Image>,
    cache: &mut HashMap<String, LoadedStageTexture>,
    pak_cache: &mut PakCache,
    root: &std::path::Path,
    scope: &str,
    name: &str,
    color_key: bool,
    alpha_channel: bool,
    texture_quality: u8,
) -> LoadedStageTexture {
    let key = format!(
        "{}|ck={}|alpha={}|quality={}",
        name.to_ascii_lowercase(),
        u8::from(color_key),
        u8::from(alpha_channel),
        texture_quality
    );
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }
    let loaded = match pak_cache.resolve(root, scope, name) {
        Some(bytes) => {
            let companion_name = legacy_alpha_companion_name(name)
                .map(|companion| sibling_texture_name(name, &companion));
            let named_companion = companion_name
                .as_deref()
                .and_then(|companion| pak_cache.resolve_exact(root, scope, companion));
            if named_companion.is_some() {
                if let Some(companion_name) = companion_name.as_deref() {
                    info!("stage texture companion alpha: {name} <- {companion_name}");
                }
            }
            let mask_companion = pak_cache.resolve_mask(root, scope, name);
            if let Some((mask_name, _)) = &mask_companion {
                info!("stage texture 8-bit mask: {name} <- {mask_name}");
            }
            let companion = named_companion
                .as_deref()
                .or_else(|| mask_companion.as_ref().map(|(_, bytes)| bytes.as_slice()));
            let luminance_fallback = alpha_channel
                && companion.is_none()
                && !name.to_ascii_lowercase().ends_with(".tga");
            let handle = if alpha_channel || color_key || companion.is_some() {
                image_handle_with_legacy_style_quality(
                    images,
                    &bytes,
                    companion,
                    luminance_fallback,
                    color_key,
                    texture_quality,
                )
            } else {
                image_handle_with_quality(images, &bytes, texture_quality)
            };
            let handle = handle.or_else(|| {
                warn!("stage texture decode failed: {name}");
                None
            });
            LoadedStageTexture {
                handle,
                alpha_test: mask_companion.is_some(),
            }
        }
        None => {
            info!("stage texture missing: {name} (untextured fallback)");
            LoadedStageTexture::default()
        }
    };
    cache.insert(key, loaded.clone());
    loaded
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::pak_textures::{
        image_handle_with_legacy_alpha, resolve_pak_texture, stage_pak_chain,
    };
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn uv_scroll_uses_simulation_time_without_per_frame_material_changes() {
        let mut app = App::new();
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        app.insert_resource(time)
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<super::super::stage_material::FrameUniforms>()
            .add_systems(Update, update_uv_scroll);
        let handle =
            app.world_mut()
                .resource_mut::<Assets<GmdlMaterial>>()
                .add(GmdlMaterial::new_scene(
                    GmdlMaterialParams {
                        opacity: 1.0,
                        alpha_cutoff: 0.0,
                        mode: 0,
                        has_texture: 0,
                        uv_offset: Vec2::new(0.2, 0.7),
                    },
                    None,
                    false,
                    GmdlMode::Opaque,
                ));
        let entity = app
            .world_mut()
            .spawn(UvScroll {
                material: handle.clone(),
                u: -0.3,
                v: 0.8,
            })
            .id();
        app.update();
        let initial = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .get(&handle)
            .unwrap()
            .clone();
        for _ in 0..19 {
            app.update();
        }
        let frame = app
            .world()
            .resource::<super::super::stage_material::FrameUniforms>();
        let mat = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .get(&handle)
            .unwrap();
        assert_eq!(mat.params.uv_offset, initial.params.uv_offset);
        assert_eq!(mat.uv_scroll, initial.uv_scroll);
        let phase = uv_scroll_offset(
            mat.params.uv_offset,
            mat.uv_scroll,
            super::super::stage_material::split_time(frame.seconds),
        );
        assert!(phase.distance(Vec2::new(0.9, 0.5)) < 1e-5);

        let clock = super::super::FrameClock {
            sim_dt: 0.0,
            ..default()
        };
        let before = frame.seconds;
        app.insert_resource(clock);
        app.update();
        assert_eq!(
            app.world()
                .resource::<super::super::stage_material::FrameUniforms>()
                .seconds,
            before
        );
        app.world_mut().entity_mut(entity).remove::<UvScroll>();
        app.update();
        let mat = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .get(&handle)
            .unwrap();
        assert_eq!(mat.uv_scroll.xy(), Vec2::ZERO);
        assert!(
            mat.params.uv_offset.distance(phase) < 1e-5,
            "removing scroll must freeze current phase"
        );
    }

    #[test]
    fn split_uv_clock_is_continuous_across_chunks_and_negative_speeds() {
        let start = super::super::stage_material::split_time(4095.5);
        let scroll = Vec4::new(-0.3, 0.8, start.x, start.y);
        for elapsed in [0.0, 0.49, 0.5, 0.51, 1.0, 4096.0] {
            let now = super::super::stage_material::split_time(4095.5 + elapsed);
            let actual = uv_scroll_offset(Vec2::ZERO, scroll, now);
            let expected = Vec2::new(
                (-0.3_f32 * elapsed as f32).rem_euclid(1.0),
                (0.8_f32 * elapsed as f32).rem_euclid(1.0),
            );
            let distance = (actual - expected).abs();
            let wrapped = distance.min(Vec2::ONE - distance);
            assert!(
                wrapped.max_element() < 0.001,
                "elapsed={elapsed}: {actual:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn hazard_sn08_roller_collision_and_hidden_damage() {
        use super::super::hazard_plugin::{sync_hazard_machines, StageHazardRuntime};
        use super::super::stage_plugin::StageScriptRuntime;
        use meteor_physics::collision::MeshCollider;
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let read = |p: &str| crate::text_codec::decode_text(&std::fs::read(root.join(p)).unwrap());
        let des = read("sn08/sn08.des");
        let desc = meteor_asset::des::parse_stage_description(&des);
        let center =
            meteor_scene::stage_loader::get_stage_center("sn08", Some(&read("Level/sn08.wp")));
        let sources = [
            "PScriptLib/Def.h",
            "PScriptLib/box.pst",
            "PScriptLib/desk.pst",
            "PScriptLib/chair.pst",
            "PScriptLib/jug.pst",
            "sn08/sn08.pst",
        ]
        .map(read)
        .join("\n");
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, d)| SceneItemInstance {
                name: d.name.clone(),
                index,
                props: [
                    ("active".into(), PstValue::Num(d.visible as i32 as f64)),
                    ("damagevalue".into(), PstValue::Num(d.damage_name as f64)),
                    (
                        "collision".into(),
                        PstValue::Num(is_solid_prop(d) as i32 as f64),
                    ),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("frame".into(), PstValue::Num(-1.0)),
                    ("state".into(), PstValue::Num(3.0)),
                ]
                .into(),
            })
            .collect();
        let mut script = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
        script.start().unwrap();
        let mesh = decode_gmdl(&std::fs::read(root.join("sn08/sn08.gmb")).unwrap()).unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn08/sn08.cob")).unwrap())
            .unwrap();
        let collider = StageMeshCollider::from_gmdl(&mesh, &desc, Some(&cob.face_ids), center);
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<GmdlMaterial>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Time>();
        world.init_resource::<StageHazardRuntime>();
        world.insert_resource(StageCollider {
            mesh: Some(collider),
        });
        world.insert_resource(StageScriptRuntime::from_script(script));
        world
            .run_system_once(
                move |mut commands: Commands,
                      mut meshes: ResMut<Assets<Mesh>>,
                      mut materials: ResMut<Assets<GmdlMaterial>>,
                      mut images: ResMut<Assets<Image>>,
                      mut collider: ResMut<StageCollider>,
                      script: Res<StageScriptRuntime>| {
                    spawn_stage_props(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                        &mut PakCache::default(),
                        &root,
                        &[root.clone()],
                        &crate::app_state::LoadedStageInfo {
                            stage_id: "sn08".into(),
                            center,
                            ..default()
                        },
                        Some(&des),
                        0,
                        &mut Vec::new(),
                        collider.mesh.as_mut().unwrap(),
                        script.script(),
                        None,
                    );
                },
            )
            .unwrap();
        world.run_system_once(sync_fmc_with_script).unwrap();
        world.run_system_once(update_fmc_clips).unwrap();
        world.run_system_once(sync_hazard_machines).unwrap();

        let mut checked = 0;
        for _ in 0..3 {
            world
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(0.5));
            world.run_system_once(update_fmc_clips).unwrap();
            let mut query = world.query::<(
                &crate::plugins::item_plugin::StageProp,
                &Transform,
                &super::super::hazard_plugin::HazardPart,
            )>();
            let collider = world.resource::<StageCollider>().mesh.as_ref().unwrap();
            let mut mover = meteor_physics::collision::CollisionSystem::new(0.96, 3.84);
            mover.set_mesh_collider(collider);
            for (prop, transform, part) in
                query.iter(&world).filter(|(p, _, _)| p.name == "D_sn08B01")
            {
                assert!(prop.collision_enabled);
                let points: Vec<_> = part
                    .0
                    .iter()
                    .map(|p| transform.transform_point(*p))
                    .collect();
                let mut contacts = 0;
                for p in points.iter().step_by(20) {
                    if collider.box_blocked(p.x, p.y - 2.0, p.z) {
                        contacts += 1;
                    }
                }
                assert!(contacts > 20, "roller surface contacts={contacts}");
                let min = points
                    .iter()
                    .fold(Vec3::splat(f32::INFINITY), |a, p| a.min(*p));
                let max = points
                    .iter()
                    .fold(Vec3::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
                let middle = (min + max) * 0.5;
                for (axis, side) in [(0, -1.0), (0, 1.0), (2, -1.0), (2, 1.0)] {
                    let mut start = middle;
                    start.y -= 2.0;
                    start[axis] = if side < 0.0 {
                        min[axis] - 5.0
                    } else {
                        max[axis] + 5.0
                    };
                    let pos = meteor_physics::collision::Point3::new(start.x, start.y, start.z);
                    let (dx, dz, _) =
                        mover.resolve_xz(pos, middle.x - start.x, middle.z - start.z, true);
                    let reached = Vec3::new(start.x + dx, start.y, start.z + dz);
                    eprintln!("roller side axis={axis} side={side} bounds={min:?}..{max:?} start={start:?} reached={reached:?} target={middle:?}");
                    assert!(
                        (reached[axis] - middle[axis]).abs() > 1.0,
                        "side jump entered roller"
                    );
                }
                let top = points.iter().max_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
                let pos = meteor_physics::collision::Point3::new(top.x, top.y + 8.0, top.z);
                let (dy, _, _) = mover.resolve_descent(pos, -16.0);
                assert!(
                    pos.y + dy >= top.y - 0.25,
                    "fall passed through roller: {top:?}, dy={dy}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 3);
        world.run_system_once(sync_hazard_machines).unwrap();
        let mut hazards = world.resource::<StageHazardRuntime>().0.clone();
        let machine = hazards
            .machines
            .iter()
            .find(|m| m.name == "D_sn08B02")
            .expect("hidden roller damage proxy remains enabled");
        assert!(!machine.boxes.is_empty());
        let b = machine.boxes[0];
        let point = std::array::from_fn(|i| (b.min[i] + b.max[i]) * 0.5);
        let mut hits = Vec::new();
        hazards.update(
            0.016,
            &[("player".into(), point, false, false)],
            &mut |_, source, damage, _| hits.push((source.to_owned(), damage)),
        );
        assert!(hits.contains(&("D_sn08B02".into(), 300)), "{hits:?}");
        let mut script = world.resource_mut::<StageScriptRuntime>();
        let index = script
            .script()
            .unwrap()
            .host()
            .instances()
            .iter()
            .position(|i| i.name == "D_sn08B02")
            .unwrap();
        script
            .script_mut()
            .unwrap()
            .host_mut()
            .instance_mut(index)
            .unwrap()
            .set("active", PstValue::Num(0.0));
        world.run_system_once(sync_hazard_machines).unwrap();
        assert!(world
            .resource::<StageHazardRuntime>()
            .0
            .machines
            .iter()
            .all(|m| m.name != "D_sn08B02"));
    }

    #[test]
    fn hazard_sn03_spawn_platforms_are_safe_after_landing() {
        use super::super::hazard_plugin::{sync_hazard_machines, StageHazardRuntime};
        use super::super::stage_plugin::StageScriptRuntime;
        use meteor_physics::collision::MeshCollider;
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let read = |p: &str| crate::text_codec::decode_text(&std::fs::read(root.join(p)).unwrap());
        let des = read("sn03/sn03.des");
        let desc = meteor_asset::des::parse_stage_description(&des);
        let center =
            meteor_scene::stage_loader::get_stage_center("sn03", Some(&read("Level/sn03.wp")));
        let sources = [
            "PScriptLib/Def.h",
            "PScriptLib/box.pst",
            "PScriptLib/desk.pst",
            "PScriptLib/chair.pst",
            "PScriptLib/jug.pst",
            "sn03/sn03.pst",
        ]
        .map(read)
        .join("\n");
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, d)| SceneItemInstance {
                name: d.name.clone(),
                index,
                props: [
                    ("active".into(), PstValue::Num(d.visible as i32 as f64)),
                    ("damagevalue".into(), PstValue::Num(d.damage_name as f64)),
                    (
                        "collision".into(),
                        PstValue::Num(is_solid_prop(d) as i32 as f64),
                    ),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("frame".into(), PstValue::Num(-1.0)),
                    ("state".into(), PstValue::Num(3.0)),
                ]
                .into(),
            })
            .collect();
        let mut script = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
        script.start().unwrap();
        let mesh = decode_gmdl(&std::fs::read(root.join("sn03/sn03.gmb")).unwrap()).unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn03/sn03.cob")).unwrap())
            .unwrap();
        let collider = StageMeshCollider::from_gmdl(&mesh, &desc, Some(&cob.face_ids), center);
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<GmdlMaterial>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Time>();
        world.init_resource::<StageHazardRuntime>();
        world.insert_resource(StageCollider {
            mesh: Some(collider),
        });
        world.insert_resource(StageScriptRuntime::from_script(script));
        world
            .run_system_once(
                move |mut commands: Commands,
                      mut meshes: ResMut<Assets<Mesh>>,
                      mut materials: ResMut<Assets<GmdlMaterial>>,
                      mut images: ResMut<Assets<Image>>,
                      mut collider: ResMut<StageCollider>,
                      script: Res<StageScriptRuntime>| {
                    spawn_stage_props(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                        &mut PakCache::default(),
                        &root,
                        &[root.clone()],
                        &crate::app_state::LoadedStageInfo {
                            stage_id: "sn03".into(),
                            center,
                            ..default()
                        },
                        Some(&des),
                        0,
                        &mut Vec::new(),
                        collider.mesh.as_mut().unwrap(),
                        script.script(),
                        None,
                    );
                },
            )
            .unwrap();
        world.run_system_once(sync_fmc_with_script).unwrap();
        world.run_system_once(update_fmc_clips).unwrap();
        world.run_system_once(sync_hazard_machines).unwrap();
        let mut hazards = world.resource::<StageHazardRuntime>().0.clone();
        hazards.volumes = super::super::hazard_plugin::build_volumes(&mesh, &desc, center);
        let collider = world.resource::<StageCollider>().mesh.as_ref().unwrap();
        let mut checked = 0;
        for spawn in desc.dummies.iter().filter(|d| d.name.starts_with("D_User")) {
            let mut p = prop_transform(spawn, center).translation;
            let ground = collider
                .sample_ground_hit(p.x, p.z, -200.0, p.y)
                .expect("spawn platform");
            p.y = ground.y;
            let mut hits = Vec::new();
            hazards.update(
                3.0,
                &[(spawn.name.clone(), p.to_array(), false, false)],
                &mut |_, source, damage, _| hits.push((source.to_owned(), damage)),
            );
            eprintln!("{} landed {p:?}: {hits:?}", spawn.name);
            assert!(hits.is_empty(), "{} landed {p:?}: {hits:?}", spawn.name);
            checked += 1;
        }
        assert_eq!(checked, 16);

        for name in ["D_sn03t09", "D_sn03t10"] {
            let machine = hazards.machines.iter().find(|m| m.name == name).unwrap();
            let box_ = machine.boxes[0];
            let point = std::array::from_fn(|i| (box_.min[i] + box_.max[i]) * 0.5);
            let amount = machine.damage_value;
            let mut hits = Vec::new();
            hazards.update(
                3.0,
                &[("contact".into(), point, false, false)],
                &mut |_, source, damage, _| hits.push((source.to_owned(), damage)),
            );
            assert!(
                hits.contains(&(name.to_owned(), amount)),
                "{name}: {hits:?}"
            );
        }
    }

    #[test]
    fn sn03_buttons_and_doors_follow_authored_fmc_and_restore_idle() {
        use crate::plugins::stage_plugin::StageScriptRuntime;
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let read =
            |path: &str| crate::text_codec::decode_text(&std::fs::read(root.join(path)).unwrap());
        let desc = meteor_asset::des::parse_stage_description(&read("sn03/sn03.des"));
        let sources = [
            "PScriptLib/Def.h",
            "PScriptLib/box.pst",
            "PScriptLib/desk.pst",
            "PScriptLib/chair.pst",
            "PScriptLib/jug.pst",
            "sn03/sn03.pst",
        ]
        .map(read)
        .join("\n");
        for button in ["D_Abutton01", "D_Abutton02", "D_Bbutton01", "D_Bbutton02"] {
            let door = if button.starts_with("D_A") {
                "D_ADoor01"
            } else {
                "D_BDoor01"
            };
            let instances = desc
                .dummies
                .iter()
                .enumerate()
                .map(|(index, dummy)| SceneItemInstance {
                    name: dummy.name.clone(),
                    index,
                    props: [
                        ("frame".into(), PstValue::Num(-1.0)),
                        ("state".into(), PstValue::Num(3.0)),
                    ]
                    .into(),
                })
                .collect();
            let mut script = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
            script.start().unwrap();
            let mut world = World::new();
            world.insert_resource(Time::<()>::default());
            world.insert_resource(StageScriptRuntime::from_script(script));
            let mut collider = StageCollider {
                mesh: Some(StageMeshCollider::default()),
            };
            let mut parts = Vec::new();
            for name in [button, door] {
                let dummy = desc.dummies.iter().find(|d| d.name == name).unwrap();
                let model_name = dummy.model.as_ref().unwrap();
                let model = decode_gmdl(
                    &std::fs::read(root.join(format!("cmodel/{model_name}.gmb"))).unwrap(),
                )
                .unwrap();
                let clip = meteor_asset::fmc::parse_fmc(&read(&format!("cmodel/{model_name}.fmc")));
                let poses =
                    meteor_asset::fmc::parse_prop_poses(&read(&format!("cmodel/{model_name}.pos")));
                let base = prop_transform(dummy, meteor_scene::stage_loader::ARENA_CENTER);
                let (vertices, indices, geometry) = prop_collision_geometry(&model, dummy, base);
                let mesh = collider.mesh.as_mut().unwrap();
                let owner = mesh.append_world_triangles(&vertices, &indices);
                let vertex_start = mesh.positions.len() - vertices.len();
                assert_eq!(clip.scene_object_count, 2);
                for (part_index, geometry) in geometry.into_iter().enumerate() {
                    let (offset, local_verts) = geometry.unwrap();
                    let entity = world
                        .spawn((
                            base,
                            FmcClip {
                                name: name.into(),
                                clip: clip.clone(),
                                part_index,
                                poses: poses.clone(),
                                pose_index: 0,
                                pose_start: 0,
                                pose_end: 0,
                                pose_loop: 0,
                                pose_revision: 0,
                                anim_time: 0.0,
                                state: 3,
                                frame_set: None,
                                multi: true,
                                base_translation: base.translation,
                                base_rotation: base.rotation,
                                collider_owner: Some(owner),
                                collider_vertex_offset: offset,
                                local_verts,
                                last_reported_state: 1,
                            },
                        ))
                        .id();
                    parts.push((entity, base, vertex_start + offset));
                }
            }
            world.insert_resource(collider);
            world.run_system_once(sync_fmc_with_script).unwrap();
            world
                .resource_mut::<StageScriptRuntime>()
                .script_mut()
                .unwrap()
                .call_item(
                    button,
                    "OnAttack",
                    &[PstValue::Num(0.0), PstValue::Num(1.0)],
                )
                .unwrap();
            world.run_system_once(sync_fmc_with_script).unwrap();
            for (entity, _, _) in &parts {
                assert_eq!(
                    world.get::<FmcClip>(*entity).unwrap().pose_index,
                    1,
                    "{button} must animate both switch and gate"
                );
            }

            for frame in [15, 48, 70, 0] {
                if frame == 0 {
                    let script = world
                        .resource_mut::<StageScriptRuntime>()
                        .into_inner()
                        .script_mut()
                        .unwrap();
                    for name in [button, door] {
                        let instance = script
                            .host_mut()
                            .instances_mut()
                            .iter_mut()
                            .find(|i| i.name == name)
                            .unwrap();
                        instance.set("pose", PstValue::Num(0.0));
                    }
                    world.run_system_once(sync_fmc_with_script).unwrap();
                }
                for (entity, _, _) in &parts {
                    world.get_mut::<FmcClip>(*entity).unwrap().frame_set = Some(frame);
                }
                world.run_system_once(update_fmc_clips).unwrap();
                for (entity, base, vertex_start) in &parts {
                    let clip = world.get::<FmcClip>(*entity).unwrap();
                    let sample =
                        &clip.clip.frames[frame.min(clip.pose_end) as usize].parts[clip.part_index];
                    let rest = &clip.clip.frames[0].parts[clip.part_index];
                    let quat = |q: meteor_core::Quat| Quat::from_xyzw(q.x, q.y, q.z, q.w);
                    let delta = quat(sample.q) * quat(rest.q).inverse();
                    let pivot = Vec3::new(rest.t.x, rest.t.y, rest.t.z);
                    let target = Vec3::new(sample.t.x, sample.t.y, sample.t.z);
                    let actual = world.get::<Transform>(*entity).unwrap();
                    let mesh = world.resource::<StageCollider>().mesh.as_ref().unwrap();
                    for (i, vertex) in clip.local_verts.iter().enumerate() {
                        let expected = base
                            .transform_point(delta * (Vec3::from_array(*vertex) - pivot) + target);
                        assert!(
                            actual
                                .transform_point(Vec3::from_array(*vertex))
                                .distance(expected)
                                < 0.002,
                            "{} part {} frame {frame}: wrong rotation/pivot",
                            clip.name,
                            clip.part_index
                        );
                        assert!(
                            Vec3::from_array(mesh.positions[vertex_start + i]).distance(expected)
                                < 0.002
                        );
                    }
                    if clip.name == button && clip.part_index == 0 && frame == 15 {
                        assert!(
                            actual.rotation.angle_between(base.rotation) > 0.1,
                            "switch handle must move"
                        );
                    }
                    assert_eq!(
                        clip.state,
                        if frame > 0 && frame < clip.pose_end {
                            1
                        } else {
                            3
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn sn09_floor_fmc_falls_vertically_with_collision() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn09/sn09.des")).unwrap(),
        );
        let model = decode_gmdl(&std::fs::read(root.join("cmodel/sn09f01.gmb")).unwrap()).unwrap();
        let fmc = meteor_asset::fmc::parse_fmc(
            &std::fs::read_to_string(root.join("cmodel/sn09f01.fmc")).unwrap(),
        );
        let poses = meteor_asset::fmc::parse_prop_poses(
            &std::fs::read_to_string(root.join("cmodel/sn09f01.pos")).unwrap(),
        );
        assert_eq!(fmc.scene_object_count, 1);
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        let mut collider = StageCollider {
            mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
        };
        let mut tiles = Vec::new();
        for dummy in desc
            .dummies
            .iter()
            .filter(|d| d.name.starts_with("D_sn09f"))
        {
            let base = prop_transform(dummy, meteor_scene::stage_loader::ARENA_CENTER);
            let (vertices, indices, parts) = prop_collision_geometry(&model, dummy, base);
            let owner = collider
                .mesh
                .as_mut()
                .unwrap()
                .append_world_triangles(&vertices, &indices);
            let vertex_start = collider.mesh.as_ref().unwrap().positions.len() - vertices.len();
            let (_, local_verts) = parts[0].clone().unwrap();
            let entity = world
                .spawn((
                    base,
                    FmcClip {
                        name: dummy.name.clone(),
                        clip: fmc.clone(),
                        part_index: 0,
                        poses: poses.clone(),
                        pose_index: 1,
                        pose_start: poses[1].start,
                        pose_end: poses[1].end,
                        pose_loop: 0,
                        pose_revision: 1,
                        anim_time: 0.0,
                        state: 1,
                        frame_set: None,
                        multi: false,
                        base_translation: base.translation,
                        base_rotation: base.rotation,
                        collider_owner: Some(owner),
                        collider_vertex_offset: 0,
                        local_verts: local_verts.clone(),
                        last_reported_state: 1,
                    },
                ))
                .id();
            tiles.push((entity, base, vertex_start, local_verts));
        }
        assert_eq!(tiles.len(), 100);
        world.insert_resource(collider);

        for frame in [0, 45, 90, 120] {
            for (entity, _, _, _) in &tiles {
                world.get_mut::<FmcClip>(*entity).unwrap().frame_set = Some(frame);
            }
            world.run_system_once(update_fmc_clips).unwrap();
            let source = &fmc.frames[frame.min(90) as usize].parts[0];
            let drop = source.t.z * MODEL_SCALE;
            if frame > 0 {
                assert!(drop < 0.0);
            }
            for (entity, base, vertex_start, local_verts) in &tiles {
                let transform = world.get::<Transform>(*entity).unwrap();
                let expected_position = base.translation + Vec3::Y * drop;
                assert!(transform.translation.distance(expected_position) < 0.0001);
                let expected_rotation = base.rotation
                    * Quat::from_xyzw(source.q.x, source.q.y, source.q.z, source.q.w)
                    * Quat::from_xyzw(0.0, 0.0, 0.0, -1.0).inverse();
                let mesh = world.resource::<StageCollider>().mesh.as_ref().unwrap();
                for (i, local) in local_verts.iter().enumerate() {
                    let expected = expected_position
                        + expected_rotation * (Vec3::from_array(*local) * base.scale);
                    assert!(
                        Vec3::from_array(mesh.positions[vertex_start + i]).distance(expected)
                            < 0.001
                    );
                }
                assert_eq!(
                    world.get::<FmcClip>(*entity).unwrap().state,
                    if frame < 90 { 1 } else { 3 }
                );
            }
        }
    }

    #[test]
    fn death_drop_real_resource_is_textured_and_pickable_immediately_after_landing() {
        use crate::components::fighter::{Fighter, Health, Qi};
        use crate::plugins::item_plugin::{item_pickup, ItemBuffs, StageProp};
        use crate::plugins::stage_plugin::{LocalFighter, StageScriptRuntime};

        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<crate::plugins::audio_plugin::SfxCache>()
            .init_resource::<ItemBuffs>()
            .init_resource::<StageScriptRuntime>()
            .init_resource::<StageCollider>()
            .init_resource::<crate::plugins::network_plugin::NetState>()
            .init_resource::<OfflineWeaponDropSerial>()
            .insert_resource(crate::app_state::LoadedStageInfo {
                stage_id: "sn01".into(),
                ..default()
            })
            .add_message::<OfflineWeaponDeathDrop>()
            .add_systems(
                Update,
                (
                    spawn_offline_weapon_death_drops,
                    advance_network_weapon_drop_flight,
                    advance_dynamic_weapon_drop_pickup_delay,
                )
                    .chain(),
            );
        let mut floor = StageMeshCollider::default();
        floor.append_world_triangles(
            &[
                [-20.0, 0.0, -20.0],
                [20.0, 0.0, -20.0],
                [20.0, 0.0, 20.0],
                [-20.0, 0.0, 20.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        app.world_mut().resource_mut::<StageCollider>().mesh = Some(floor);
        app.world_mut().write_message(OfflineWeaponDeathDrop {
            model: "W0_0".into(),
            position: [0.0, NETWORK_WEAPON_DROP_HEIGHT, 0.0],
            velocity: [0.0, 0.0, 12.0],
        });
        app.update();
        let drop = app
            .world_mut()
            .query_filtered::<Entity, With<DynamicWeaponDrop>>()
            .single(app.world())
            .unwrap();
        let material_handles = app
            .world_mut()
            .query::<&MeshMaterial3d<GmdlMaterial>>()
            .iter(app.world())
            .map(|material| material.0.clone())
            .collect::<Vec<_>>();
        assert!(
            !material_handles.is_empty(),
            "death drop must render the real GMC weapon"
        );
        for handle in material_handles {
            let material = app
                .world()
                .resource::<Assets<GmdlMaterial>>()
                .get(&handle)
                .unwrap();
            assert!(
                material.texture.is_some(),
                "death drop must resolve its PAK texture from a crate cwd"
            );
            assert_eq!(material.params.has_texture, 1);
        }
        for _ in 0..16 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(20));
            app.update();
        }
        assert!(app.world().get::<DynamicWeaponDropFlight>(drop).is_some());
        assert!(!app.world().get::<StageProp>(drop).unwrap().interactive);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(20));
        app.update();
        assert!(app.world().get::<DynamicWeaponDropFlight>(drop).is_none());
        assert!(app.world().get::<StageProp>(drop).unwrap().interactive);
        let position = app
            .world()
            .get::<StageProp>(drop)
            .unwrap()
            .world_center(app.world().get::<Transform>(drop).unwrap());
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_translation(Vec3::new(position.x, 0.0, position.z)),
                Health::default(),
                Qi::default(),
            ))
            .id();
        for slots in [[5, 1], [5, 6]] {
            app.world_mut()
                .get_mut::<Fighter>(player)
                .unwrap()
                .weapon_slots = slots;
            app.world_mut().run_system_once(item_pickup).unwrap();
            assert!(
                app.world().get::<StageProp>(drop).unwrap().interactive,
                "full or duplicate loadouts must leave the drop available"
            );
        }
        app.world_mut()
            .get_mut::<Fighter>(player)
            .unwrap()
            .weapon_slots = [5, 0];
        app.world_mut().run_system_once(item_pickup).unwrap();
        assert_eq!(
            app.world().get::<Fighter>(player).unwrap().weapon_slots,
            [5, 6]
        );
        assert!(
            app.world().get_entity(drop).is_err(),
            "claimed dynamic drop must be removed permanently"
        );
        assert_eq!(
            app.world_mut()
                .query::<&MeshMaterial3d<GmdlMaterial>>()
                .iter(app.world())
                .count(),
            0,
            "claim must remove every render child too"
        );
    }

    fn sky_gmdl(names: &[&str]) -> GmdlMesh {
        GmdlMesh {
            textures: names
                .iter()
                .map(|name| meteor_asset::gmdl::GmdlTexture {
                    name: (*name).to_string(),
                })
                .collect(),
            materials: Vec::new(),
            objects: Vec::new(),
        }
    }

    #[test]
    fn original_skies_do_not_spawn_duplicate_domes() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<Image>>();
        app.world_mut()
            .run_system_once(
                |mut commands: Commands,
                 mut meshes: ResMut<Assets<Mesh>>,
                 mut materials: ResMut<Assets<StandardMaterial>>,
                 mut images: ResMut<Assets<Image>>| {
                    for stage in ["sn01", "sn05", "sn13", "sn14", "sn18", "sn22", "sn23"] {
                        spawn_stage_sky(
                            &mut commands,
                            &mut meshes,
                            &mut materials,
                            &mut images,
                            &sky_gmdl(&["authored_sky.jpg"]),
                            stage,
                        );
                    }
                },
            )
            .unwrap();
        assert_eq!(
            app.world_mut()
                .query::<&StageSkyDome>()
                .iter(app.world())
                .count(),
            0
        );
        app.world_mut()
            .run_system_once(
                |mut commands: Commands,
                 mut meshes: ResMut<Assets<Mesh>>,
                 mut materials: ResMut<Assets<StandardMaterial>>,
                 mut images: ResMut<Assets<Image>>| {
                    spawn_stage_sky(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                        &sky_gmdl(&[]),
                        "sn23",
                    );
                },
            )
            .unwrap();
        assert_eq!(
            app.world_mut()
                .query::<&StageSkyDome>()
                .iter(app.world())
                .count(),
            1
        );
    }

    #[test]
    fn origin_sn21_dummy_billboards_face_camera_and_animate_patch_textures() {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.04));
        app.insert_resource(time)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<Image>>()
            .add_plugins(super::super::ifl_plugin::IflPlugin)
            .add_systems(PostUpdate, update_prop_billboard);
        app.world_mut()
            .run_system_once(
                |mut commands: Commands,
                 mut meshes: ResMut<Assets<Mesh>>,
                 mut materials: ResMut<Assets<GmdlMaterial>>,
                 mut images: ResMut<Assets<Image>>| {
                    let roots = stage_asset_roots(&std::env::current_dir().unwrap());
                    let root = pak_files_root(&roots, "sn21");
                    spawn_stage_props(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                        &mut PakCache::default(),
                        &root,
                        &roots,
                        &crate::app_state::LoadedStageInfo {
                            stage_id: "sn21".into(),
                            ..default()
                        },
                        None,
                        0,
                        &mut Vec::new(),
                        &mut StageMeshCollider::default(),
                        None,
                        None,
                    );
                },
            )
            .unwrap();
        let before: Vec<_> = app
            .world_mut()
            .query_filtered::<(Entity, &Transform), With<PropBillboard>>()
            .iter(app.world())
            .map(|(e, t)| (e, *t))
            .collect();
        assert_eq!(before.len(), 136);
        let tracks: Vec<_> = app
            .world_mut()
            .query::<&IflAnim>()
            .iter(app.world())
            .map(|anim| {
                assert_eq!(anim.frames.len(), 22);
                (anim.material.clone(), anim.frames.clone())
            })
            .collect();
        assert_eq!(tracks.len(), 3, "one track per shared model material");
        let camera = app
            .world_mut()
            .spawn((
                crate::plugins::camera_plugin::GameplayCamera,
                Transform::default(),
            ))
            .id();
        for position in [Vec3::new(50.0, 70.0, 100.0), Vec3::new(-80.0, 40.0, -100.0)] {
            app.world_mut()
                .get_mut::<Transform>(camera)
                .unwrap()
                .translation = position;
            for _ in 0..4 {
                app.update();
            }
            for (entity, initial) in &before {
                let transform = app.world().get::<Transform>(*entity).unwrap();
                assert_eq!(transform.translation, initial.translation);
                assert_eq!(transform.scale, initial.scale);
                assert!(
                    (transform.rotation * -Vec3::Z)
                        .dot((position - transform.translation).normalize())
                        > 0.999
                );
            }
        }
        for (material, frames) in tracks {
            let material = app
                .world()
                .resource::<Assets<GmdlMaterial>>()
                .get(&material)
                .unwrap();
            assert_eq!(material.texture.as_ref(), Some(&frames[3]));
            assert_eq!(material.mode, GmdlMode::SrcColorOne);
            assert_eq!(material.params.has_texture, 1);
        }
    }

    #[test]
    fn authored_pickups_keep_textures_effects_and_respawn() {
        use crate::components::fighter::{Fighter, Health, Qi};
        use crate::plugins::item_plugin::{item_pickup, ItemBuffs, StageProp};
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<AudioSource>>()
            .init_resource::<crate::plugins::audio_plugin::SfxCache>()
            .init_resource::<ItemBuffs>()
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>();

        app.world_mut().run_system_once(|mut commands: Commands,
            mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<GmdlMaterial>>,
            mut images: ResMut<Assets<Image>>| {
            let roots = stage_asset_roots(&std::env::current_dir().unwrap());
            let pak_root = pak_files_root(&roots, "sn01");
            spawn_stage_props(&mut commands, &mut meshes, &mut materials, &mut images,
                &mut PakCache::default(), &pak_root, &roots,
                &crate::app_state::LoadedStageInfo { stage_id: "sn01".into(), ..default() },
                Some("SceneObjects 0 DummeyObjects 2\n\
                    Object D_itTest\n{\nPosition: 0 0 0\nQuaternion: 1 0 0 0\nCustom:\n{\nmodel=Item01\n}\n}\n\
                    Object D_wpTest\n{\nPosition: 1000 0 0\nQuaternion: 1 0 0 0\nCustom:\n{\nmodel=W0_0\n}\n}\n"),
                0, &mut Vec::new(), &mut StageMeshCollider::default(), None, None);
        }).unwrap();
        let player = app
            .world_mut()
            .spawn((
                crate::plugins::stage_plugin::LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::default(),
                Health {
                    current: 500.0,
                    max: 1000.0,
                },
                Qi::default(),
            ))
            .id();
        for name in ["D_itTest", "D_wpTest"] {
            let (entity, position, material) = app
                .world_mut()
                .query::<(Entity, &StageProp, &Transform, &Children)>()
                .iter(app.world())
                .find(|(_, prop, _, _)| prop.name == name)
                .map(|(entity, prop, transform, children)| {
                    let material = app
                        .world()
                        .get::<MeshMaterial3d<GmdlMaterial>>(children[0])
                        .unwrap();
                    assert!(app
                        .world()
                        .get::<gem_visual::GemVisual>(children[0])
                        .is_some());
                    (entity, prop.world_center(transform), material.0.clone())
                })
                .expect("authored pickup must have a render batch");
            let base = *app.world().get::<Transform>(entity).unwrap();
            let child = app.world().get::<Children>(entity).unwrap()[0];
            app.world_mut()
                .init_resource::<crate::plugins::stage_material::FrameUniforms>();
            app.world_mut()
                .resource_mut::<crate::plugins::stage_material::FrameUniforms>()
                .seconds = 0.0;
            app.world_mut()
                .run_system_once(gem_visual::update_gem_visuals)
                .unwrap();
            let first = *app.world().get::<Transform>(child).unwrap();
            app.world_mut()
                .resource_mut::<crate::plugins::stage_material::FrameUniforms>()
                .seconds = 0.25;
            app.world_mut()
                .run_system_once(gem_visual::update_gem_visuals)
                .unwrap();
            let next = app.world().get::<Transform>(child).unwrap();
            assert!(first.translation.distance(next.translation) > 0.01);
            assert!(first.rotation.angle_between(next.rotation) > 0.1);
            assert_eq!(*app.world().get::<Transform>(entity).unwrap(), base);
            assert!(
                app.world()
                    .resource::<Assets<GmdlMaterial>>()
                    .get(&material)
                    .unwrap()
                    .texture
                    .is_some(),
                "{name} must use its original texture"
            );
            app.world_mut()
                .get_mut::<Transform>(player)
                .unwrap()
                .translation = position;
            app.world_mut().run_system_once(item_pickup).unwrap();
            let prop = app.world().get::<StageProp>(entity).unwrap();
            assert!(!prop.interactive);
            assert!(prop.respawn_timer > 0.0);
            assert_eq!(
                *app.world().get::<Visibility>(entity).unwrap(),
                Visibility::Hidden
            );
            let clock = crate::plugins::FrameClock {
                sim_dt: prop.respawn_timer + 0.01,
                ..default()
            };
            app.world_mut().insert_resource(clock);
            app.world_mut().run_system_once(item_pickup).unwrap();
            app.world_mut()
                .remove_resource::<crate::plugins::FrameClock>();
            assert!(app.world().get::<StageProp>(entity).unwrap().interactive);
            assert_eq!(
                *app.world().get::<Visibility>(entity).unwrap(),
                Visibility::Visible
            );
        }
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 800.0);
        assert_eq!(
            app.world().get::<Fighter>(player).unwrap().weapon_slots,
            [5, 6]
        );
    }

    #[test]
    fn gmdl_vertex_alpha_is_not_forced_opaque() {
        let color = gmdl_vertex_color(&[120, 110, 90, 64]);
        assert!((color[0] - 120.0 / 255.0).abs() < 1e-6);
        assert!((color[3] - 64.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn sky_name_priority_matches_web_and_custom_never_guesses() {
        let gmdl = sky_gmdl(&["foo_sky1_.jpg", "plainSky.bmp", "primarySky1.jpg"]);
        assert_eq!(
            pick_stage_sky_name(&gmdl, "sn06").as_deref(),
            Some("primarySky1.jpg")
        );
        assert_eq!(
            pick_stage_sky_name(&sky_gmdl(&[]), "sn06").as_deref(),
            Some("sn06sky1.jpg")
        );
        assert_eq!(pick_stage_sky_name(&sky_gmdl(&[]), "sn23"), None);
    }

    #[test]
    fn custom_sky_is_deterministic_web_gradient() {
        let first = custom_stage_sky_rgba(64);
        let second = custom_stage_sky_rgba(64);
        assert_eq!(first, second);
        assert_eq!(first.len(), 64 * 64 * 4);
        assert_eq!(&first[..4], &[0x1a, 0x2a, 0x4a, 0xff]);
        assert_eq!(&first[first.len() - 4..], &[0x1d, 0x1d, 0x1d, 0xff]);
    }

    #[test]
    fn origin_official_sky_selection_resolves_decodable_pack_texture() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01.pak").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        for (stage, expected) in [
            ("sn01", "sn01sky.jpg"),
            ("sn03", "sn3sky.JPG"),
            ("sn04", "sn4sky.jpg"),
            ("sn05", "sn05sky1.jpg"),
            ("sn06", "sn06sky1.jpg"),
            ("sn08", "sn08sky.jpg"),
            ("sn13", "sky1.bmp"),
            ("sn18", "hev7sky1.jpg"),
            ("sn19", "SN19SKY.jpg"),
        ] {
            let bytes = std::fs::read(root.join(stage).join(format!("{stage}.gmb")))
                .unwrap_or_else(|_| panic!("missing {stage}.gmb"));
            let gmdl = decode_gmdl(&bytes).unwrap_or_else(|_| panic!("decode {stage}.gmb"));
            let name = pick_stage_sky_name(&gmdl, stage).expect("authored sky name");
            assert_eq!(name, expected, "{stage}");
            let paks = stage_pak_chain(&root, stage);
            let refs: Vec<&PakArchive> = paks.iter().collect();
            let texture = resolve_pak_texture(&refs, &name)
                .unwrap_or_else(|| panic!("resolve {stage}/{name}"));
            meteor_asset::texture::decode_image(&texture)
                .unwrap_or_else(|_| panic!("decode {stage}/{name}"));
        }
    }

    #[test]
    fn deform_ref_mask_defaults_to_xyz() {
        assert_eq!(compile_deform_ref_mask(None), 1 | 2 | 4);
        assert_eq!(compile_deform_ref_mask(Some("")), 1 | 2 | 4);
        assert_eq!(compile_deform_ref_mask(Some("xyz")), 1 | 2 | 4);
        assert_eq!(compile_deform_ref_mask(Some("xz")), 1 | 4);
        assert_eq!(compile_deform_ref_mask(Some("Y")), 2);

        assert_eq!(compile_deform_ref_mask(Some("w")), 1 | 2 | 4);
    }

    #[test]
    fn deform_ref_value_sums_masked_axes() {
        let pos = [1.0, 2.0, 3.0];

        assert!((deform_reference_value(pos, Some("xyz")) - 6.0).abs() < 1e-5);

        assert!((deform_reference_value(pos, Some("xz")) - 4.0).abs() < 1e-5);

        assert!((deform_reference_value(pos, Some("y")) - 2.0).abs() < 1e-5);

        assert!((deform_reference_value(pos, None) - 6.0).abs() < 1e-5);
    }

    #[test]
    fn origin_sn05_des_drives_real_leaf_meshes() {
        use bevy::mesh::VertexAttributeValues;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl = decode_gmdl(&std::fs::read(root.join("sn05/sn05.gmb")).unwrap()).unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn05/sn05.des")).unwrap(),
        );
        let mut app = App::new();
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        app.insert_resource(time)
            .init_resource::<Assets<Mesh>>()
            .add_systems(Update, update_scene_deform);
        let mut animated = Vec::new();
        for object in &gmdl.objects {
            let obj = desc
                .scene_objects
                .iter()
                .find(|o| o.name == object.name)
                .unwrap();
            let (mesh, transform) = build_scene_mesh(object, &object.indices, [0.0; 3], Some(obj));
            let Some(deform) = SceneDeform::from_mesh(object, &mesh, obj) else {
                continue;
            };
            assert!(!object.indices.is_empty(), "{}", object.name);
            assert_eq!((deform.size, deform.freq, deform.range), (0.5, 0.25, 0.5));
            let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
            let entity = app
                .world_mut()
                .spawn((Mesh3d(handle.clone()), transform, deform))
                .id();
            animated.push((entity, handle, transform, object.indices.clone()));
        }
        assert_eq!(animated.len(), 7);
        for _ in 0..20 {
            app.update();
        }
        let positions = |app: &App, handle: &Handle<Mesh>| {
            let meshes = app.world().resource::<Assets<Mesh>>();
            let Some(VertexAttributeValues::Float32x3(p)) = meshes
                .get(handle)
                .unwrap()
                .attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("positions");
            };
            p.clone()
        };
        let first: Vec<_> = animated
            .iter()
            .map(|(_, h, _, _)| positions(&app, h))
            .collect();
        for _ in 0..20 {
            app.update();
        }
        for ((entity, handle, transform, indices), first) in animated.iter().zip(first) {
            let second = positions(&app, handle);
            assert!(
                indices.iter().any(|&i| {
                    Vec3::from_array(first[i as usize])
                        .distance(Vec3::from_array(second[i as usize]))
                        > 0.01
                }),
                "authored drawn vertices must move over time"
            );
            assert_eq!(app.world().get::<Transform>(*entity).unwrap(), transform);
        }
    }

    #[test]
    fn scene_deform_changes_vertices_without_moving_object_or_accumulating() {
        use bevy::mesh::VertexAttributeValues;
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<Assets<Mesh>>();
        app.add_systems(Update, update_scene_deform);
        let base = vec![[0.0; 3], [1.0, 0.0, 0.0]];
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, base.clone());
        let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        let transform = Transform::from_xyz(4.0, 5.0, 6.0).with_scale(Vec3::splat(MODEL_SCALE));
        let entity = app
            .world_mut()
            .spawn((
                Mesh3d(handle.clone()),
                transform,
                SceneDeform {
                    freq: 0.25,
                    size: 0.5,
                    range: 0.5,
                    references: vec![0.0, std::f32::consts::PI.powi(2) / 2.0],
                    base_positions: base,
                    offset_axis: Vec3::ONE,
                    time: 0.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.05));
        for _ in 0..20 {
            app.update();
        }
        let positions = |app: &App| {
            let meshes = app.world().resource::<Assets<Mesh>>();
            let Some(VertexAttributeValues::Float32x3(p)) = meshes
                .get(&handle)
                .unwrap()
                .attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("positions");
            };
            p.clone()
        };
        let first = positions(&app);
        assert!((first[0][0] - 0.5).abs() < 1e-5);
        assert!((first[1][0] - 1.0).abs() < 1e-5);
        for _ in 0..20 {
            app.update();
        }
        let second = positions(&app);
        assert!(second[0][0].abs() < 1e-5);
        assert!((second[1][0] - 0.5).abs() < 1e-5);
        assert_eq!(*app.world().get::<Transform>(entity).unwrap(), transform);
    }

    #[test]
    fn deform_offset_is_sin_of_time_phase() {
        let freq = 1.0;
        let size = 10.0;
        let range = 5.0;
        let reference = 0.0;
        let t = 0.0;
        let off = (t * freq * std::f32::consts::TAU + reference / (size * std::f32::consts::TAU))
            .sin()
            * range;
        assert!(off.abs() < 1e-5);

        let t = 0.25;
        let off = (t * freq * std::f32::consts::TAU + reference / (size * std::f32::consts::TAU))
            .sin()
            * range;
        assert!((off - 5.0).abs() < 1e-4);
    }

    fn mat(blend: &str, state: &str, alpha: f32) -> RawGmdlMaterial {
        RawGmdlMaterial {
            tex_index: 0,
            blend: blend.into(),
            two_side: false,
            state: state.into(),
            alpha,
        }
    }

    fn classify(mat: &RawGmdlMaterial) -> (u32, f32, f32) {
        let (mode, opacity, cutoff) = classify_gmdl_material(mat);
        (mode.as_u32(), opacity, cutoff)
    }

    #[test]
    fn opaque_material() {
        let (mode, opacity, cutoff) = classify(&mat("NORMAL", "DISABLE 1 0", 1.0));
        assert_eq!(mode, 0);
        assert_eq!(opacity, 1.0);
        assert_eq!(cutoff, 0.0);
    }

    #[test]
    fn colorkey_material_uses_alpha_ref() {
        let (mode, _, cutoff) = classify(&mat("COLORKEY", "DISABLE 1 0", 1.0));
        assert_eq!(mode, 0);
        assert!((cutoff - 127.0 / 255.0).abs() < 1e-5);
    }

    #[test]
    fn blend_modes_map_to_tokens() {
        let (mode, opacity, _) = classify(&mat("ALPHACHANNEL", "ENABLE sa 1-sa", 0.5));
        assert_eq!(mode, 1);
        assert_eq!(opacity, 0.5);

        let (mode, _, _) = classify(&mat("ALPHACHANNEL", "ENABLE sa 1", 1.0));
        assert_eq!(mode, 2);

        let (mode, _, _) = classify(&mat("NORMAL", "ENABLE 0 1-s", 1.0));
        assert_eq!(mode, 3);

        let (mode, _, _) = classify(&mat("NORMAL", "ENABLE s 1", 1.0));
        assert_eq!(mode, 4);
    }

    #[test]
    fn drawable_index_bounds() {
        assert!(!is_drawable_material_index(-1, 3));
        assert!(!is_drawable_material_index(3, 3));
        assert!(is_drawable_material_index(0, 3));
        assert!(is_drawable_material_index(2, 3));
    }

    #[test]
    fn tex_override_mapping_documented() {
        const TEX_OVERRIDE: &[(&str, &str)] =
            &[("sn01z1.jpg", "sn01Z1.jpg"), ("sn01z2.jpg", "sn01Z2.jpg")];
        assert_eq!(
            TEX_OVERRIDE,
            &[("sn01z1.jpg", "sn01Z1.jpg"), ("sn01z2.jpg", "sn01Z2.jpg")]
        );
    }

    #[test]
    fn dynamic_weapon_drop_is_pickable_on_landing_before_temporary_timeout() {
        assert!(dynamic_weapon_drop_pickup_ready(0.34, false));
        assert!(dynamic_weapon_drop_pickup_ready(5.0, false));
        assert!(!dynamic_weapon_drop_pickup_ready(5.001, true));
        assert!(!dynamic_weapon_drop_pickup_ready(6.0, true));
    }

    #[test]
    fn offline_drop_item_copy_keeps_active_weapon_and_both_slots() {
        let mut fighter = crate::components::fighter::Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [2, 0],
            weapon_variant: 2,
        };
        let before = fighter.clone();
        assert_eq!(
            prepare_offline_weapon_drop(&mut fighter, false).as_deref(),
            Some("w0_2")
        );
        assert_eq!(fighter.weapon, before.weapon);
        assert_eq!(fighter.weapon_slots, before.weapon_slots);
        assert_eq!(fighter.weapon_slot, before.weapon_slot);
        assert_eq!(fighter.weapon_slot_variants, before.weapon_slot_variants);
        assert_eq!(fighter.weapon_variant, before.weapon_variant);
    }

    #[test]
    fn offline_h_copies_current_and_g_drops_spare_model_and_variant() {
        let mut fighter = crate::components::fighter::Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 1],
            weapon_variant: 0,
        };
        assert_eq!(
            prepare_offline_weapon_drop(&mut fighter, false).as_deref(),
            Some("w0_0"),
            "H copies the currently held sword without changing slots"
        );
        assert_eq!(
            prepare_offline_weapon_drop(&mut fighter, true).as_deref(),
            Some("w1_1"),
            "G removes the spare dagger with its own variant"
        );
        assert_eq!(
            (fighter.weapon, fighter.weapon_variant, fighter.weapon_slot),
            (6, 0, 0)
        );
        assert_eq!(fighter.weapon_slots, [6, 0]);
        assert_eq!(prepare_offline_weapon_drop(&mut fighter, true), None);
        assert_eq!(
            prepare_offline_weapon_drop(&mut fighter, false).as_deref(),
            Some("w0_0")
        );
    }

    #[test]
    fn origin_res_chain_resolves_sn01_textures() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01.pak").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let paks = stage_pak_chain(&root, "sn01");
        assert!(!paks.is_empty(), "sn01 pak chain loads");
        let refs: Vec<&PakArchive> = paks.iter().collect();

        assert!(
            resolve_pak_texture(&refs, "sn01z1.jpg").is_some(),
            "case-insensitive repair resolves sn01Z1.jpg"
        );

        let ifl = resolve_pak_texture(&refs, "s53f.ifl")
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        assert!(
            ifl.lines().any(|l| l.trim() == "s53f100.jpg"),
            "ifl list parses"
        );
    }

    #[test]
    fn origin_sn14_moon_billboard_keeps_authored_anchor_and_faces_camera() {
        use bevy::mesh::VertexAttributeValues;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmdl = decode_gmdl(&std::fs::read(root.join("sn14/sn14.gmb")).unwrap()).unwrap();
        let des = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn14/sn14.des")).unwrap(),
        );
        let moon = gmdl.objects.iter().find(|o| o.name == "Circle01").unwrap();
        let obj = des
            .scene_objects
            .iter()
            .find(|o| o.name == moon.name)
            .unwrap();
        assert_eq!(obj.billboard, 1);
        assert_eq!(moon.num_faces, 26);
        let mat = &gmdl.materials[moon.face_mat[0] as usize];
        assert_eq!(gmdl.resolve_texture_name(mat), Some("Asn14026.jpg"));
        assert_eq!(classify_gmdl_material(mat), (GmdlMode::Blend, 1.0, 0.0));
        let center = [173.0, -291.0, 42.0];
        let (mesh, transform) = build_scene_mesh(moon, &moon.indices, center, Some(obj));
        let Some(VertexAttributeValues::Float32x3(vertices)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("normals");
        };
        let axes = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        for (local, original) in vertices.iter().zip(moon.positions.chunks_exact(3)) {
            let expected =
                axes * (Vec3::from_slice(original) - Vec3::from_array(center)) * MODEL_SCALE;
            assert!(
                transform
                    .transform_point(Vec3::from_array(*local))
                    .distance(expected)
                    < 0.001
            );
            assert!(
                local[2].abs() < 0.2,
                "inverse DES rotation recovers XY plane"
            );
        }
        let anchor = transform.translation;
        let mut app = App::new();
        app.add_systems(PostUpdate, update_scene_billboard);
        let entity = app.world_mut().spawn((transform, SceneBillboard)).id();
        let camera = app
            .world_mut()
            .spawn((
                crate::plugins::camera_plugin::GameplayCamera,
                Transform::default(),
            ))
            .id();
        for offset in [
            Vec3::new(100.0, 25.0, 10.0),
            Vec3::new(-30.0, -20.0, 150.0),
            Vec3::new(10.0, 80.0, -60.0),
        ] {
            app.world_mut()
                .get_mut::<Transform>(camera)
                .unwrap()
                .translation = anchor + offset;
            app.update();
            let facing = app.world().get::<Transform>(entity).unwrap();
            assert_eq!(
                facing.translation, anchor,
                "camera must not orbit the moon around stage origin"
            );
            assert_eq!(facing.scale, transform.scale);
            for normal in normals {
                assert!(
                    (facing.rotation * Vec3::from_array(*normal)).dot(offset.normalize()) > 0.999
                );
            }
        }

        let mut images = Assets::<Image>::default();
        let mut pak_cache = PakCache::default();
        let loaded = load_or_cache_texture(
            &mut images,
            &mut HashMap::new(),
            &mut pak_cache,
            &root,
            "sn14",
            "Asn14026.jpg",
            false,
            true,
            0,
        );
        let color = meteor_asset::texture::decode_image(
            &pak_cache
                .resolve_exact(&root, "sn14", "Asn14026.jpg")
                .unwrap(),
        )
        .unwrap();
        let mask = meteor_asset::texture::decode_image(
            &pak_cache
                .resolve_exact(&root, "sn14", "Asn14026.bmp")
                .unwrap(),
        )
        .unwrap();
        let image = images.get(loaded.handle.as_ref().unwrap()).unwrap();
        for ((pixel, rgb), alpha) in image
            .data
            .as_ref()
            .unwrap()
            .chunks_exact(4)
            .take((color.width * color.height) as usize)
            .zip(color.rgba.chunks_exact(4))
            .zip(mask.rgba.chunks_exact(4))
        {
            assert_eq!(&pixel[..3], &rgb[..3]);
            assert_eq!(pixel[3], alpha[0]);
        }
    }

    #[test]
    fn origin_sn06_fog_uses_jpeg_rgb_and_bmp_companion_alpha() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("sn06.pak").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let chain = stage_pak_chain(&origin, "sn06");
        let refs: Vec<&PakArchive> = chain.iter().collect();
        let color_bytes = resolve_pak_texture(&refs, "Asn06wu.jpg").expect("fog JPEG color");
        let alpha_bytes = resolve_pak_texture(&refs, "Asn06wu.bmp").expect("fog BMP alpha");
        let color = meteor_asset::texture::decode_image(&color_bytes).expect("decode color");
        let alpha = meteor_asset::texture::decode_image(&alpha_bytes).expect("decode alpha");
        let mut images = Assets::<bevy::image::Image>::default();
        let handle =
            image_handle_with_legacy_alpha(&mut images, &color_bytes, Some(&alpha_bytes), false)
                .expect("composite fog texture");
        let rgba = images
            .get(&handle)
            .and_then(|image| image.data.as_deref())
            .expect("CPU image data");

        assert_eq!(&rgba[0..3], &color.rgba[0..3], "JPG remains RGB source");
        assert_eq!(rgba[3], alpha.rgba[0], "BMP red channel becomes alpha");
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] < 240));
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] > 16));
    }

    #[test]
    fn origin_sn03_grass_uses_8bit_mask_and_source_scope() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("sn03.pak").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut pak_cache = PakCache::default();
        let direct = pak_cache
            .resolve(&origin, "sn03", "sn3tr.JPG")
            .expect("sn03 grass color");
        let scoped = pak_cache
            .resolve(&origin, "sn01", "sn03/sn3tr.JPG")
            .expect("cross-stage grass color");
        assert_eq!(direct, scoped, "sn03/ prefix owns the texture scope");

        let mut images = Assets::<bevy::image::Image>::default();
        let mut cache = HashMap::new();
        let loaded = load_or_cache_texture(
            &mut images,
            &mut cache,
            &mut pak_cache,
            &origin,
            "sn01",
            "sn03/sn3tr.JPG",
            false,
            false,
            0,
        );
        assert!(loaded.alpha_test, "NORMAL grass mask promotes alpha-test");
        let rgba = images
            .get(&loaded.handle.expect("masked grass image"))
            .and_then(|image| image.data.as_deref())
            .expect("grass CPU pixels");
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] < 16));
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] > 200));
    }

    #[test]
    fn origin_sn15_furniture_preserves_authored_companion_alpha() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !origin.join("sn15.pak").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let gmdl = decode_gmdl(&std::fs::read(origin.join("sn15/sn15.gmb")).unwrap()).unwrap();
        let mut images = Assets::<bevy::image::Image>::default();
        let mut cache = HashMap::new();
        let mut pak_cache = PakCache::default();
        for name in ["km1507.JPG", "km1596.JPG", "km15004.JPG"] {
            let material = gmdl
                .materials
                .iter()
                .find(|mat| gmdl.resolve_texture_name(mat) == Some(name))
                .expect("original furniture material");
            assert_eq!(material.blend, "COLORKEY");
            let color = meteor_asset::texture::decode_image(
                &pak_cache.resolve_exact(&origin, "sn15", name).unwrap(),
            )
            .unwrap();
            let mask_name = legacy_alpha_companion_name(name).unwrap();
            let mask = meteor_asset::texture::decode_image(
                &pak_cache
                    .resolve_exact(&origin, "sn15", &mask_name)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!((color.width, color.height), (mask.width, mask.height));
            let loaded = load_or_cache_texture(
                &mut images,
                &mut cache,
                &mut pak_cache,
                &origin,
                "sn15",
                name,
                true,
                false,
                0,
            );
            let rgba = images
                .get(&loaded.handle.unwrap())
                .unwrap()
                .data
                .as_deref()
                .unwrap();
            for ((pixel, rgb), alpha) in rgba
                .chunks_exact(4)
                .zip(color.rgba.chunks_exact(4))
                .zip(mask.rgba.chunks_exact(4))
            {
                assert_eq!(&pixel[..3], &rgb[..3], "{name}: preserve authored RGB");
                assert_eq!(pixel[3], alpha[0], "{name}: mask, without colour-key holes");
            }
            assert!(rgba.chunks_exact(4).any(|p| p[3] == 0));
            assert!(rgba.chunks_exact(4).any(|p| p[3] == 255));
        }
    }

    #[test]
    fn origin_static_colorkey_material_punches_background() {
        let origin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let gmb_path = origin.join("sn03/sn03.gmb");
        if !gmb_path.is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let gmdl =
            decode_gmdl(&std::fs::read(gmb_path).expect("sn03.gmb")).expect("decode sn03.gmb");
        let mut images = Assets::<bevy::image::Image>::default();
        let mut cache = HashMap::new();
        let mut pak_cache = PakCache::default();
        let mut matched = None;
        for material in &gmdl.materials {
            if !material.blend.eq_ignore_ascii_case("COLORKEY") {
                continue;
            }
            let Some(name) = gmdl.resolve_texture_name(material) else {
                continue;
            };
            if name.to_ascii_lowercase().ends_with(".ifl") {
                continue;
            }
            let loaded = load_or_cache_texture(
                &mut images,
                &mut cache,
                &mut pak_cache,
                &origin,
                "sn03",
                name,
                true,
                false,
                0,
            );
            let Some(rgba) = loaded
                .handle
                .as_ref()
                .and_then(|handle| images.get(handle))
                .and_then(|image| image.data.as_deref())
            else {
                continue;
            };
            if rgba.chunks_exact(4).any(|pixel| pixel[3] == 0)
                && rgba.chunks_exact(4).any(|pixel| pixel[3] == 255)
            {
                matched = Some(name.to_string());
                break;
            }
        }
        assert!(
            matched.is_some(),
            "no real sn03 COLORKEY texture was punched"
        );
    }

    #[test]
    fn origin_res_ctexture_resolves_prop_texture() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("ctexture.pak").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let paks = stage_pak_chain(&root, "sn01");
        let refs: Vec<&PakArchive> = paks.iter().collect();
        assert!(
            resolve_pak_texture(&refs, "An07y1.BMP").is_some(),
            "ctexture mixed-case entry resolves"
        );
    }

    #[test]
    fn origin_sn01_bbox_bounds_keep_the_bottom_origin_offset() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("cmodel/BBox02.gmb");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mesh =
            decode_gmdl(&std::fs::read(path).expect("BBox02.gmb")).expect("decode BBox02.gmb");
        let (center, half_extents) = prop_bounds(&mesh, 1.0);

        assert!(center.x.abs() < 1e-4 && center.y.abs() < 1e-4);
        assert!(
            (center.z - 1.593_381_4).abs() < 1e-3,
            "BBox02 origin is at the bottom; authored center must survive: {center:?}"
        );
        assert!(
            (half_extents.z - 1.579_031).abs() < 1e-3,
            "BBox02 authored half height changed unexpectedly: {half_extents:?}"
        );
    }

    #[test]
    fn origin_ground_weapon_gmc_uses_the_shared_prop_model_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("cmodel/w0_0.GMC").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let model = load_prop_model(std::slice::from_ref(&root), &model_stems("W0_0"))
            .expect("GMC-only ground sword must load");
        assert!(!model.objects.is_empty());
        assert!(model
            .objects
            .iter()
            .all(|object| !object.indices.is_empty()));
        assert!(model
            .materials
            .iter()
            .any(|material| material.tex_index >= 0));
        let (center, half_extents) = prop_bounds(&model, 1.0);
        assert!(center.is_finite() && half_extents.is_finite());
        assert!(half_extents.max_element() > 1.0);

        assert!(load_prop_model(std::slice::from_ref(&root), &model_stems("W8_0")).is_some());
        assert!(load_prop_model(std::slice::from_ref(&root), &model_stems("W8_3")).is_some());
    }

    #[test]
    fn all_authored_ground_item_models_resolve_as_gmb_or_gmc() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01/sn01.des").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut models = std::collections::BTreeSet::new();
        for stage_number in 1..=22 {
            let stage = format!("sn{stage_number:02}");
            let path = root.join(&stage).join(format!("{stage}.des"));
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let desc = meteor_asset::des::parse_stage_description(&text);
            for dummy in desc.dummies {
                let Some(model) = dummy.model else {
                    continue;
                };
                let lower = model.to_ascii_lowercase();
                if lower.starts_with("item")
                    || lower == "hot"
                    || lower
                        .strip_prefix('w')
                        .is_some_and(|tail| tail.starts_with(|c: char| c.is_ascii_digit()))
                {
                    models.insert(lower);
                }
            }
        }
        assert_eq!(models.len(), 82, "authored pickup-model inventory drifted");
        let missing = models
            .iter()
            .filter(|model| {
                load_prop_model(std::slice::from_ref(&root), &model_stems(model)).is_none()
            })
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "unresolved ground item models: {missing:?}"
        );
    }

    #[test]
    fn prop_pose_end_zero_stays_on_start_frame() {
        let idle = meteor_asset::fmc::PropPose { start: 0, end: 0 };
        assert_eq!(prop_pose_clip_end(&idle), 0);
        let brk = meteor_asset::fmc::PropPose { start: 0, end: 60 };
        assert_eq!(prop_pose_clip_end(&brk), 60);
    }

    #[test]
    fn bbox_fmc_moves_between_authored_frames_and_preserves_cycle_endpoint() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let source = meteor_asset::fmc::parse_fmc(
            &std::fs::read_to_string(root.join("cmodel/BBox02.fmc")).unwrap(),
        );
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<StageCollider>();
        let entity = world
            .spawn((
                Transform::from_scale(Vec3::splat(MODEL_SCALE)),
                FmcClip {
                    name: "D_BBox01".into(),
                    clip: source.clone(),
                    part_index: 0,
                    poses: vec![],
                    pose_index: 1,
                    pose_start: 0,
                    pose_end: 60,
                    pose_loop: 0,
                    pose_revision: 1,
                    anim_time: 0.0,
                    state: 1,
                    frame_set: None,
                    multi: true,
                    base_translation: Vec3::ZERO,
                    base_rotation: Quat::IDENTITY,
                    collider_owner: None,
                    collider_vertex_offset: 0,
                    local_verts: vec![],
                    last_reported_state: 1,
                },
            ))
            .id();
        let rest = &source.frames[0].parts[0];
        let rest_q = Quat::from_xyzw(rest.q.x, rest.q.y, rest.q.z, rest.q.w);
        let rest_t = Vec3::new(rest.t.x, rest.t.y, rest.t.z);

        let mut previous = None;
        for frame in [10.0, 10.25, 10.5, 10.75, 11.0, 60.0, 60.25, 120.25] {
            let looping = frame > 60.0;
            {
                let mut clip = world.get_mut::<FmcClip>(entity).unwrap();
                clip.anim_time = frame / 30.0;
                clip.pose_loop = u32::from(looping);
            }
            world.run_system_once(update_fmc_clips).unwrap();
            let transform = *world.get::<Transform>(entity).unwrap();
            let sample = source
                .sample_part(if looping { 0.25 } else { frame }, 0)
                .unwrap();
            let delta =
                Quat::from_xyzw(sample.q.x, sample.q.y, sample.q.z, sample.q.w) * rest_q.inverse();
            let expected =
                (Vec3::new(sample.t.x, sample.t.y, sample.t.z) - delta * rest_t) * MODEL_SCALE;
            assert!(
                transform.translation.distance(expected) < 1e-4,
                "frame {frame}"
            );
            if frame <= 11.0 {
                if let Some(last) = previous {
                    assert!(transform.translation.distance(last) > 1e-5);
                }
                previous = Some(transform.translation);
            }
            assert_eq!(
                world.get::<FmcClip>(entity).unwrap().state,
                if frame == 60.0 { 3 } else { 1 }
            );
        }

        world.get_mut::<FmcClip>(entity).unwrap().pose_end = 0;
        world.run_system_once(update_fmc_clips).unwrap();
        assert!(world.get::<Transform>(entity).unwrap().translation.length() < 1e-5);
        assert_eq!(world.get::<FmcClip>(entity).unwrap().state, 3);
    }

    #[test]
    fn script_pose_request_enters_playing_until_multiframe_clip_finishes() {
        let poses = [
            meteor_asset::fmc::PropPose { start: 0, end: 0 },
            meteor_asset::fmc::PropPose { start: 1, end: 60 },
        ];
        assert_eq!(requested_prop_pose(&poses, 0, true), Some((0, 0, 3)));
        assert_eq!(requested_prop_pose(&poses, 1, true), Some((1, 60, 1)));
        assert_eq!(requested_prop_pose(&poses, 99, true), Some((1, 60, 1)));
        assert_eq!(requested_prop_pose(&poses, 1, false), Some((1, 60, 3)));
        assert_eq!(requested_prop_pose(&[], 1, true), None);
    }

    #[test]
    fn solid_prop_is_opt_in_like_web() {
        let des = meteor_asset::des::parse_stage_description(
            r#"SceneObjects 0 DummeyObjects 3
Object D_BBox01 { Position: 0 0 0 Quaternion: 1 0 0 0 Custom: { model="BBox02" } }
Object D_Wpn14 { Position: 0 0 0 Quaternion: 1 0 0 0 Custom: { model="W0_0" } }
Object D_roller { Position: 0 0 0 Quaternion: 1 0 0 0 Custom: { model="sn08B01" collision=1 name="damage100" } }
"#,
        );
        assert!(
            is_solid_prop(&des.dummies[0]),
            "BBox solid by model pattern"
        );
        assert!(
            !is_solid_prop(&des.dummies[1]),
            "weapon stays non-solid despite SceneObject-style block_player default"
        );
        assert!(
            is_solid_prop(&des.dummies[2]),
            "collision=1 wins over damage name (sn08 roller)"
        );
    }

    #[test]
    fn origin_sn03_flag_pickup_matches_rendered_prop_and_walkable_ground() {
        use meteor_physics::collision::MeshCollider;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn03/sn03.des")).unwrap(),
        );
        let wp = std::fs::read_to_string(root.join("Level/sn03.wp")).unwrap();
        let center = meteor_scene::stage_loader::get_stage_center("sn03", Some(&wp));
        let dummy = desc
            .dummies
            .iter()
            .find(|d| d.name == "D_IPItem02")
            .unwrap();
        let position = prop_transform(dummy, center).translation;
        let home = meteor_network::escort::SN03.flag_home;
        assert!(position.distance(Vec3::new(home.x, home.y, home.z)) < 0.001);
        let gmdl = decode_gmdl(&std::fs::read(root.join("sn03/sn03.gmb")).unwrap()).unwrap();
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn03/sn03.cob")).unwrap())
            .unwrap();
        let collider = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
        let ground = collider
            .sample_ground_hit(position.x, position.z, position.y, position.y)
            .unwrap();
        assert!(!collider.box_blocked(position.x, ground.y, position.z));
        assert!(
            meteor_network::escort::character_capsule_intersects_flag(
                meteor_network::role_state::RoleVec3 {
                    y: ground.y,
                    ..home
                },
                home
            ),
            "authored flag at {position:?}, walkable ground at {}",
            ground.y
        );
    }

    #[test]
    fn origin_sn03_invisible_pd02box_keeps_the_breakable_gate_blocking() {
        use meteor_physics::collision::MeshCollider;
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn03/sn03.des").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let des_text = std::fs::read_to_string(root.join("sn03/sn03.des")).expect("sn03.des");
        let desc = meteor_asset::des::parse_stage_description(&des_text);
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: [
                    (
                        "active".to_string(),
                        PstValue::Num(dummy.visible as i32 as f64),
                    ),
                    (
                        "collision".to_string(),
                        PstValue::Num(is_solid_prop(dummy) as i32 as f64),
                    ),
                    ("interactive".to_string(), PstValue::Num(1.0)),
                    ("pose".to_string(), PstValue::Num(0.0)),
                    ("pose_loop".to_string(), PstValue::Num(0.0)),
                    ("frame".to_string(), PstValue::Num(-1.0)),
                    ("state".to_string(), PstValue::Num(3.0)),
                ]
                .into_iter()
                .collect(),
            })
            .collect();
        let mut script_parts = ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"]
            .into_iter()
            .map(|name| {
                std::fs::read_to_string(root.join("PScriptLib").join(name))
                    .unwrap_or_else(|_| String::new())
            })
            .collect::<Vec<_>>();
        script_parts.push(std::fs::read_to_string(root.join("sn03/sn03.pst")).expect("sn03.pst"));
        let mut runtime = SceneScriptRuntime::load(&script_parts.join("\n"), instances, 250_000)
            .expect("sn03 scene runtime");
        runtime.start().expect("sn03 scene init");

        let proxy = desc
            .dummies
            .iter()
            .find(|dummy| dummy.name == "D_APd02Box01")
            .expect("A gate collision proxy");
        let state = runtime
            .host()
            .instances()
            .iter()
            .find(|instance| instance.name == proxy.name)
            .expect("proxy runtime state");
        assert_eq!(state.get("active").as_f64(), 0.0, "proxy stays invisible");
        assert_eq!(
            state.get("collision").as_f64(),
            1.0,
            "Scene_OnInit promotes the invisible proxy to a blocker"
        );
        assert!(state.collision_enabled());

        let model =
            decode_gmdl(&std::fs::read(root.join("cmodel/Pd02Box.gmb")).expect("Pd02Box.gmb"))
                .expect("decode Pd02Box");
        assert!(model.materials.is_empty());
        assert!(model
            .objects
            .iter()
            .flat_map(|object| &object.face_mat)
            .all(|material| *material == -1));
        let transform = prop_transform(proxy, meteor_scene::stage_loader::ARENA_CENTER);
        let (vertices, indices, _) = prop_collision_geometry(&model, proxy, transform);
        assert_eq!(indices.len(), 36, "all 12 render-empty proxy faces collide");
        let mut collider = StageMeshCollider::default();
        let owner = collider.append_world_triangles(&vertices, &indices);
        collider.set_owner_enabled(owner, true);
        let (center_offset, _) = prop_bounds(&model, proxy.scale);
        let center = transform.translation + transform.rotation * center_offset;
        assert!(
            collider.box_blocked(center.x, transform.translation.y, center.z),
            "the authored player body is blocked at the gate proxy"
        );

        let initial_instances = runtime.host().instances().to_vec();
        runtime.host_mut().take_net_ops();
        for side in ["A", "B"] {
            runtime
                .call_item(
                    &format!("D_{side}Pdoor01"),
                    "OnAttack",
                    &[PstValue::Num(0.0), PstValue::Num(100_000.0)],
                )
                .unwrap();
            let name = format!("D_{side}Pd02Box01");
            let proxy = runtime
                .host()
                .instances()
                .iter()
                .find(|i| i.name == name)
                .unwrap();
            assert_eq!(proxy.get("active").as_f64(), 0.0);
            assert_eq!(proxy.get("collision").as_f64(), 1.0);
            assert!(
                !proxy.collision_enabled(),
                "destroyed {name} must stop blocking"
            );
        }
        let mut remote =
            SceneScriptRuntime::load(&script_parts.join("\n"), initial_instances, 250_000).unwrap();
        for op in runtime.host_mut().take_net_ops() {
            remote.host_mut().apply_net_op(&op);
        }
        for side in ["A", "B"] {
            let name = format!("D_{side}Pd02Box01");
            assert!(!remote
                .host()
                .instances()
                .iter()
                .find(|i| i.name == name)
                .unwrap()
                .collision_enabled());
        }
        let state = runtime
            .host()
            .instances()
            .iter()
            .find(|i| i.name == "D_APd02Box01")
            .unwrap();
        collider.set_owner_enabled(owner, state.collision_enabled());
        assert!(
            !collider.box_blocked(center.x, transform.translation.y, center.z),
            "the destroyed gate must be traversable"
        );
    }

    #[test]
    fn render_empty_touch_carrier_keeps_script_active_visibility() {
        assert_eq!(prop_visibility(true), Visibility::Visible);
        assert_eq!(prop_visibility(false), Visibility::Hidden);
    }
}
