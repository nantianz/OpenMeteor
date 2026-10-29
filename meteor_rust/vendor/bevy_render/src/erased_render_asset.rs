use crate::{
    render_resource::AsBindGroupError, ExtractSchedule, MainWorld, Render, RenderApp,
    RenderStartup, RenderSystems, Res,
};
use bevy_app::{App, Plugin, SubApp};
use bevy_asset::RenderAssetUsages;
use bevy_asset::{Asset, AssetEvent, AssetId, Assets, UntypedAssetId};
use bevy_ecs::{
    prelude::{Commands, IntoScheduleConfigs, Local, MessageReader, ResMut, Resource},
    schedule::{ScheduleConfigs, SystemSet},
    system::{ScheduleSystem, StaticSystemParam, SystemParam, SystemParamItem, SystemState},
    world::{FromWorld, Mut},
};
use bevy_log::{debug, error};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_render::render_asset::RenderAssetBytesPerFrameLimiter;
use core::marker::PhantomData;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrepareAssetError<E: Send + Sync + 'static> {
    #[error("Failed to prepare asset")]
    RetryNextUpdate(E),
    #[error("Failed to build bind group: {0}")]
    AsBindGroupError(AsBindGroupError),
}

#[derive(SystemSet, Clone, PartialEq, Eq, Debug, Hash)]
pub struct AssetExtractionSystems;

pub trait ErasedRenderAsset: Send + Sync + 'static {

    type SourceAsset: Asset + Clone;

    type ErasedAsset: Send + Sync + 'static + Sized;

    type Param: SystemParam;

    #[inline]
    fn asset_usage(_source_asset: &Self::SourceAsset) -> RenderAssetUsages {
        RenderAssetUsages::default()
    }

    #[inline]
    #[expect(
        unused_variables,
        reason = "The parameters here are intentionally unused by the default implementation; however, putting underscores here will result in the underscores being copied by rust-analyzer's tab completion."
    )]
    fn byte_len(erased_asset: &Self::SourceAsset) -> Option<usize> {
        None
    }

    fn prepare_asset(
        source_asset: Self::SourceAsset,
        asset_id: AssetId<Self::SourceAsset>,
        param: &mut SystemParamItem<Self::Param>,
    ) -> Result<Self::ErasedAsset, PrepareAssetError<Self::SourceAsset>>;

    fn unload_asset(
        _source_asset: AssetId<Self::SourceAsset>,
        _param: &mut SystemParamItem<Self::Param>,
    ) {
    }
}

pub struct ErasedRenderAssetPlugin<
    A: ErasedRenderAsset,
    AFTER: ErasedRenderAssetDependency + 'static = (),
> {
    phantom: PhantomData<fn() -> (A, AFTER)>,
}

impl<A: ErasedRenderAsset, AFTER: ErasedRenderAssetDependency + 'static> Default
    for ErasedRenderAssetPlugin<A, AFTER>
{
    fn default() -> Self {
        Self {
            phantom: Default::default(),
        }
    }
}

impl<A: ErasedRenderAsset, AFTER: ErasedRenderAssetDependency + 'static> Plugin
    for ErasedRenderAssetPlugin<A, AFTER>
{
    fn build(&self, app: &mut App) {
        app.init_resource::<CachedExtractErasedRenderAssetSystemState<A>>();
    }

    fn finish(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<ExtractedAssets<A>>()
                .init_resource::<ErasedRenderAssets<A::ErasedAsset>>()
                .allow_ambiguous_resource::<ErasedRenderAssets<A::ErasedAsset>>()
                .init_resource::<PrepareNextFrameAssets<A>>()
                .add_systems(
                    RenderStartup,
                    collect_erased_render_assets_to_reextract::<A>,
                )
                .add_systems(
                    ExtractSchedule,
                    extract_erased_render_asset::<A>.in_set(AssetExtractionSystems),
                );
            AFTER::register_system(
                render_app,
                prepare_erased_assets::<A>.in_set(RenderSystems::PrepareAssets),
            );
        }
    }
}

pub trait ErasedRenderAssetDependency {
    fn register_system(render_app: &mut SubApp, system: ScheduleConfigs<ScheduleSystem>);
}

impl ErasedRenderAssetDependency for () {
    fn register_system(render_app: &mut SubApp, system: ScheduleConfigs<ScheduleSystem>) {
        render_app.add_systems(Render, system);
    }
}

impl<A: ErasedRenderAsset> ErasedRenderAssetDependency for A {
    fn register_system(render_app: &mut SubApp, system: ScheduleConfigs<ScheduleSystem>) {
        render_app.add_systems(Render, system.after(prepare_erased_assets::<A>));
    }
}

