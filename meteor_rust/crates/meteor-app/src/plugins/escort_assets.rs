use bevy::prelude::*;
use meteor_network::escort_definition::EscortDefinition;
use std::sync::Arc;

#[derive(Resource, Default)]
pub(crate) struct LoadedEscort(pub Option<Arc<EscortDefinition>>);

pub(crate) struct StageAssetReader {
    stage_id: String,
    roots: Vec<std::path::PathBuf>,
    pack: Option<meteor_asset::pak::PakArchive>,
    authoring_fallback: bool,
}

impl StageAssetReader {
    pub fn new(stage_id: &str) -> Self {
        let roots =
            super::stage_plugin::resource_roots(&std::env::current_dir().unwrap_or_default());
        let pack = None;
        Self {
            stage_id: stage_id.into(),
            roots,
            pack,
            authoring_fallback: false,
        }
    }

    fn for_authoring(stage_id: &str) -> Self {
        Self {
            authoring_fallback: true,
            ..Self::new(stage_id)
        }
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_with_authoring_fallback(path, |path| {
            #[cfg(target_os = "android")]
            {
                crate::android_assets::read_authoring_asset(&self.stage_id, path)
            }
            #[cfg(not(target_os = "android"))]
            {
                let _ = path;
                None
            }
        })
    }

    fn read_with_authoring_fallback(
        &self,
        path: &str,
        fallback: impl FnOnce(&str) -> Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        let mapped = if path.starts_with(&format!("{}/", self.stage_id)) {
            format!("stage/{}", path.rsplit('/').next()?)
        } else {
            path.to_ascii_lowercase()
        };
        self.pack
            .as_ref()
            .and_then(|p| p.read_entry(&mapped))
            .or_else(|| meteor_asset::runtime_assets::read_legacy_path(path))
            .or_else(|| {
                self.roots
                    .iter()
                    .find_map(|root| crate::resource_io::read(root.join(path)).ok())
            })
            .or_else(|| self.authoring_fallback.then(|| fallback(path)).flatten())
    }
}

pub(crate) fn load_escort(stage_id: &str) -> Result<Arc<EscortDefinition>, String> {
    let reader = StageAssetReader::for_authoring(stage_id);
    load_escort_from_reader(&reader)
}

fn load_escort_from_reader(reader: &StageAssetReader) -> Result<Arc<EscortDefinition>, String> {
    let stage_id = &reader.stage_id;
    let read = |path: &str| reader.read(path);
    let des = read(&format!("{stage_id}/{stage_id}.des")).ok_or("地图缺少 DES")?;
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));

    for name in ["safeA", "safeB"] {
        if !desc.scene_objects.iter().any(|o| {
            o.name_attr
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        }) {
            return Err(format!("地图缺少 {name} 安全区"));
        }
    }
    let registry = read("Meteor.res").ok_or("地图缺少资源定义 Meteor.res")?;
    let registry = meteor_asset::res::parse_meteor_res(&registry).map_err(|e| e.to_string())?;
    let bytes = read(&format!("{stage_id}/{stage_id}.gmb")).ok_or("地图缺少 GMB")?;
    let mesh = meteor_asset::gmdl::decode_gmdl(&bytes).map_err(|e| e.to_string())?;
    let wp = read(&format!("Level/{stage_id}.wp")).map(|b| crate::text_codec::decode_text(&b));
    let center = meteor_scene::stage_loader::get_stage_center(stage_id, wp.as_deref());
    EscortDefinition::from_assets(&desc, &mesh, &registry, center, |model| {
        super::render_plugin::load_prop_model_with(&super::render_plugin::model_stems(model), read)
    })
    .map(Arc::new)
}

#[derive(Component)]
pub(crate) struct EscortPropVisual {
    pub home: Vec3,
    pub applied_offset: Vec3,
}

type EscortAnchors<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, &'static Visibility),
    (
        Without<EscortPropVisual>,
        Or<(
            With<super::stage_plugin::NetworkEscortFlag>,
            With<super::stage_plugin::LocalEscortState>,
        )>,
    ),
>;

pub(crate) fn sync_escort_models(
    anchors: EscortAnchors<'_, '_>,
    mut props: Query<(
        &mut Transform,
        &mut Visibility,
        &mut EscortPropVisual,
        Option<&mut super::render_plugin::FmcClip>,
    )>,
) {
    let Ok((anchor, visible)) = anchors.single() else {
        return;
    };
    for (mut transform, mut visibility, mut visual, clip) in &mut props {
        let offset = anchor.translation - visual.home;
        let delta = offset - visual.applied_offset;
        transform.translation += delta;
        if let Some(mut clip) = clip {
            clip.base_translation += delta;
        }
        visual.applied_offset = offset;
        *visibility = *visible;
    }
}

