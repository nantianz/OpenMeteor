use crate::{
    render_resource::AsBindGroupError, Extract, ExtractSchedule, MainWorld, Render, RenderApp,
    RenderStartup, RenderSystems, Res,
};
use bevy_app::{App, Plugin, SubApp};
use bevy_asset::{Asset, AssetEvent, AssetId, Assets, RenderAssetUsages};
use bevy_ecs::{
    prelude::{Commands, IntoScheduleConfigs, Local, MessageReader, ResMut, Resource},
    schedule::{ScheduleConfigs, SystemSet},
    system::{ScheduleSystem, StaticSystemParam, SystemParam, SystemParamItem, SystemState},
    world::{FromWorld, Mut},
};
use bevy_log::{debug, error};
use bevy_platform::collections::{HashMap, HashSet};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};
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

#[derive(Debug, Error)]
pub enum AssetExtractionError {
    #[error("The asset has already been extracted")]
    AlreadyExtracted,
    #[error("The asset type does not support extraction. To clone the asset to the renderworld, use `RenderAssetUsages::default()`")]
    NoExtractionImplementation,
}

pub trait RenderAsset: Send + Sync + 'static + Sized {

    type SourceAsset: Asset + Clone;

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
    fn byte_len(source_asset: &Self::SourceAsset) -> Option<usize> {
        None
    }

    fn prepare_asset(
        source_asset: Self::SourceAsset,
        asset_id: AssetId<Self::SourceAsset>,
        param: &mut SystemParamItem<Self::Param>,
        previous_asset: Option<&Self>,
    ) -> Result<Self, PrepareAssetError<Self::SourceAsset>>;

    fn unload_asset(
        _source_asset: AssetId<Self::SourceAsset>,
        _param: &mut SystemParamItem<Self::Param>,
    ) {
    }

    fn take_gpu_data(
        _source: &mut Self::SourceAsset,
        _previous_gpu_asset: Option<&Self>,
    ) -> Result<Self::SourceAsset, AssetExtractionError> {
        Err(AssetExtractionError::NoExtractionImplementation)
    }
}

pub struct RenderAssetPlugin<A: RenderAsset, AFTER: RenderAssetDependency + 'static = ()> {
    phantom: PhantomData<fn() -> (A, AFTER)>,
}

impl<A: RenderAsset, AFTER: RenderAssetDependency + 'static> Default
    for RenderAssetPlugin<A, AFTER>
{
    fn default() -> Self {
        Self {
            phantom: Default::default(),
        }
    }
}

impl<A: RenderAsset, AFTER: RenderAssetDependency + 'static> Plugin
    for RenderAssetPlugin<A, AFTER>
{
    fn build(&self, app: &mut App) {
        app.init_resource::<CachedExtractRenderAssetSystemState<A>>();
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<ExtractedAssets<A>>()
                .init_resource::<RenderAssets<A>>()
                .allow_ambiguous_resource::<RenderAssets<A>>()
                .init_resource::<PrepareNextFrameAssets<A>>()
                .add_systems(RenderStartup, collect_render_assets_to_reextract::<A>)
                .add_systems(
                    ExtractSchedule,
                    extract_render_asset::<A>.in_set(AssetExtractionSystems),
                );
            AFTER::register_system(
                render_app,
                prepare_assets::<A>.in_set(RenderSystems::PrepareAssets),
            );
        }
    }
}

pub trait RenderAssetDependency {
    fn register_system(render_app: &mut SubApp, system: ScheduleConfigs<ScheduleSystem>);
}

impl RenderAssetDependency for () {
    fn register_system(render_app: &mut SubApp, system: ScheduleConfigs<ScheduleSystem>) {
        render_app.add_systems(Render, system);
    }
}

impl<A: RenderAsset> RenderAssetDependency for A {
    fn register_system(render_app: &mut SubApp, system: ScheduleConfigs<ScheduleSystem>) {
        render_app.add_systems(Render, system.after(prepare_assets::<A>));
    }
}

#[derive(Resource)]
pub struct ExtractedAssets<A: RenderAsset> {

    pub extracted: Vec<(AssetId<A::SourceAsset>, A::SourceAsset)>,

    pub removed: HashSet<AssetId<A::SourceAsset>>,

    pub modified: HashSet<AssetId<A::SourceAsset>>,

    pub added: HashSet<AssetId<A::SourceAsset>>,
}

impl<A: RenderAsset> Default for ExtractedAssets<A> {
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
pub struct RenderAssets<A: RenderAsset>(HashMap<AssetId<A::SourceAsset>, A>);

impl<A: RenderAsset> Default for RenderAssets<A> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<A: RenderAsset> RenderAssets<A> {
    pub fn get(&self, id: impl Into<AssetId<A::SourceAsset>>) -> Option<&A> {
        self.0.get(&id.into())
    }

    pub fn get_mut(&mut self, id: impl Into<AssetId<A::SourceAsset>>) -> Option<&mut A> {
        self.0.get_mut(&id.into())
    }

    pub fn insert(&mut self, id: impl Into<AssetId<A::SourceAsset>>, value: A) -> Option<A> {
        self.0.insert(id.into(), value)
    }

    pub fn remove(&mut self, id: impl Into<AssetId<A::SourceAsset>>) -> Option<A> {
        self.0.remove(&id.into())
    }