#[derive(Resource)]
pub struct ExtractedAssets<A: ErasedRenderAsset> {

    pub extracted: Vec<(AssetId<A::SourceAsset>, A::SourceAsset)>,

    pub removed: HashSet<AssetId<A::SourceAsset>>,

    pub modified: HashSet<AssetId<A::SourceAsset>>,

    pub added: HashSet<AssetId<A::SourceAsset>>,
}

impl<A: ErasedRenderAsset> Default for ExtractedAssets<A> {
    fn default() -> Self {
        Self {
            extracted: Default::default(),
            removed: Default::default(),
            modified: Default::default(),
            added: Default::default(),
        }
    }
}

#[derive(Resource)]
pub struct ErasedRenderAssets<ERA>(HashMap<UntypedAssetId, ERA>);

impl<ERA> Default for ErasedRenderAssets<ERA> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<ERA> ErasedRenderAssets<ERA> {
    pub fn get(&self, id: impl Into<UntypedAssetId>) -> Option<&ERA> {
        self.0.get(&id.into())
    }

    pub fn get_mut(&mut self, id: impl Into<UntypedAssetId>) -> Option<&mut ERA> {
        self.0.get_mut(&id.into())
    }

    pub fn insert(&mut self, id: impl Into<UntypedAssetId>, value: ERA) -> Option<ERA> {
        self.0.insert(id.into(), value)
    }

    pub fn remove(&mut self, id: impl Into<UntypedAssetId>) -> Option<ERA> {
        self.0.remove(&id.into())
    }