#[allow(clippy::unnecessary_lazy_evaluations)]
pub(crate) fn custom_stage_entries() -> Vec<meteor_scene::stage_registry::StageInfo> {
    if let Some(manifest) = meteor_asset::runtime_assets::manifest() {
        return meteor_scene::stage_registry::custom_stages_from_manifest(&manifest);
    }

    let current = std::env::current_dir().unwrap_or_default();
    super::stage_plugin::resource_roots(&current)
        .iter()
        .find_map(|root| crate::resource_io::read_to_string(root.join("packs/packs.json")).ok())
        .or_else(|| {
            #[cfg(target_os = "android")]
            {
                crate::android_assets::authoring_manifest()
            }
            #[cfg(not(target_os = "android"))]
            {
                None
            }
        })
        .map(|text| meteor_scene::stage_registry::custom_stages_from_manifest(&text))
        .unwrap_or_default()
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sn06_loaded_packs_finish_prop_probes_without_reopening_apk() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let packs_dir = repo.join("asset-tools/packs");
        let index = crate::pack_manifest::parse_pack_index(
            &std::fs::read_to_string(packs_dir.join("packs.json")).unwrap(),
        )
        .unwrap();
        let mut entries = std::collections::BTreeMap::new();
        for id in ["common", "stage/sn06"] {
            let bytes = std::fs::read(packs_dir.join(&index[id].file_name)).unwrap();
            let pack = meteor_asset::pak::PakArchive::from_mpk1(&bytes).unwrap();
            for entry in pack.entries() {
                entries.insert(entry.name.clone(), pack.read_entry(&entry.name).unwrap());
            }
        }
        let mut manifest = Vec::new();
        let mut blob = Vec::new();
        for (name, bytes) in entries {
            manifest.push(serde_json::json!({"name":name,"offset":blob.len(),"size":bytes.len()}));
            blob.extend(bytes);
        }
        let manifest = serde_json::to_vec(
            &serde_json::json!({"formatVersion":1,"kind":"test","entries":manifest}),
        )
        .unwrap();
        let mut bytes = b"MPK1".to_vec();
        bytes.extend((manifest.len() as u32).to_le_bytes());
        bytes.extend(manifest);
        bytes.extend(blob);

        let mut reader = StageAssetReader::new("sn06");
        reader.roots.clear();
        reader.pack = Some(meteor_asset::pak::PakArchive::from_mpk1(&bytes).unwrap());
        let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(
            &reader.read("sn06/sn06.des").unwrap(),
        ));
        assert_eq!(desc.dummies.len(), 131, "same stage as the user's log");
        let mut fallback_reads = 0;
        let mut misses = 0;
        let mut read = |path: &str| {
            let bytes = reader.read_with_authoring_fallback(path, |_| {
                fallback_reads += 1;
                None
            });
            misses += usize::from(bytes.is_none());
            bytes
        };
        let mut models = 0;
        for dummy in &desc.dummies {
            let Some(model) = dummy.model.as_deref() else {
                continue;
            };
            let stems = super::super::render_plugin::model_stems(model);
            assert!(
                super::super::render_plugin::load_prop_model_with(&stems, &mut read).is_some(),
                "missing packed model {model}"
            );
            for extension in ["fmc", "pos"] {
                let _ = stems.iter().find_map(|stem| {
                    ["cmodel", "pmodel"]
                        .iter()
                        .find_map(|dir| read(&format!("{dir}/{stem}.{extension}")))
                });
            }
            models += 1;
        }
        assert!(models > 0);
        assert!(
            misses > 100,
            "exercise optional-file misses, not just happy paths"
        );
        assert_eq!(
            fallback_reads, 0,
            "gameplay must never reopen/rehash APK packs for optional-file misses"
        );
        eprintln!("sn06: {models} prop models loaded; {misses} optional misses; {fallback_reads} APK fallback reads");
    }

    #[test]
    fn menu_authoring_reader_retains_its_explicit_apk_fallback() {
        let mut reader = StageAssetReader::for_authoring("editor_missing");
        reader.roots.clear();
        let mut paths = Vec::new();
        let result =
            reader.read_with_authoring_fallback("editor_missing/editor_missing.des", |path| {
                paths.push(path.to_string());
                Some(b"authoring descriptor".to_vec())
            });
        assert_eq!(result.as_deref(), Some(b"authoring descriptor".as_slice()));
        assert_eq!(paths, ["editor_missing/editor_missing.des"]);
    }

    #[test]
    fn custom_pack_can_bind_an_unregistered_model_to_an_objective_record() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let des = std::fs::read_to_string(root.join("sn03/sn03.des"))
            .unwrap()
            .replace("Object D_IPItem02", "Object D_RelicAnchor")
            .replace(
                "model=\"IPItem02\"",
                "model=\"RelicMesh\"\n    resource=4,2",
            );
        let entries = [
            ("stage/editor_geometry.des", des.into_bytes()),
            (
                "stage/editor_geometry.gmb",
                std::fs::read(root.join("sn03/sn03.gmb")).unwrap(),
            ),
            (
                "level/editor_geometry.wp",
                std::fs::read(root.join("Level/sn03.wp")).unwrap(),
            ),
            (
                "cmodel/relicmesh.gmb",
                std::fs::read(root.join("cmodel/BBox02.gmb")).unwrap(),
            ),
        ];
        let mut blob = Vec::new();
        let mut manifest = Vec::new();
        for (name, bytes) in entries {
            manifest.push(
                serde_json::json!({"name":name,"offset":blob.len(),"size":bytes.len(),"mime":""}),
            );
            blob.extend(bytes);
        }
        let manifest = serde_json::to_vec(
            &serde_json::json!({"formatVersion":1,"kind":"stage","entries":manifest}),
        )
        .unwrap();
        let mut bytes = b"MPK1".to_vec();
        bytes.extend((manifest.len() as u32).to_le_bytes());
        bytes.extend(manifest);
        bytes.extend(blob);
        let reader = StageAssetReader {
            stage_id: "editor_geometry".into(),
            roots: vec![root],
            pack: Some(meteor_asset::pak::PakArchive::from_mpk1(&bytes).unwrap()),
            authoring_fallback: false,
        };
        let definition = load_escort_from_reader(&reader).unwrap();
        assert_eq!(definition.model, "RelicMesh");
        assert_eq!(definition.scene_name, "D_RelicAnchor");
        assert_eq!(definition.item_id, 2);
        assert!(
            definition.vertices.iter().any(|p| p[1] > 2.0),
            "uses the custom packed box geometry"
        );
        assert!(definition.intersects(definition.stage.flag_home, definition.stage.flag_home));
    }

    #[test]
    fn model_batches_keep_authored_shape_and_transform_through_pickup_and_drop() {
        let mut app = App::new();
        app.add_systems(Update, sync_escort_models);
        let home = Vec3::new(70.0, 8.0, -32.0);
        let anchor = app
            .world_mut()
            .spawn((
                super::super::stage_plugin::NetworkEscortFlag,
                Transform::from_translation(home),
                Visibility::Visible,
            ))
            .id();
        let rotation = Quat::from_rotation_y(0.9);
        let scale = Vec3::new(2.0, 3.0, 4.0);
        let mut batches = Vec::new();
        for part in [Vec3::ZERO, Vec3::Y * 3.0] {
            batches.push((
                app.world_mut()
                    .spawn((
                        EscortPropVisual {
                            home,
                            applied_offset: Vec3::ZERO,
                        },
                        Transform {
                            translation: home + part,
                            rotation,
                            scale,
                        },
                        Visibility::Visible,
                        Mesh3d::default(),
                    ))
                    .id(),
                part,
            ));
        }
        for (position, visibility) in [
            (home, Visibility::Visible),
            (home + Vec3::X * 9.0, Visibility::Hidden),
            (home + Vec3::Z * 12.0, Visibility::Visible),
            (home, Visibility::Visible),
        ] {
            app.world_mut()
                .get_mut::<Transform>(anchor)
                .unwrap()
                .translation = position;
            *app.world_mut().get_mut::<Visibility>(anchor).unwrap() = visibility;
            app.update();
            for (entity, part) in &batches {
                let actual = app.world().get::<Transform>(*entity).unwrap();
                assert_eq!(actual.translation, position + *part);
                assert_eq!(actual.rotation, rotation);
                assert_eq!(actual.scale, scale);
                assert!(app.world().get::<Mesh3d>(*entity).is_some());
                assert_eq!(*app.world().get::<Visibility>(*entity).unwrap(), visibility);
            }
        }
    }
}