    pub fn iter(&self) -> impl Iterator<Item = (AssetId<A::SourceAsset>, &A)> {
        self.0.iter().map(|(k, v)| (*k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (AssetId<A::SourceAsset>, &mut A)> {
        self.0.iter_mut().map(|(k, v)| (*k, v))
    }
}

#[derive(Resource)]
struct CachedExtractRenderAssetSystemState<A: RenderAsset> {
    state: SystemState<(
        MessageReader<'static, 'static, AssetEvent<A::SourceAsset>>,
        ResMut<'static, Assets<A::SourceAsset>>,
        Option<Res<'static, RenderAssets<A>>>,
    )>,
}

impl<A: RenderAsset> FromWorld for CachedExtractRenderAssetSystemState<A> {
    fn from_world(world: &mut bevy_ecs::world::World) -> Self {
        Self {
            state: SystemState::new(world),
        }
    }
}

#[derive(Resource)]
pub(crate) struct RenderAssetsToReExtract<A: RenderAsset> {
    ids: Vec<AssetId<A::SourceAsset>>,
}

fn collect_render_assets_to_reextract<A: RenderAsset>(
    mut commands: Commands,
    mut render_assets: ResMut<RenderAssets<A>>,
    mut prepare_next_frame: ResMut<PrepareNextFrameAssets<A>>,
) {
    let ids: Vec<_> = render_assets.0.drain().map(|(id, _)| id).collect();
    prepare_next_frame.assets.clear();
    if !ids.is_empty() {
        commands.insert_resource(RenderAssetsToReExtract::<A> { ids });
    }
}

pub(crate) fn extract_render_asset<A: RenderAsset>(
    mut to_reextract: Option<ResMut<RenderAssetsToReExtract<A>>>,
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
        |world, mut cached_state: Mut<CachedExtractRenderAssetSystemState<A>>| {
            let (mut events, mut assets, maybe_render_assets) = cached_state.state.get_mut(world).unwrap();

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
                            if let Some(asset) = assets.get_mut_untracked(id) {
                                let previous_asset = maybe_render_assets.as_ref().and_then(|render_assets| render_assets.get(id));
                                match A::take_gpu_data(asset, previous_asset) {
                                    Ok(gpu_data_asset) => {
                                        extracted_assets.extracted.push((id, gpu_data_asset));
                                        extracted_assets.added.insert(id);
                                    }
                                    Err(e) => {
                                        error!("{} with RenderAssetUsages == RENDER_WORLD cannot be extracted: {e}", core::any::type_name::<A>());
                                    }
                                };
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
pub struct PrepareNextFrameAssets<A: RenderAsset> {
    assets: Vec<(AssetId<A::SourceAsset>, A::SourceAsset)>,
}

impl<A: RenderAsset> Default for PrepareNextFrameAssets<A> {
    fn default() -> Self {
        Self {
            assets: Default::default(),
        }
    }
}

pub fn prepare_assets<A: RenderAsset>(
    mut extracted_assets: ResMut<ExtractedAssets<A>>,
    mut render_assets: ResMut<RenderAssets<A>>,
    mut prepare_next_frame: ResMut<PrepareNextFrameAssets<A>>,
    param: StaticSystemParam<<A as RenderAsset>::Param>,
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

        let previous_asset = render_assets.get(id);
        match A::prepare_asset(extracted_asset, id, &mut param, previous_asset) {
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

        let previous_asset = render_assets.remove(id);

        let write_bytes = if let Some(size) = A::byte_len(&extracted_asset) {
            if bpf.exhausted() {
                prepare_next_frame.assets.push((id, extracted_asset));
                continue;
            }
            size
        } else {
            0
        };

        match A::prepare_asset(extracted_asset, id, &mut param, previous_asset.as_ref()) {
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

pub fn reset_render_asset_bytes_per_frame(
    mut bpf_limiter: ResMut<RenderAssetBytesPerFrameLimiter>,
) {
    bpf_limiter.reset();
}

pub fn extract_render_asset_bytes_per_frame(
    bpf: Extract<Res<RenderAssetBytesPerFrame>>,
    mut bpf_limiter: ResMut<RenderAssetBytesPerFrameLimiter>,
) {
    bpf_limiter.max_bytes = bpf.max_bytes;
}

#[derive(Resource, Default)]
pub struct RenderAssetBytesPerFrame {
    pub max_bytes: Option<usize>,
}

impl RenderAssetBytesPerFrame {

    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: Some(max_bytes),
        }
    }
}

#[derive(Resource, Default)]
pub struct RenderAssetBytesPerFrameLimiter {

    pub max_bytes: Option<usize>,

    pub bytes_written: AtomicUsize,
}

impl RenderAssetBytesPerFrameLimiter {

    pub fn reset(&mut self) {
        if self.max_bytes.is_none() {
            return;
        }
        self.bytes_written.store(0, Ordering::Relaxed);
    }

    pub fn available_bytes(&self, required_bytes: usize) -> usize {
        if let Some(max_bytes) = self.max_bytes {
            let total_bytes = self
                .bytes_written
                .fetch_add(required_bytes, Ordering::Relaxed);

            if total_bytes >= max_bytes {
                required_bytes.saturating_sub(total_bytes - max_bytes)
            } else {
                required_bytes
            }
        } else {
            required_bytes
        }
    }

    pub(crate) fn write_bytes(&self, bytes: usize) {
        if self.max_bytes.is_some() && bytes > 0 {
            self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    pub(crate) fn exhausted(&self) -> bool {
        if let Some(max_bytes) = self.max_bytes {
            let bytes_written = self.bytes_written.load(Ordering::Relaxed);
            bytes_written >= max_bytes
        } else {
            false
        }
    }
}
