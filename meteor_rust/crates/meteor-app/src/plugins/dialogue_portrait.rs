use super::{
    character_plugin::{FighterBone, FighterModel},
    stage_plugin::{LocalFighter, MissionNpc},
    ui_plugin::{DialoguePresentation, DialogueState},
    ui_theme as theme,
};
use crate::{
    app_state::{LaunchRequest, PlayingRoot},
    components::fighter::{Fighter, MotionState},
};
use bevy::camera::{
    visibility::{RenderLayers, VisibilitySystems},
    RenderTarget,
};
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

const PORTRAIT_LAYER: usize = 31;
#[derive(Component)]
pub(crate) struct PortraitWindow;
#[derive(Component)]
struct PortraitCamera;
#[derive(Component)]
struct PortraitLayer;

pub(crate) struct DialoguePortraitPlugin;
impl Plugin for DialoguePortraitPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, setup).add_systems(
            PostUpdate,
            track_head
                .after(bevy::transform::TransformSystems::Propagate)
                .before(VisibilitySystems::UpdateFrusta)
                .before(VisibilitySystems::CheckVisibility),
        );
    }
}
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    windows: Query<Entity, (With<PortraitWindow>, Without<ImageNode>)>,
) {
    for window in &windows {
        let target = images.add(Image::new_target_texture(
            192,
            192,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        commands
            .entity(window)
            .insert(ImageNode::new(target.clone()));
        commands.spawn((
            Camera3d::default(),
            Camera {
                order: -1,
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(target.into()),
            Projection::Perspective(PerspectiveProjection {
                fov: 32.0_f32.to_radians(),
                near: 0.05,
                far: 12.0,
                ..default()
            }),
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Msaa::Off,
            RenderLayers::layer(PORTRAIT_LAYER),
            Transform::default(),
            PortraitCamera,
            PlayingRoot,
        ));
    }
}

pub(crate) fn portrait_transform(head: Vec3, facing: f32) -> Transform {
    let target = head + Vec3::Y * 0.12;
    let front = Vec3::new(facing.sin(), 0.0, facing.cos());
    Transform::from_translation(target + front * 1.35 + Vec3::Y * 0.02).looking_at(target, Vec3::Y)
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn track_head(
    mut commands: Commands,
    view: Option<Res<DialoguePresentation>>,
    dialogue: Option<Res<DialogueState>>,
    request: Option<Res<LaunchRequest>>,
    state: Option<Res<State<crate::app_state::AppState>>>,
    scale: Option<Res<super::ui_scale_plugin::EffectiveUiScale>>,
    fighters: Query<(
        Entity,
        &Fighter,
        Option<&MissionNpc>,
        Has<LocalFighter>,
        &MotionState,
        Option<&Visibility>,
    )>,
    models: Query<(Entity, &FighterModel)>,
    bones: Query<(&FighterBone, &GlobalTransform), Without<PortraitCamera>>,
    skins: Query<(Entity, &ChildOf, Option<&RenderLayers>, Has<PortraitLayer>), With<SkinnedMesh>>,
    mut cameras: Query<(&mut Camera, &mut Transform, &mut GlobalTransform), With<PortraitCamera>>,
    mut windows: Query<&mut Node, With<PortraitWindow>>,
) {
    let speaker = view.as_ref().and_then(|v| v.speaker());
    let active = dialogue.is_some_and(|d| d.text.is_some() && d.timer > 0.0)
        && state.is_none_or(|s| *s.get() == crate::app_state::AppState::Playing);
    let mut candidates = fighters
        .iter()
        .filter(|(_, fighter, npc, local, _, visibility)| {
            active
                && !visibility.is_some_and(|v| *v == Visibility::Hidden)
                && speaker.is_some_and(|name| {
                    npc.is_some_and(|npc| npc.name == name)
                        || (*local
                            && (request.as_ref().is_some_and(|r| r.player_name == name)
                                || super::ui_plugin::character_label(fighter.character_id)
                                    .split_once(' ')
                                    .is_some_and(|(_, label)| label == name)))
                })
        });
    let first = candidates.next();

    let selected =
        first
            .filter(|_| candidates.next().is_none())
            .and_then(|(fighter, _, _, _, motion, _)| {
                let (model, _) = models.iter().find(|(_, m)| m.fighter == fighter)?;
                let (_, head) = bones
                    .iter()
                    .find(|(b, _)| b.model == model && b.name.eq_ignore_ascii_case("bau_Head"))?;
                let position = head.translation();
                position
                    .is_finite()
                    .then_some((model, portrait_transform(position, motion.facing)))
            });
    for (entity, parent, layers, tagged) in &skins {
        let chosen = selected
            .as_ref()
            .is_some_and(|(model, _)| parent.parent() == *model);
        if chosen && !tagged {
            commands.entity(entity).insert((
                layers.cloned().unwrap_or_default().with(PORTRAIT_LAYER),
                PortraitLayer,
            ));
        } else if !chosen && tagged {
            commands
                .entity(entity)
                .insert(layers.cloned().unwrap_or_default().without(PORTRAIT_LAYER))
                .remove::<PortraitLayer>();
        }
    }
    for (mut camera, mut transform, mut global) in &mut cameras {
        camera.is_active = selected.is_some();
        if let Some((_, shot)) = &selected {
            *transform = *shot;
            *global = GlobalTransform::from(*shot);
        }
    }
    let unit = scale.as_ref().map_or(1.0, |s| s.0.max(0.05));
    for mut node in &mut windows {
        node.display = if selected.is_some() {
            Display::Flex
        } else {
            Display::None
        };
        node.width = Val::Px(72.0 / unit);
        node.height = node.width;
    }
}
pub(crate) fn window_node() -> impl Bundle {
    (
        Node {
            display: Display::None,
            width: Val::Px(72.0),
            height: Val::Px(72.0),
            flex_shrink: 0.0,
            overflow: Overflow::clip(),
            border_radius: BorderRadius::all(Val::Px(8.0)),
            ..default()
        },
        BackgroundColor(theme::BACKGROUND.with_alpha(0.35)),
        PortraitWindow,
    )
}
