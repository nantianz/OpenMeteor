use super::{launcher_ui::UiFont, ui_theme as theme};
use crate::app_state::{AppState, LaunchRequest, LoadingRoot};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::{Shader, ShaderRef};

const FOG_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("46b2f013-2ef8-4cdc-8970-3658f11d0013");
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(crate) struct FogMaterial {
    #[uniform(0)]
    params: Vec4,
}
impl UiMaterial for FogMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(FOG_SHADER)
    }
}
#[derive(Resource, Default)]
pub(crate) struct LoadingPresentation {
    pub active: bool,
    age: f32,
    frames: u32,
    scene_frames: u32,
    ready_frames: u32,
    reveal: f32,
}
impl LoadingPresentation {
    pub fn can_enter(&self) -> bool {
        self.frames >= 3 && self.age >= 0.65
    }
}
#[derive(Component)]
struct LoadingCurtain;
#[derive(Component)]
struct LoadingCaption;
#[derive(Component)]
struct LoadingCopy;

pub(crate) fn input_ready(state: Option<Res<LoadingPresentation>>) -> bool {
    state.is_none_or(|state| !state.active)
}

pub(crate) struct LoadingUiPlugin;
impl Plugin for LoadingUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiMaterialPlugin::<FogMaterial>::default())
            .init_resource::<LoadingPresentation>();
        bevy::asset::load_internal_asset!(
            app,
            FOG_SHADER,
            "ui_loading_fog.wgsl",
            Shader::from_wgsl
        );
        app.add_systems(OnEnter(AppState::Loading), setup)
            .add_systems(
                Update,
                animate.after(crate::plugins::FighterFrameSet::PoseResolve),
            )
            .add_systems(OnEnter(AppState::Menu), cleanup)
            .add_systems(OnEnter(AppState::Error), cleanup);
    }
}
pub(crate) fn setup(
    mut commands: Commands,
    mut state: ResMut<LoadingPresentation>,
    mut materials: ResMut<Assets<FogMaterial>>,
    font: Res<UiFont>,
    request: Res<LaunchRequest>,
    collider: Option<ResMut<super::stage_collider::StageCollider>>,
) {
    if let Some(mut collider) = collider {
        collider.mesh = None;
    }
    *state = LoadingPresentation {
        active: true,
        ..default()
    };
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            MaterialNode(materials.add(FogMaterial {
                params: Vec4::new(0.0, 1.0, 1.7778, 0.0),
            })),
            LoadingRoot,
            LoadingCurtain,
            GlobalZIndex(1000),
            Interaction::default(),
            bevy::ui::FocusPolicy::Block,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(16.0),
                    width: Val::Percent(80.0),
                    max_width: Val::Px(420.0),
                    ..default()
                },
                LoadingCopy,
            ))
            .with_children(|copy| {
                copy.spawn(theme::title_node(false));
                copy.spawn((
                    Text::new(request.stage_name.clone()),
                    TextFont {
                        font: font.0.clone().into(),
                        font_size: 20.0.into(),
                        ..default()
                    },
                    TextColor(theme::TEXT),
                ));
                copy.spawn((
                    Node {
                        width: Val::Px(90.0),
                        height: Val::Px(1.0),
                        ..default()
                    },
                    BackgroundColor(theme::ACCENT),
                ));
                copy.spawn((
                    Text::new("正在准备山川与刀剑"),
                    TextFont {
                        font: font.0.clone().into(),
                        font_size: 13.0.into(),
                        ..default()
                    },
                    TextColor(theme::MUTED),
                    LoadingCaption,
                ));
            });
        });
}
fn animate(
    time: Res<Time<Real>>,
    app_state: Res<State<AppState>>,
    mut state: ResMut<LoadingPresentation>,
    mut materials: ResMut<Assets<FogMaterial>>,
    windows: Query<&Window>,
    mut captions: Query<&mut Text, With<LoadingCaption>>,
    mut copies: Query<&mut Visibility, With<LoadingCopy>>,
    mut commands: Commands,
    roots: Query<Entity, With<LoadingCurtain>>,
    collider: Option<Res<super::stage_collider::StageCollider>>,
    mut next: ResMut<NextState<AppState>>,
    info: Option<ResMut<crate::app_state::LoadedStageInfo>>,
    characters: super::character_plugin::CharacterLoadingState,
) {
    if !state.active {
        return;
    }
    state.age += time.delta_secs().min(0.1);
    state.frames += 1;
    if *app_state.get() == AppState::Playing {
        state.scene_frames += 1;
        if let Some(error) = characters.error() {
            if let Some(mut info) = info {
                info.ready = false;
                info.error = Some(error);
            }
            next.set(AppState::Error);
            return;
        }
        if state.scene_frames > 4
            && collider
                .is_some_and(|c| c.mesh.as_ref().is_none_or(|mesh| mesh.positions.is_empty()))
        {
            if let Some(mut info) = info {
                info.ready = false;
                info.error = Some("场景网格初始化失败，请返回后重试".into());
            }
            next.set(AppState::Error);
            return;
        }

        let poses_ready = characters.ready();
        state.ready_frames = if poses_ready {
            state.ready_frames + 1
        } else {
            0
        };
        if state.ready_frames > 4 && state.can_enter() {
            state.reveal += time.delta_secs().min(0.05) / 0.65;
        }
        for mut caption in &mut captions {
            caption.0 = if poses_ready {
                "雾散江湖 · 即将入场"
            } else {
                "正在准备人物与招式"
            }
            .into();
        }
    }
    for mut copy in &mut copies {
        *copy = if state.reveal > 0.05 {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    let aspect = windows
        .single()
        .map_or(1.7778, |w| w.width() / w.height().max(1.0));
    for (_, material) in materials.iter_mut() {
        material.params = Vec4::new(
            state.age,
            (1.0 - state.reveal).clamp(0.0, 1.0),
            aspect,
            state.reveal,
        );
    }
    if state.reveal >= 1.0 {
        state.active = false;
        for root in &roots {
            commands.entity(root).despawn();
        }
    }
}
fn cleanup(
    mut commands: Commands,
    roots: Query<Entity, With<LoadingCurtain>>,
    mut state: ResMut<LoadingPresentation>,
) {
    state.active = false;
    for root in &roots {
        commands.entity(root).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fog_fragment_validates_with_ui_vertex_layout() {
        let source = include_str!("ui_loading_fog.wgsl").replace(
            "#import bevy_ui::ui_vertex_output::UiVertexOutput",
            "struct UiVertexOutput {
                @location(0) uv: vec2<f32>,
                @location(1) border_widths: vec4<f32>,
                @location(2) border_radius: vec4<f32>,
                @location(3) @interpolate(flat) size: vec2<f32>,
                @builtin(position) position: vec4<f32>,
            };",
        );
        let module = naga::front::wgsl::parse_str(&source).expect("fog shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("fog shader validates with the Bevy UI inputs");
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn curtain_waits_for_loading_then_reveals_and_cleans_up_on_error() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::state::app::StatesPlugin,
        ))
        .init_state::<AppState>()
        .init_asset::<Image>()
        .init_asset::<Font>()
        .insert_resource(UiFont(Handle::default()))
        .init_resource::<LaunchRequest>()
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(100),
        ))
        .add_plugins((theme::CinematicThemePlugin, LoadingUiPlugin));
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Loading);
        app.update();
        assert!(!app.world().resource::<LoadingPresentation>().can_enter());
        for _ in 0..10 {
            app.update();
        }
        assert!(app.world().resource::<LoadingPresentation>().can_enter());
        assert!(app.world().resource::<LoadingPresentation>().active);
        let fog_time = app
            .world()
            .resource::<Assets<FogMaterial>>()
            .iter()
            .next()
            .unwrap()
            .1
            .params
            .x;
        app.update();
        assert!(
            app.world()
                .resource::<Assets<FogMaterial>>()
                .iter()
                .next()
                .unwrap()
                .1
                .params
                .x
                > fog_time
        );
        assert_eq!(app.world().resource::<Time<Virtual>>().delta_secs(), 0.0);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Playing);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(app.world().resource::<LoadingPresentation>().reveal, 0.0);
        for _ in 0..20 {
            app.update();
        }
        assert_eq!(app.world().resource::<LoadingPresentation>().reveal, 0.0);
        let fighter = app
            .world_mut()
            .spawn(crate::components::fighter::Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            })
            .id();
        let model = app
            .world_mut()
            .spawn(super::super::character_plugin::FighterModel {
                fighter,
                character: "p0".into(),
            })
            .id();
        app.world_mut()
            .insert_resource(super::super::character_plugin::CommonAmb({
                let mut bytes = vec![0u8; meteor_asset::amb::BANIM_HEADER_SIZE];
                bytes[..5].copy_from_slice(b"BANIM");
                bytes[5..9].copy_from_slice(&1u32.to_le_bytes());
                meteor_asset::amb::parse_amb(&bytes).unwrap()
            }));
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(app.world().resource::<LoadingPresentation>().reveal, 0.0);
        app.world_mut()
            .entity_mut(model)
            .insert(super::super::character_plugin::FighterPoseSampled);
        let skin = app
            .world_mut()
            .spawn(super::super::character_plugin::PendingFighterSkinReveal)
            .id();
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(app.world().resource::<LoadingPresentation>().reveal, 0.0);
        app.world_mut().despawn(skin);
        for _ in 0..20 {
            app.update();
        }
        assert!(!app.world().resource::<LoadingPresentation>().active);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<LoadingCurtain>>()
                .iter(app.world())
                .count(),
            0
        );
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Loading);
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Error);
        app.update();
        assert!(!app.world().resource::<LoadingPresentation>().active);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<LoadingCurtain>>()
                .iter(app.world())
                .count(),
            0
        );
    }
}
