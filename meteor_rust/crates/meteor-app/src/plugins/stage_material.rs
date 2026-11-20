#![allow(dead_code)]

mod dynamic_uniforms;
pub(super) use dynamic_uniforms::SharedBuffers;
pub(crate) mod scene_normals;
pub(crate) mod scene_textures;
pub(crate) mod water;
pub(crate) use dynamic_uniforms::{split_time, FrameUniforms};

use bevy::material::AlphaMode;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Face,
    RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderDefVal, ShaderRef};

fn is_prepass_pipeline(descriptor: &RenderPipelineDescriptor) -> bool {
    descriptor
        .vertex
        .shader_defs
        .contains(&ShaderDefVal::Bool("PREPASS_PIPELINE".into(), true))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GmdlMode {
    Opaque,

    Blend,

    Add,

    MultiplyShadow,

    SrcColorOne,
}

impl GmdlMode {
    pub fn as_u32(self) -> u32 {
        match self {
            GmdlMode::Opaque => 0,
            GmdlMode::Blend => 1,
            GmdlMode::Add => 2,
            GmdlMode::MultiplyShadow => 3,
            GmdlMode::SrcColorOne => 4,
        }
    }
}

bitflags::bitflags! {

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct GmdlMaterialKey: u32 {

        const TWO_SIDE = 1 << 0;

        const BLEND_GROUP = 1 << 1;
        const SRC_COLOR_ONE = 1 << 2;
        const SRC_ALPHA_ONE = 1 << 3;
        const SRC_ALPHA_BLEND = 1 << 4;
        const WATER_SKY = 1 << 5;
    }
}

#[derive(ShaderType, Clone)]
pub struct GmdlMaterialParams {
    pub opacity: f32,

    pub alpha_cutoff: f32,

    pub mode: u32,

    pub has_texture: u32,

    pub uv_offset: Vec2,
}

#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraOcclusionParams {
    pub eye: Vec4,
    pub player: Vec4,
    pub watched: Vec4,

    pub self_eye: Vec4,
}

#[derive(Asset, TypePath, Clone)]
pub struct GmdlMaterial {
    pub params: GmdlMaterialParams,
    pub texture: Option<Handle<Image>>,

    pub normal_texture: Option<Handle<Image>>,
    pub orm_texture: Option<Handle<Image>>,
    pub env_diffuse: Option<Handle<Image>>,
    pub env_specular: Option<Handle<Image>>,

    pub pbr: Vec4,

    pub pbr_tint: Vec4,

    pub pbr_textures: Vec4,

    pub(crate) pbr_lighting: Vec3,

    pub normal_strength: f32,
    pub normal_contrast: f32,

    pub uv_scroll: Vec4,

    pub water: Vec4,
    pub(crate) water_optics: WaterOptics,
    pub(crate) grass_ground: Vec4,
    pub(crate) grass_bounds: Vec4,
    pub(crate) grass_coverage: Option<Handle<Image>>,
    pub(crate) grass_underlay: Option<Handle<Image>>,
    pub(crate) grass_underlay_scale: f32,

    pub water_bounds: Vec4,
    pub water_reflection: Option<Handle<Image>>,
    pub(crate) water_sky_visibility: Option<Handle<Image>>,
    pub water_depth: Option<Handle<Image>>,
    pub water_normal: Option<Handle<Image>>,
    pub water_dynamics: Option<Handle<Image>>,
    pub water_dynamics_bounds: Vec4,

    pub water_live_reflection: bool,
    pub(crate) water_screen_refraction: bool,
    pub(crate) water_sky_reflection: bool,
    pub(crate) water_sky_style: Vec4,
    pub(crate) water_sky_ranges: Vec4,

    pub body_glow: Vec4,

    pub body_style: Vec4,

    pub first_person_clip: Vec4,

    pub camera_occlusion_enabled: bool,

    pub weapon_refraction_source: bool,

    pub two_side: bool,

    pub mode: GmdlMode,
}

