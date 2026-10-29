use bevy_material::descriptor::{
    CachedComputePipelineId, CachedRenderPipelineId, ComputePipelineDescriptor,
    RenderPipelineDescriptor,
};

use super::{ComputePipeline, PipelineCache, RenderPipeline};
use bevy_ecs::error::BevyError;
use bevy_log::error;
use bevy_platform::{
    collections::{
        hash_map::{Entry, VacantEntry},
        HashMap,
    },
    hash::FixedHasher,
};
use core::{hash::Hash, marker::PhantomData};
use variadics_please::all_tuples;

pub use bevy_render_macros::{Specializer, SpecializerKey};

pub trait Specializable {
    type Descriptor: PartialEq + Clone + Send + Sync;
    type CachedId: Clone + Send + Sync;
    fn queue(pipeline_cache: &PipelineCache, descriptor: Self::Descriptor) -> Self::CachedId;
    fn get_descriptor(pipeline_cache: &PipelineCache, id: Self::CachedId) -> &Self::Descriptor;
}

impl Specializable for RenderPipeline {
    type Descriptor = RenderPipelineDescriptor;
    type CachedId = CachedRenderPipelineId;

    fn queue(pipeline_cache: &PipelineCache, descriptor: Self::Descriptor) -> Self::CachedId {
        pipeline_cache.queue_render_pipeline(descriptor)
    }

    fn get_descriptor(
        pipeline_cache: &PipelineCache,
        id: CachedRenderPipelineId,
    ) -> &Self::Descriptor {
        pipeline_cache.get_render_pipeline_descriptor(id)
    }
}

impl Specializable for ComputePipeline {
    type Descriptor = ComputePipelineDescriptor;

    type CachedId = CachedComputePipelineId;

    fn queue(pipeline_cache: &PipelineCache, descriptor: Self::Descriptor) -> Self::CachedId {
        pipeline_cache.queue_compute_pipeline(descriptor)
    }

    fn get_descriptor(
        pipeline_cache: &PipelineCache,
        id: CachedComputePipelineId,
    ) -> &Self::Descriptor {
        pipeline_cache.get_compute_pipeline_descriptor(id)
    }
}

pub trait Specializer<T: Specializable>: Send + Sync + 'static {
    type Key: SpecializerKey;
    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut T::Descriptor,
    ) -> Result<Canonical<Self::Key>, BevyError>;
}

pub trait SpecializerKey: Clone + Hash + Eq {

    const IS_CANONICAL: bool;

    type Canonical: Hash + Eq;
}

pub type Canonical<T> = <T as SpecializerKey>::Canonical;

impl<T: Specializable> Specializer<T> for () {
    type Key = ();

    fn specialize(
        &self,
        _key: Self::Key,
        _descriptor: &mut T::Descriptor,
    ) -> Result<(), BevyError> {
        Ok(())
    }
}

impl<T: Specializable, V: Send + Sync + 'static> Specializer<T> for PhantomData<V> {
    type Key = ();

    fn specialize(
        &self,
        _key: Self::Key,
        _descriptor: &mut T::Descriptor,
    ) -> Result<(), BevyError> {
        Ok(())
    }
}

macro_rules! impl_specialization_key_tuple {
    ($(#[$meta:meta])* $($T:ident),*) => {
        $(#[$meta])*
        impl <$($T: SpecializerKey),*> SpecializerKey for ($($T,)*) {
            const IS_CANONICAL: bool = true $(&& <$T as SpecializerKey>::IS_CANONICAL)*;
            type Canonical = ($(Canonical<$T>,)*);
        }
    };
}

all_tuples!(
    #[doc(fake_variadic)]
    impl_specialization_key_tuple,
    0,
    12,
    T
);

pub struct Variants<T: Specializable, S: Specializer<T>> {
    specializer: S,
    base_descriptor: T::Descriptor,
    primary_cache: HashMap<S::Key, T::CachedId>,
    secondary_cache: HashMap<Canonical<S::Key>, T::CachedId>,
}

impl<T: Specializable, S: Specializer<T>> Variants<T, S> {

    #[inline]
    pub fn new(specializer: S, base_descriptor: T::Descriptor) -> Self {
        Self {
            specializer,
            base_descriptor,
            primary_cache: Default::default(),
            secondary_cache: Default::default(),
        }
    }

    #[inline]
    pub fn specialize(
        &mut self,
        pipeline_cache: &PipelineCache,
        key: S::Key,
    ) -> Result<T::CachedId, BevyError> {
        let entry = self.primary_cache.entry(key.clone());
        match entry {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => Self::specialize_slow(
                &self.specializer,
                self.base_descriptor.clone(),
                pipeline_cache,
                key,
                entry,
                &mut self.secondary_cache,
            ),
        }
    }

    #[cold]
    fn specialize_slow(
        specializer: &S,
        base_descriptor: T::Descriptor,
        pipeline_cache: &PipelineCache,
        key: S::Key,
        primary_entry: VacantEntry<S::Key, T::CachedId, FixedHasher>,
        secondary_cache: &mut HashMap<Canonical<S::Key>, T::CachedId>,
    ) -> Result<T::CachedId, BevyError> {
        let mut descriptor = base_descriptor.clone();
        let canonical_key = specializer.specialize(key.clone(), &mut descriptor)?;

        if <S::Key as SpecializerKey>::IS_CANONICAL {
            return Ok(primary_entry
                .insert(<T as Specializable>::queue(pipeline_cache, descriptor))
                .clone());
        }

        let id = match secondary_cache.entry(canonical_key) {
            Entry::Occupied(entry) => {
                if cfg!(debug_assertions) {
                    let stored_descriptor =
                        <T as Specializable>::get_descriptor(pipeline_cache, entry.get().clone());
                    if &descriptor != stored_descriptor {
                        error!(
                            "Invalid Specializer<{}> impl for {}: the cached descriptor \
                            is not equal to the generated descriptor for the given key. \
                            This means the Specializer implementation uses unused information \
                            from the key to specialize the pipeline. This is not allowed \
                            because it would invalidate the cache.",
                            core::any::type_name::<T>(),
                            core::any::type_name::<S>()
                        );
                    }
                }
                entry.into_mut().clone()
            }
            Entry::Vacant(entry) => entry
                .insert(<T as Specializable>::queue(pipeline_cache, descriptor))
                .clone(),
        };

        primary_entry.insert(id.clone());
        Ok(id)
    }
}