    pub fn iter(&self) -> impl Iterator<Item = (UntypedAssetId, &ERA)> {
        self.0.iter().map(|(k, v)| (*k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (UntypedAssetId, &mut ERA)> {
        self.0.iter_mut().map(|(k, v)| (*k, v))
    }
}

#[derive(Resource)]
struct CachedExtractErasedRenderAssetSystemState<A: ErasedRenderAsset> {
    state: SystemState<(
        MessageReader<'static, 'static, AssetEvent<A::SourceAsset>>,
        ResMut<'static, Assets<A::SourceAsset>>,
    )>,
}

impl<A: ErasedRenderAsset> FromWorld for CachedExtractErasedRenderAssetSystemState<A> {
    fn from_world(world: &mut bevy_ecs::world::World) -> Self {
        Self {
            state: SystemState::new(world),
        }
    }
}

#[derive(Resource)]
pub(crate) struct ErasedRenderAssetsToReExtract<A: ErasedRenderAsset> {
    ids: Vec<AssetId<A::SourceAsset>>,
}

fn collect_erased_render_assets_to_reextract<A: ErasedRenderAsset>(
    mut commands: Commands,
    mut render_assets: ResMut<ErasedRenderAssets<A::ErasedAsset>>,
    mut prepare_next_frame: ResMut<PrepareNextFrameAssets<A>>,
) {
    let source_type_id = core::any::TypeId::of::<A::SourceAsset>();

    let mut ids = Vec::new();
    render_assets.0.retain(|untyped_id, _| {
        if untyped_id.type_id() == source_type_id {
            ids.push(untyped_id.typed());
            false
        } else {
            true
        }
    });
    prepare_next_frame.assets.clear();
    if !ids.is_empty() {
        commands.insert_resource(ErasedRenderAssetsToReExtract::<A> { ids });
    }
}

pub(crate) fn extract_erased_render_asset<A: ErasedRenderAsset>(
    mut to_reextract: Option<ResMut<ErasedRenderAssetsToReExtract<A>>>,
    mut extracted_assets: ResMut<ExtractedAssets<A>>,
    mut main_world: ResMut<MainWorld>,
    mut needs_extracting: Local<HashSet<AssetId<A::SourceAsset>>>,
) {
    extracted_assets.extracted.clear();
    extracted_assets.removed.clear();
    extracted_assets.modified.clear();
    extracted_assets.added.clear();
    needs_extracting.clear();

    let reextract_ids = to_reextract
        .as_mut()
        .map(|r| core::mem::take(&mut r.ids))
        .filter(|ids| !ids.is_empty());

    main_world.resource_scope(
        |world, mut cached_state: Mut<CachedExtractErasedRenderAssetSystemState<A>>| {
            let (mut events, mut assets) = cached_state.state.get_mut(world).unwrap();

            if let Some(reextract_ids) = reextract_ids {
                needs_extracting.extend(reextract_ids);
            }

            for event in events.read() {
                #[expect(
                    clippy::match_same_arms,
                    reason = "LoadedWithDependencies is marked as a TODO, so it's likely this will no longer lint soon."
                )]
                match event {
                    AssetEvent::Added { id } => {
                        needs_extracting.insert(*id);
                    }
                    AssetEvent::Modified { id } => {
                        needs_extracting.insert(*id);
                        extracted_assets.modified.insert(*id);
                    }
                    AssetEvent::Removed { .. } => {

                    }
                    AssetEvent::Unused { id } => {
                        needs_extracting.remove(id);
                        extracted_assets.modified.remove(id);
                        extracted_assets.removed.insert(*id);
                    }
                    AssetEvent::LoadedWithDependencies { .. } => {

                    }
                }
            }

            for id in needs_extracting.drain() {
                if let Some(asset) = assets.get(id) {
                    let asset_usage = A::asset_usage(asset);
                    if asset_usage.contains(RenderAssetUsages::RENDER_WORLD) {
                        if asset_usage == RenderAssetUsages::RENDER_WORLD {
                            if let Some(asset) = assets.remove(id) {
                                extracted_assets.extracted.push((id, asset));
                                extracted_assets.added.insert(id);
                            }
                        } else {
                            extracted_assets.extracted.push((id, asset.clone()));
                            extracted_assets.added.insert(id);
                        }
                    }
                }
            }

            cached_state.state.apply(world);
        },
    );
}

#[derive(Resource)]
pub struct PrepareNextFrameAssets<A: ErasedRenderAsset> {
    assets: Vec<(AssetId<A::SourceAsset>, A::SourceAsset)>,
}

impl<A: ErasedRenderAsset> Default for PrepareNextFrameAssets<A> {
    fn default() -> Self {
        Self {
            assets: Default::default(),
        }
    }
}

pub fn prepare_erased_assets<A: ErasedRenderAsset>(
    mut extracted_assets: ResMut<ExtractedAssets<A>>,
    mut render_assets: ResMut<ErasedRenderAssets<A::ErasedAsset>>,
    mut prepare_next_frame: ResMut<PrepareNextFrameAssets<A>>,
    param: StaticSystemParam<<A as ErasedRenderAsset>::Param>,
    bpf: Res<RenderAssetBytesPerFrameLimiter>,
) {
    let mut wrote_asset_count = 0;

    let mut param = param.into_inner();
    let queued_assets = core::mem::take(&mut prepare_next_frame.assets);
    for (id, extracted_asset) in queued_assets {
        if extracted_assets.removed.contains(&id) || extracted_assets.added.contains(&id) {

            continue;
        }

        let write_bytes = if let Some(size) = A::byte_len(&extracted_asset) {

            if bpf.exhausted() {
                prepare_next_frame.assets.push((id, extracted_asset));
                continue;
            }
            size
        } else {
            0
        };

        match A::prepare_asset(extracted_asset, id, &mut param) {
            Ok(prepared_asset) => {
                render_assets.insert(id, prepared_asset);
                bpf.write_bytes(write_bytes);
                wrote_asset_count += 1;
            }
            Err(PrepareAssetError::RetryNextUpdate(extracted_asset)) => {
                prepare_next_frame.assets.push((id, extracted_asset));
            }
            Err(PrepareAssetError::AsBindGroupError(e)) => {
                error!(
                    "{} Bind group construction failed: {e}",
                    core::any::type_name::<A>()
                );
            }
        }
    }

    for removed in extracted_assets.removed.drain() {
        render_assets.remove(removed);
        A::unload_asset(removed, &mut param);
    }

    for (id, extracted_asset) in extracted_assets.extracted.drain(..) {

        render_assets.remove(id);

        let write_bytes = if let Some(size) = A::byte_len(&extracted_asset) {
            if bpf.exhausted() {
                prepare_next_frame.assets.push((id, extracted_asset));
                continue;
            }
            size
        } else {
            0
        };

        match A::prepare_asset(extracted_asset, id, &mut param) {
            Ok(prepared_asset) => {
                render_assets.insert(id, prepared_asset);
                bpf.write_bytes(write_bytes);
                wrote_asset_count += 1;
            }
            Err(PrepareAssetError::RetryNextUpdate(extracted_asset)) => {
                prepare_next_frame.assets.push((id, extracted_asset));
            }
            Err(PrepareAssetError::AsBindGroupError(e)) => {
                error!(
                    "{} Bind group construction failed: {e}",
                    core::any::type_name::<A>()
                );
            }
        }
    }

    if bpf.exhausted() && !prepare_next_frame.assets.is_empty() {
        debug!(
            "{} write budget exhausted with {} assets remaining (wrote {})",
            core::any::type_name::<A>(),
            prepare_next_frame.assets.len(),
            wrote_asset_count
        );
    }
}