impl GmdlMaterial {
    pub fn new(
        params: GmdlMaterialParams,
        texture: Option<Handle<Image>>,
        two_side: bool,
        mode: GmdlMode,
    ) -> Self {
        Self {
            params,
            texture,
            normal_texture: None,
            orm_texture: None,
            env_diffuse: None,
            env_specular: None,
            pbr: Vec4::ZERO,
            pbr_tint: Vec4::ZERO,
            pbr_textures: Vec4::ZERO,
            pbr_lighting: Vec3::ONE,
            normal_strength: 1.0,
            normal_contrast: 0.8,
            uv_scroll: Vec4::ZERO,
            water: Vec4::ZERO,
            water_optics: WaterOptics::default(),
            grass_ground: Vec4::ZERO,
            grass_bounds: Vec4::ZERO,
            grass_coverage: None,
            grass_underlay: None,
            grass_underlay_scale: 0.0,
            water_bounds: Vec4::ZERO,
            water_reflection: None,
            water_sky_visibility: None,
            water_depth: None,
            water_normal: None,
            water_dynamics: None,
            water_dynamics_bounds: Vec4::ZERO,
            water_live_reflection: false,
            water_screen_refraction: false,
            water_sky_reflection: false,
            water_sky_style: Vec4::ZERO,
            water_sky_ranges: Vec4::ZERO,
            body_glow: Vec4::ZERO,
            body_style: Vec4::ZERO,
            first_person_clip: Vec4::ZERO,
            camera_occlusion_enabled: false,
            weapon_refraction_source: false,
            two_side,
            mode,
        }
    }

    pub fn new_scene(
        params: GmdlMaterialParams,
        texture: Option<Handle<Image>>,
        two_side: bool,
        mode: GmdlMode,
    ) -> Self {
        Self {
            camera_occlusion_enabled: true,
            pbr_textures: Vec4::new(0.0, 0.0, 0.0, 1.0),
            ..Self::new(params, texture, two_side, mode)
        }
    }

    fn key(&self) -> GmdlMaterialKey {
        let mut key = GmdlMaterialKey::empty();
        if self.water_sky_reflection && !cfg!(any(target_arch = "wasm32", target_os = "android")) {
            key |= GmdlMaterialKey::WATER_SKY;
        }
        if self.two_side {
            key |= GmdlMaterialKey::TWO_SIDE;
        }
        if self.mode != GmdlMode::Opaque {
            key |= GmdlMaterialKey::BLEND_GROUP;
        }
        match self.mode {
            GmdlMode::Blend => key |= GmdlMaterialKey::SRC_ALPHA_BLEND,
            GmdlMode::SrcColorOne => key |= GmdlMaterialKey::SRC_COLOR_ONE,
            GmdlMode::Add => key |= GmdlMaterialKey::SRC_ALPHA_ONE,
            _ => {}
        }
        key
    }
}

impl From<&GmdlMaterial> for GmdlMaterialKey {
    fn from(material: &GmdlMaterial) -> Self {
        material.key()
    }
}

impl GmdlMaterialKey {
    fn additive_blend(self) -> Option<BlendState> {
        let src_factor = if self.contains(Self::SRC_COLOR_ONE) {
            BlendFactor::Src
        } else if self.contains(Self::SRC_ALPHA_ONE) {
            BlendFactor::SrcAlpha
        } else {
            return None;
        };
        let component = BlendComponent {
            src_factor,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        };
        Some(BlendState {
            color: component,
            alpha: component,
        })
    }
}

const GMDL_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("00000000-0000-0000-0000-9d2a1b3c4d5e");

const GMDL_PREPASS_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("c12c7fa4-5bdf-4c0b-9c44-41f1eac54d27");

#[derive(ShaderType, Clone, Copy, Default)]
pub(crate) struct WaterOptics {
    pub absorption_ior: Vec4,
    pub scattering_g: Vec4,
    pub control: Vec4,
}

