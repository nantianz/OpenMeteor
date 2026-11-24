use super::{config, GraphicsPacks};
use crate::app_state::{LoadedStageInfo, Settings};
use bevy::{
    core_pipeline::prepass::DepthPrepass, prelude::*, render::render_resource::TextureFormat,
};
use std::collections::HashMap;

pub(crate) struct ResolvedTrail {
    pub style: config::WeaponTrail,
    pub texture: Option<Handle<Image>>,
}

pub(crate) struct ResolvedRule {
    pub rule: config::EffectRule,

    pub textures: Vec<Option<Handle<Image>>>,
}

#[derive(Resource, Default)]
pub(crate) struct EffectEnhancements {
    pub enabled: bool,

    pub generation: u64,
    trails: HashMap<u8, ResolvedTrail>,
    exact: HashMap<String, usize>,
    patterns: Vec<(String, usize)>,
    rules: Vec<ResolvedRule>,
    stage: Option<String>,
    settings_enabled: Option<bool>,
}

impl EffectEnhancements {
    pub fn trail(&self, weapon: u8) -> Option<(u8, &ResolvedTrail)> {
        if !self.enabled {
            return None;
        }
        self.trails
            .get_key_value(&weapon)
            .or_else(|| self.trails.get_key_value(&0))
            .map(|(key, trail)| (*key, trail))
    }

    pub fn rule(&self, effect: &str) -> Option<&ResolvedRule> {
        if !self.enabled {
            return None;
        }
        let stem = effect_stem(effect);
        if let Some(&index) = self.exact.get(&stem) {
            return Some(&self.rules[index]);
        }

        self.patterns
            .iter()
            .filter(|(pattern, _)| glob_match(pattern, &stem))
            .max_by_key(|(pattern, index)| {
                (
                    pattern.chars().filter(|c| !matches!(c, '*' | '?')).count(),
                    *index,
                )
            })
            .map(|(_, index)| &self.rules[*index])
    }

    pub fn wants_depth_prepass(&self) -> bool {
        self.enabled
            && self
                .rules
                .iter()
                .any(|resolved| resolved.rule.soft || resolved.rule.emitters.iter().any(|e| e.soft))
    }
}

#[cfg(test)]
impl EffectEnhancements {
    pub(crate) fn with_rule(stem: &str, rule: config::EffectRule) -> Self {
        let textures = vec![None; rule.emitters.len()];
        let mut out = Self {
            enabled: true,
            ..default()
        };
        out.rules.push(ResolvedRule { rule, textures });
        out.exact.insert(stem.to_ascii_lowercase(), 0);
        out
    }
}

pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub(crate) fn effect_stem(effect: &str) -> String {
    let name = effect.rsplit(['/', '\\']).next().unwrap_or(effect);
    let name = name
        .strip_suffix(".ef")
        .or_else(|| name.strip_suffix(".EF"))
        .unwrap_or(name);
    name.to_ascii_lowercase()
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<EffectEnhancements>()
        .add_systems(Update, (resolve, sync_depth_prepass).chain());
}

