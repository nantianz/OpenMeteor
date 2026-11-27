use super::*;
use bevy_asset::{embedded_asset, load_embedded_asset, AssetServer};
use bevy_core_pipeline::FullscreenShader;
use std::sync::{Mutex, OnceLock};

#[derive(Component)]
pub struct CacheStaticDirectionalShadows;

#[derive(Component)]
pub struct StaticShadowCaster;

#[derive(Resource, Default)]
pub struct ExtractedStaticCasters(pub(super) HashSet<MainEntity>);

pub(crate) fn extract_static_shadow_casters(
    mut extracted: ResMut<ExtractedStaticCasters>,
    entities: Extract<Query<Entity, With<StaticShadowCaster>>>,
) {
    if entities.is_empty() && extracted.0.is_empty() {
        return;
    }
    extracted.0.clear();
    extracted.0.extend(entities.iter().map(MainEntity::from));
}

#[derive(PartialEq)]
struct CacheKey {
    projection: Mat4,
    casters: Vec<(MainEntity, ShadowBatchSetKey, ShadowBinKey)>,
}

struct StaticDepth {
    texture: Texture,
    view: TextureView,
    copy_source: OnceLock<BindGroup>,
    size: Extent3d,
    state: Mutex<CacheState>,
}

#[derive(Default)]
struct CacheState {
    observed_projection: Option<Mat4>,
    stable_frames: u8,
    saved: Option<CacheKey>,
}

impl CacheState {
    fn can_reuse(&mut self, projection: Mat4) -> bool {
        if self.observed_projection == Some(projection) {
            self.stable_frames = self.stable_frames.saturating_add(1);
        } else {
            self.observed_projection = Some(projection);
            self.stable_frames = 1;
        }
        self.saved
            .as_ref()
            .is_some_and(|saved| saved.projection == projection)
            || self.stable_frames >= 3
    }
}

#[derive(Component)]
pub struct ShadowCacheTarget {
    cache: Arc<StaticDepth>,
    target: Texture,
    target_layer: u32,
    direct_copy: bool,
}

#[derive(Resource)]
pub(crate) struct StaticDepthCopyPipeline {
    layout: BindGroupLayoutDescriptor,
    id: CachedRenderPipelineId,
}

pub(crate) fn register_static_depth_copy_shader(app: &mut App) {
    embedded_asset!(app, "copy_static_depth.wgsl");
}

pub(crate) fn init_static_depth_copy(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
    assets: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "static shadow depth source",
        &BindGroupLayoutEntries::single(ShaderStages::FRAGMENT, binding_types::texture_depth_2d()),
    );
    let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("compose static shadow depth".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        depth_stencil: Some(DepthStencilState {
            format: CORE_3D_DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(CompareFunction::Always),
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        }),
        fragment: Some(FragmentState {
            shader: load_embedded_asset!(assets.as_ref(), "copy_static_depth.wgsl"),
            ..default()
        }),
        ..default()
    });
    commands.insert_resource(StaticDepthCopyPipeline { layout, id });
}

pub(super) enum PreparedStaticDepth<'a> {
    Copied,
    Draw {
        pipeline: &'a RenderPipeline,
        source: BindGroup,
    },
}

impl ShadowCacheTarget {
    pub fn has_cached_depth(&self) -> bool {
        self.cached_caster_count() > 0
    }

    pub fn cached_caster_count(&self) -> usize {
        self.cache
            .state
            .lock()
            .unwrap()
            .saved
            .as_ref()
            .map_or(0, |key| key.casters.len())
    }

    pub(super) fn new(
        device: &RenderDevice,
        target: &Texture,
        target_layer: u32,
        direct_copy: bool,
        previous: Option<&Self>,
    ) -> Self {
        let size = Extent3d {
            width: target.width(),
            height: target.height(),
            depth_or_array_layers: 1,
        };
        let cache = previous
            .filter(|previous| previous.cache.size == size && previous.direct_copy == direct_copy)
            .map(|previous| previous.cache.clone())
            .unwrap_or_else(|| {
                let copy_usage = if direct_copy {
                    TextureUsages::COPY_SRC
                } else {
                    TextureUsages::TEXTURE_BINDING
                };
                let texture = device.create_texture(&TextureDescriptor {
                    label: Some("static directional shadow cache"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: CORE_3D_DEPTH_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | copy_usage,
                    view_formats: &[],
                });
                let view = texture.create_view(&TextureViewDescriptor::default());
                Arc::new(StaticDepth {
                    texture,
                    view,
                    copy_source: OnceLock::new(),
                    size,
                    state: Mutex::new(CacheState::default()),
                })
            });
        Self {
            cache,
            target: target.clone(),
            target_layer,
            direct_copy,
        }
    }

    pub(super) fn prepare<'a>(
        &self,
        world: &'a World,
        ctx: &mut RenderContext,
        view: Entity,
        extracted_view: &ExtractedView,
        phase: &BinnedRenderPhase<Shadow>,
    ) -> Option<PreparedStaticDepth<'a>> {
        let projection = extracted_view.clip_from_world?;
        let mut state = self.cache.state.lock().unwrap();

        if !state.can_reuse(projection) {
            return None;
        }
        let pipeline_cache = world.resource::<PipelineCache>();
        let copy = if self.direct_copy {
            None
        } else {
            let copy = world.get_resource::<StaticDepthCopyPipeline>()?;
            let pipeline = pipeline_cache.get_render_pipeline(copy.id)?;
            Some((copy, pipeline))
        };
        let mut casters = Vec::new();
        for (entity, bin) in phase
            .entity_bins()
            .filter(|(_, bin)| bin.batch_set_key.static_caster)
        {
            if pipeline_cache
                .get_render_pipeline(bin.batch_set_key.pipeline)
                .is_none()
            {
                return None;
            }
            casters.push((entity, bin.batch_set_key.clone(), bin.bin_key.clone()));
        }
        if casters.is_empty() {
            return None;
        }
        casters.sort_unstable_by_key(|(entity, _, _)| entity.index());
        let key = CacheKey {
            projection,
            casters,
        };
        if state.saved.as_ref() != Some(&key) {
            let depth = DepthAttachment::new(self.cache.view.clone(), Some(0.0));
            let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("refresh static directional shadow cache"),
                color_attachments: &[],
                depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            state.saved = None;
            if let Err(err) = phase.render_filtered(&mut pass, world, view, |key| key.static_caster)
            {
                error!("Error encountered while refreshing the static shadow phase {err:?}");
                return None;
            }
            state.saved = Some(key);
        }
        if let Some((copy, pipeline)) = copy {
            let source = self.cache.copy_source.get_or_init(|| {
                ctx.render_device().create_bind_group(
                    "static shadow depth source",
                    &pipeline_cache.get_bind_group_layout(&copy.layout),
                    &BindGroupEntries::single(&self.cache.view),
                )
            });
            return Some(PreparedStaticDepth::Draw {
                pipeline,
                source: source.clone(),
            });
        }
        ctx.command_encoder().copy_texture_to_texture(
            TexelCopyTextureInfo {
                texture: &self.cache.texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::DepthOnly,
            },
            TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: Origin3d {
                    z: self.target_layer,
                    ..Origin3d::ZERO
                },
                aspect: TextureAspect::DepthOnly,
            },
            self.cache.size,
        );
        Some(PreparedStaticDepth::Copied)
    }
}