impl Material for GmdlMaterial {
    fn prepass_fragment_shader() -> ShaderRef {
        ShaderRef::Handle(GMDL_PREPASS_SHADER_HANDLE)
    }

    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(GMDL_SHADER_HANDLE)
    }

    fn reads_view_transmission_texture(&self) -> bool {
        self.weapon_refraction_source || self.water_live_reflection || self.water_screen_refraction
    }

    fn depth_bias(&self) -> f32 {
        if self.weapon_refraction_source {
            -10_000.0
        } else {
            0.0
        }
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.reads_view_transmission_texture() {
            return AlphaMode::Opaque;
        }
        match self.mode {
            GmdlMode::Blend => AlphaMode::Blend,
            GmdlMode::Add | GmdlMode::SrcColorOne => AlphaMode::Add,
            GmdlMode::MultiplyShadow => AlphaMode::Multiply,
            GmdlMode::Opaque if self.params.alpha_cutoff > 0.0 => {
                AlphaMode::Mask(self.params.alpha_cutoff)
            }
            GmdlMode::Opaque => AlphaMode::Opaque,
        }
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = if key.bind_group_data.contains(GmdlMaterialKey::TWO_SIDE)
        {
            None
        } else {
            Some(Face::Back)
        };

        if is_prepass_pipeline(descriptor) {
            return Ok(());
        }
        if let Some(fragment) = &mut descriptor.fragment {
            if fragment.targets.iter().flatten().any(|target| {
                target.format == bevy::render::render_resource::TextureFormat::Rgba16Float
            }) {
                fragment.shader_defs.push("METEOR_HDR_TARGET".into());
            }
        }
        if key.bind_group_data.contains(GmdlMaterialKey::WATER_SKY) {
            if let Some(fragment) = &mut descriptor.fragment {
                fragment.shader_defs.push("WATER_SKY_REFLECTION".into());
            }
        }
        if let Some(depth_stencil) = &mut descriptor.depth_stencil {
            depth_stencil.depth_write_enabled =
                Some(!key.bind_group_data.contains(GmdlMaterialKey::BLEND_GROUP));
        }
        if let Some(blend) = key.bind_group_data.additive_blend() {
            if let Some(fragment) = &mut descriptor.fragment {
                if let Some(Some(target)) = fragment.targets.get_mut(0) {
                    target.blend = Some(blend);
                }
            }
        } else if key
            .bind_group_data
            .contains(GmdlMaterialKey::SRC_ALPHA_BLEND)
        {
            if let Some(fragment) = &mut descriptor.fragment {
                if let Some(Some(target)) = fragment.targets.get_mut(0) {
                    target.blend = Some(BlendState::ALPHA_BLENDING);
                }
            }
        }
        Ok(())
    }
}

pub struct GmdlMaterialPlugin;

impl Plugin for GmdlMaterialPlugin {
    fn build(&self, app: &mut App) {
        super::graphics_pack::sky::register_panorama_shader(app);
        water::install(app);
        dynamic_uniforms::install(app);
        water::interaction::install(app);
        water::screen_reflection::install(app);
        app.add_systems(Update, scene_normals::sync_normal_setting);
        app.add_plugins(MaterialPlugin::<GmdlMaterial>::default())
            .add_systems(
                Update,
                update_camera_occlusion.after(crate::plugins::FighterFrameSet::CameraFinalize),
            );
        bevy::asset::load_internal_asset!(
            app,
            GMDL_PREPASS_SHADER_HANDLE,
            "gmdl_prepass.wgsl",
            Shader::from_wgsl
        );
        bevy::asset::load_internal_asset!(
            app,
            GMDL_SHADER_HANDLE,
            "gmdl_material.wgsl",
            Shader::from_wgsl
        );
    }
}

fn update_camera_occlusion(
    state: Res<State<crate::app_state::AppState>>,
    time: Res<Time>,
    lock: Option<Res<crate::components::fighter::LockOnController>>,
    mode: Option<Res<crate::plugins::camera_plugin::CameraMode>>,
    cameras: Query<
        (
            &Transform,
            Option<&crate::plugins::camera_plugin::ThirdPersonState>,
        ),
        With<crate::plugins::camera_plugin::GameplayCamera>,
    >,
    players: Query<&Transform, With<crate::plugins::stage_plugin::LocalFighter>>,
    targets: Query<&Transform>,
    mut frame: ResMut<FrameUniforms>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(5);
    use crate::app_state::AppState;
    let fade = &mut frame.camera;
    let context = cameras.single().ok().zip(players.single().ok());
    let in_scene = matches!(state.get(), AppState::Playing | AppState::Paused)
        && mode.is_none_or(|mode| !mode.fly && !mode.freecam_hold);
    if let Some(((eye, camera_state), player)) = context.filter(|_| in_scene) {
        let target = lock
            .and_then(|lock| lock.lock_entity)
            .and_then(|entity| targets.get(entity).ok());
        let free_fade = camera_state.is_some_and(|camera| camera.scene_fade_required);
        let desired = if target.is_some() || free_fade {
            0.8
        } else {
            0.0
        };
        let rate = if desired > fade.eye.w { 16.0 } else { 8.0 };
        let mut strength = fade.eye.w
            + (desired - fade.eye.w) * (1.0 - (-rate * time.delta_secs().clamp(0.0, 0.1)).exp());

        if free_fade {
            strength = desired;
        }
        fade.eye = eye
            .translation
            .extend(if strength < 0.001 { 0.0 } else { strength });
        fade.player =
            (player.translation + Vec3::Y * 2.5).extend(if free_fade { 3.4 } else { 2.4 });
        if let Some(target) = target {
            fade.watched = (target.translation + Vec3::Y * 2.5).extend(2.4);
        } else if free_fade {
            fade.watched = Vec4::ZERO;
        }
    } else {
        *fade = CameraOcclusionParams::default();
    }
    if fade.eye.w == 0.0 {
        *fade = CameraOcclusionParams::default();
    }

    fade.self_eye = cameras
        .single()
        .ok()
        .filter(|_| in_scene)
        .and_then(|(eye, camera)| {
            camera
                .filter(|c| c.is_first_person())
                .map(|_| eye.translation.extend(1.0))
        })
        .unwrap_or(Vec4::ZERO);
}