fn resolve(
    packs: Option<Res<GraphicsPacks>>,
    loaded: Option<Res<LoadedStageInfo>>,
    settings: Option<Res<Settings>>,
    mut enhancements: ResMut<EffectEnhancements>,
    mut images: ResMut<Assets<Image>>,
) {
    let settings_enabled = settings.as_ref().is_none_or(|s| s.effect_enhance);
    let stage = loaded.as_ref().map(|loaded| loaded.stage_id.clone());
    let packs_changed = packs.as_ref().is_some_and(|packs| packs.is_changed());
    if !packs_changed
        && enhancements.stage == stage
        && enhancements.settings_enabled == Some(settings_enabled)
    {
        return;
    }
    enhancements.stage = stage.clone();
    enhancements.settings_enabled = Some(settings_enabled);
    enhancements.trails.clear();
    enhancements.exact.clear();
    enhancements.patterns.clear();
    enhancements.rules.clear();
    enhancements.generation += 1;
    let (Some(packs), Some(stage)) = (packs.as_deref(), stage) else {
        enhancements.enabled = false;
        return;
    };
    let mut texture_cache: HashMap<(usize, String), Handle<Image>> = HashMap::new();
    let mut texture = |pack: usize,
                       id: &str,
                       images: &mut Assets<Image>|
     -> Option<Handle<Image>> {
        let decoded = packs.packs[pack].images.get(id)?;
        Some(
            texture_cache
                .entry((pack, id.to_owned()))
                .or_insert_with(|| {
                    let handle = crate::plugins::pak_textures::image_from_decoded(images, decoded);
                    if let Some(mut image) = images.get_mut(&handle) {
                        image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
                        let (bytes, count) = super::surfaces::mip_chain(decoded, true);
                        image.data = Some(bytes);
                        image.texture_descriptor.mip_level_count = count;
                        if let bevy::image::ImageSampler::Descriptor(sampler) = &mut image.sampler {
                            sampler.address_mode_v = bevy::image::ImageAddressMode::ClampToEdge;
                        }
                    }
                    handle
                })
                .clone(),
        )
    };
    for (index, pack) in packs.packs.iter().enumerate() {
        let applies = pack
            .config
            .scenes
            .iter()
            .any(|scene| scene.stage == "*" || scene.stage.eq_ignore_ascii_case(&stage));
        if !applies {
            continue;
        }
        for (key, style) in &pack.config.weapon_trails {
            let weapon = if key == "default" {
                0
            } else {
                key.parse().unwrap_or(0)
            };
            let handle = style
                .texture
                .as_deref()
                .and_then(|id| texture(index, id, &mut images));
            enhancements.trails.insert(
                weapon,
                ResolvedTrail {
                    style: style.clone(),
                    texture: handle,
                },
            );
        }
        for (key, rule) in &pack.config.effects {
            let textures = rule
                .emitters
                .iter()
                .map(|emitter| {
                    emitter
                        .texture
                        .as_deref()
                        .and_then(|id| texture(index, id, &mut images))
                })
                .collect();
            let slot = enhancements.rules.len();
            enhancements.rules.push(ResolvedRule {
                rule: rule.clone(),
                textures,
            });
            let lower = key.to_ascii_lowercase();
            if lower.contains(['*', '?']) {
                enhancements.patterns.push((lower, slot));
            } else {
                enhancements.exact.insert(lower, slot);
            }
        }
    }
    enhancements.enabled =
        settings_enabled && (!enhancements.trails.is_empty() || !enhancements.rules.is_empty());
    if enhancements.enabled {
        info!(
            "graphics stage {stage}: {} trail styles, {} effect rules",
            enhancements.trails.len(),
            enhancements.rules.len()
        );
    }
}

#[derive(Component)]
pub(crate) struct EffectDepthOwner;

fn sync_depth_prepass(
    mut commands: Commands,
    enhancements: Res<EffectEnhancements>,
    cameras: Query<
        (Entity, Has<DepthPrepass>, Has<EffectDepthOwner>),
        With<crate::plugins::camera_plugin::GameplayCamera>,
    >,
) {
    let wanted = !cfg!(any(target_arch = "wasm32", target_os = "android"))
        && enhancements.wants_depth_prepass();
    for (entity, has_depth, owned) in &cameras {
        if wanted && !has_depth {
            commands
                .entity(entity)
                .insert((DepthPrepass, EffectDepthOwner));
        } else if !wanted && owned {
            commands
                .entity(entity)
                .remove::<DepthPrepass>()
                .remove::<EffectDepthOwner>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_stems_ignore_paths_extension_and_case() {
        assert_eq!(effect_stem("SFX/W02BL112.ef"), "w02bl112");
        assert_eq!(effect_stem("pos431"), "pos431");
        assert_eq!(effect_stem("a\\b\\Pos571.EF"), "pos571");
    }

    #[test]
    fn rules_prefer_exact_then_longest_prefix_then_later_pack() {
        let mut e = EffectEnhancements {
            enabled: true,
            ..default()
        };
        let rule = |intensity: f32| ResolvedRule {
            rule: config::EffectRule {
                intensity,
                ..default()
            },
            textures: vec![],
        };
        e.rules.extend([rule(1.0), rule(2.0), rule(3.0), rule(4.0)]);
        e.exact.insert("w02bl112".into(), 0);
        e.patterns.push(("w02*".into(), 1));
        e.patterns.push(("w??bl*".into(), 2));
        e.patterns.push(("w02bl*".into(), 3));
        assert_eq!(e.rule("SFX/W02BL112.ef").unwrap().rule.intensity, 1.0);
        assert_eq!(e.rule("W02BL113.ef").unwrap().rule.intensity, 4.0);
        assert_eq!(e.rule("W02GD094").unwrap().rule.intensity, 2.0);
        assert_eq!(e.rule("W07BL105").unwrap().rule.intensity, 3.0);
        assert!(e.rule("pos431").is_none());
        e.enabled = false;
        assert!(e.rule("W02BL112").is_none());
    }

    #[test]
    fn glob_matches_stars_and_single_chars() {
        assert!(glob_match("w??bl*", "w02bl112"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("pos4*", "pos431"));
        assert!(!glob_match("pos4*", "pos531"));
        assert!(glob_match("*bl*", "w02bl112"));
        assert!(!glob_match("w?bl*", "w02bl112"));
        assert!(glob_match("abc", "abc"));
        assert!(!glob_match("abc", "abcd"));
    }

    #[test]
    fn combat_fx_sample_pack_loads_and_resolves_for_any_stage() {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let index = br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/combat-fx/pack.json"]}"#;
        let mut graphics = super::super::loader::load_catalog_with_reader(index, |name| {
            super::super::loader::read_bounded(&root, name)
        })
        .unwrap()
        .with_test_loose_root(root.clone());
        graphics.prepare_stage("sn03");
        assert_eq!(graphics.packs.len(), 1);
        let pack = &graphics.packs[0];
        for id in ["trail-soft", "spark", "dust", "impact-flash"] {
            assert!(pack.images.contains_key(id), "{id} loaded for the stage");
        }
        assert_eq!(pack.config.weapon_trails.len(), 3);
        let hit = &pack.config.effects["@hit"];
        assert!(!hit.replace && hit.emitters.len() == 4);
        assert!(pack.config.scenes[0]
            .color_pipeline
            .as_ref()
            .unwrap()
            .bloom
            .is_some());

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<EffectEnhancements>()
            .insert_resource(Settings::default())
            .insert_resource(graphics)
            .insert_resource(LoadedStageInfo {
                stage_id: "sn03".into(),
                ..default()
            })
            .add_systems(Update, resolve);
        app.update();
        let e = app.world().resource::<EffectEnhancements>();
        assert!(e.enabled);
        assert_eq!(e.trail(6).unwrap().0, 6);
        assert_eq!(e.trail(1).unwrap().0, 0);
        assert!(e.trail(6).unwrap().1.texture.is_some());
        assert!(e
            .rule("W05BL112")
            .is_some_and(|r| r.rule.soft && !r.rule.replace));
        assert!(e.rule("pos431").is_some_and(|r| !r.rule.soft));
        assert!(e.rule("@guard").is_some_and(|r| r
            .textures
            .iter()
            .filter(|t| t.is_some())
            .count()
            == 2));
        assert!(e.wants_depth_prepass());
        assert!(e.rule("spdup").is_none());

        app.world_mut().resource_mut::<Settings>().effect_enhance = false;
        app.update();
        assert!(!app.world().resource::<EffectEnhancements>().enabled);
    }

    #[test]
    fn trails_fall_back_to_default() {
        let mut e = EffectEnhancements {
            enabled: true,
            ..default()
        };
        e.trails.insert(
            0,
            ResolvedTrail {
                style: default(),
                texture: None,
            },
        );
        e.trails.insert(
            6,
            ResolvedTrail {
                style: config::WeaponTrail {
                    intensity: 9.0,
                    ..default()
                },
                texture: None,
            },
        );
        assert_eq!(e.trail(6).unwrap().1.style.intensity, 9.0);
        assert_eq!(e.trail(6).unwrap().0, 6);
        assert_eq!(e.trail(8).unwrap().0, 0);
        assert_eq!(
            e.trail(8).unwrap().1.style.intensity,
            config::WeaponTrail::default().intensity
        );
    }
}