#[cfg(test)]
pub(crate) fn install_camera_uniforms_for_test(app: &mut App) {
    app.init_resource::<FrameUniforms>().add_systems(
        Update,
        update_camera_occlusion.after(crate::plugins::FighterFrameSet::CameraFinalize),
    );
}

#[cfg(test)]
pub(super) fn standalone_shader_source(colors: bool) -> String {
    standalone_shader_variant(colors, true)
}

#[cfg(test)]
fn standalone_shader_variant(colors: bool, tangents: bool) -> String {
    let mut active = vec![true];
    let mut source = String::from("struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) world_normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(4) world_tangent: vec4<f32>, @location(5) color: vec4<f32>, };\nstruct TestView { world_position: vec3<f32>, }; @group(0) @binding(0) var<uniform> view: TestView;\n");
    for line in include_str!("gmdl_material.wgsl").lines() {
        if line.starts_with("#import") {
            continue;
        }
        if let Some(flag) = line.strip_prefix("#ifdef ") {
            active.push(
                *active.last().unwrap()
                    && ((flag == "VERTEX_COLORS" && colors)
                        || (flag == "VERTEX_TANGENTS" && tangents)),
            );
        } else if line.starts_with("#else") {
            let last = active.pop().unwrap();
            active.push(*active.last().unwrap() && !last);
        } else if line.starts_with("#endif") {
            active.pop();
        } else if *active.last().unwrap() {
            source.push_str(line);
            source.push('\n');
        }
    }
    source.replace("#{MATERIAL_BIND_GROUP}", "3")
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    fn test_material(mode: GmdlMode) -> GmdlMaterial {
        GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: mode.as_u32(),
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            None,
            true,
            mode,
        )
    }

    #[test]
    fn live_water_enters_transmissive_phase_with_alpha_blending() {
        let mut water = test_material(GmdlMode::Blend);
        assert_eq!(water.alpha_mode(), AlphaMode::Blend);
        water.water_live_reflection = true;
        assert_eq!(water.alpha_mode(), AlphaMode::Opaque);
        assert!(water.reads_view_transmission_texture());
        assert!(water.key().contains(GmdlMaterialKey::SRC_ALPHA_BLEND));
    }

    #[test]
    fn additive_modes_have_distinct_pipeline_keys_and_authored_factors() {
        let src_color = test_material(GmdlMode::SrcColorOne).key();
        let src_alpha = test_material(GmdlMode::Add).key();
        let blend = test_material(GmdlMode::Blend).key();
        assert_ne!(src_color, src_alpha);
        assert_ne!(src_alpha, blend);
        for (key, factor) in [
            (src_color, BlendFactor::Src),
            (src_alpha, BlendFactor::SrcAlpha),
        ] {
            assert!(key.contains(GmdlMaterialKey::BLEND_GROUP));
            let state = key.additive_blend().unwrap();
            assert_eq!(state.color.src_factor, factor);
            assert_eq!(state.color.dst_factor, BlendFactor::One);
            assert_eq!(state.color.operation, BlendOperation::Add);
        }
        for mode in [GmdlMode::Opaque, GmdlMode::Blend, GmdlMode::MultiplyShadow] {
            assert!(test_material(mode).key().additive_blend().is_none());
        }
    }

    #[test]
    fn original_christmas_stars_preserve_background_at_black_texels() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for name in ["start001", "start002", "start003"] {
            let model = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("cmodel/{name}.gmb"))).unwrap(),
            )
            .unwrap();
            assert!(!model.materials.is_empty());
            for authored in &model.materials {
                let (mode, _, _) = crate::plugins::render_plugin::classify_gmdl_material(authored);
                assert_eq!(mode, GmdlMode::SrcColorOne);
                let state = test_material(mode).key().additive_blend().unwrap();

                assert_eq!(state.color.src_factor, BlendFactor::Src);
                assert_eq!(state.color.dst_factor, BlendFactor::One);
                for (source, background) in [(0.0_f32, 0.35_f32), (0.25, 0.35)] {
                    let result = source * source + background;
                    assert!(result >= background);
                    if source == 0.0 {
                        assert_eq!(result, background);
                    }
                }
            }
        }
    }

    #[test]
    fn camera_fade_shader_validates_with_and_without_vertex_colors() {
        for (colors, tangents) in [(false, false), (true, false), (false, true), (true, true)] {
            let source = standalone_shader_variant(colors, tangents);
            let module = naga::front::wgsl::parse_str(&source).expect("GMDL WGSL parses");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("GMDL camera fade validates");
        }
    }

    #[test]
    fn production_camera_fade_restores_materials_on_unlock_and_scene_exit() {
        use crate::app_state::AppState;
        use crate::components::fighter::LockOnController;
        use crate::plugins::camera_plugin::{CameraMode, GameplayCamera};
        use crate::plugins::stage_plugin::LocalFighter;
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(State::new(AppState::Playing))
            .init_resource::<CameraMode>()
            .init_resource::<LockOnController>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<FrameUniforms>()
            .add_systems(Update, update_camera_occlusion);
        let camera = app
            .world_mut()
            .spawn((GameplayCamera, Transform::from_xyz(0.0, 4.0, -10.0)))
            .id();
        app.world_mut().spawn((LocalFighter, Transform::default()));
        let target = app
            .world_mut()
            .spawn(Transform::from_xyz(0.0, 0.0, 3.0))
            .id();
        app.world_mut()
            .resource_mut::<LockOnController>()
            .lock_entity = Some(target);
        let handles: Vec<_> = [GmdlMode::Opaque, GmdlMode::Blend, GmdlMode::MultiplyShadow]
            .into_iter()
            .map(|mode| {
                app.world_mut()
                    .resource_mut::<Assets<GmdlMaterial>>()
                    .add(GmdlMaterial::new_scene(
                        GmdlMaterialParams {
                            opacity: 0.7,
                            alpha_cutoff: 0.5,
                            mode: mode.as_u32(),
                            has_texture: 0,
                            uv_offset: Vec2::ZERO,
                        },
                        None,
                        true,
                        mode,
                    ))
            })
            .collect();
        app.update();
        let materials = app.world().resource::<Assets<GmdlMaterial>>();
        for handle in &handles {
            let material = materials.get(handle).unwrap();
            let fade = app.world().resource::<FrameUniforms>().camera;
            assert!(fade.eye.w > 0.0 && fade.eye.w < 0.8);
            assert_eq!(material.params.opacity, 0.7);
            assert_eq!(material.params.alpha_cutoff, 0.5);
            assert_eq!(material.params.mode, material.mode.as_u32());
        }
        app.world_mut()
            .resource_mut::<LockOnController>()
            .lock_entity = None;
        for _ in 0..25 {
            app.update();
        }
        assert_eq!(app.world().resource::<FrameUniforms>().camera.eye.w, 0.0);
        let mut camera_state = crate::plugins::camera_plugin::ThirdPersonState::default();
        camera_state.scene_fade_required = true;
        app.world_mut().entity_mut(camera).insert(camera_state);
        app.update();
        let params = app.world().resource::<FrameUniforms>().camera;
        assert_eq!(
            params.eye.w, 0.8,
            "unlocked cramped view must reveal the player immediately"
        );
        assert_eq!(
            params.watched,
            Vec4::ZERO,
            "do not reveal a stale lock target"
        );
        assert_eq!(
            params.player.w, 3.4,
            "include the space around attack animations"
        );
        app.world_mut()
            .get_mut::<crate::plugins::camera_plugin::ThirdPersonState>(camera)
            .unwrap()
            .scene_fade_required = false;
        app.update();
        let strength = app.world().resource::<FrameUniforms>().camera.eye.w;
        assert!(strength > 0.0 && strength < 0.8, "recover opacity smoothly");
        app.world_mut()
            .resource_mut::<LockOnController>()
            .lock_entity = Some(target);
        app.update();
        app.world_mut().resource_mut::<CameraMode>().fly = true;
        app.update();
        assert!(app.world().resource::<FrameUniforms>().camera == CameraOcclusionParams::default());
        app.world_mut().resource_mut::<CameraMode>().fly = false;
        app.update();
        app.world_mut().despawn(target);
        for _ in 0..25 {
            app.update();
        }
        assert_eq!(app.world().resource::<FrameUniforms>().camera.eye.w, 0.0);
        app.insert_resource(State::new(AppState::Loading));
        app.update();
        for _handle in &handles {
            assert!(
                app.world().resource::<FrameUniforms>().camera == CameraOcclusionParams::default()
            );
        }
    }

    #[test]
    fn camera_fade_excludes_characters_weapons_and_effects() {
        use crate::app_state::AppState;
        use crate::components::fighter::LockOnController;
        use crate::plugins::camera_plugin::{GameplayCamera, ThirdPersonState};
        use crate::plugins::stage_plugin::LocalFighter;

        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(State::new(AppState::Playing))
            .init_resource::<LockOnController>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<FrameUniforms>()
            .add_systems(Update, update_camera_occlusion);
        let camera = app
            .world_mut()
            .spawn((
                GameplayCamera,
                ThirdPersonState::default(),
                Transform::from_xyz(0.0, 4.0, -10.0),
            ))
            .id();
        app.world_mut().spawn((LocalFighter, Transform::default()));
        let target = app
            .world_mut()
            .spawn(Transform::from_xyz(0.0, 0.0, 3.0))
            .id();
        let mut excluded = Vec::new();
        let scene;
        {
            let mut materials = app.world_mut().resource_mut::<Assets<GmdlMaterial>>();
            for mode in [GmdlMode::Opaque, GmdlMode::Blend, GmdlMode::Add] {
                let material = GmdlMaterial::new(
                    GmdlMaterialParams {
                        opacity: 0.7,
                        alpha_cutoff: 0.5,
                        mode: mode.as_u32(),
                        has_texture: 0,
                        uv_offset: Vec2::ZERO,
                    },
                    None,
                    true,
                    mode,
                );
                excluded.push(materials.add(material));
            }
            let params = materials.get(&excluded[0]).unwrap().params.clone();
            scene = materials.add(GmdlMaterial::new_scene(
                params,
                None,
                true,
                GmdlMode::Opaque,
            ));
        }
        for free_fade in [false, true] {
            app.world_mut()
                .resource_mut::<LockOnController>()
                .lock_entity = if free_fade { None } else { Some(target) };
            app.world_mut()
                .get_mut::<ThirdPersonState>(camera)
                .unwrap()
                .scene_fade_required = free_fade;
            app.update();
            let materials = app.world().resource::<Assets<GmdlMaterial>>();
            assert!(materials.get(&scene).unwrap().camera_occlusion_enabled);
            assert!(app.world().resource::<FrameUniforms>().camera.eye.w > 0.0);
            for handle in &excluded {
                let material = materials.get(handle).unwrap();
                assert!(!material.camera_occlusion_enabled);
                assert_eq!(material.params.opacity, 0.7);
                assert_eq!(material.params.alpha_cutoff, 0.5);
                assert_eq!(material.params.mode, material.mode.as_u32());
            }
        }
    }

    #[test]
    fn gmdl_shader_uses_bevy_material_bind_group() {
        let shader = include_str!("gmdl_material.wgsl");
        assert!(shader.contains("@group(#{MATERIAL_BIND_GROUP}) @binding(0)"));
        assert!(shader.contains("@group(#{MATERIAL_BIND_GROUP}) @binding(1)"));
        assert!(shader.contains("@group(#{MATERIAL_BIND_GROUP}) @binding(2)"));
        assert!(
            !shader.contains("@group(2)"),
            "Bevy reserves bind group 2 for per-mesh buffers"
        );
    }
}
