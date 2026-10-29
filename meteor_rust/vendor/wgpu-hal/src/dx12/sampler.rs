

use alloc::vec::Vec;

use hashbrown::{hash_map::Entry, HashMap};

use ordered_float::OrderedFloat;
use parking_lot::Mutex;
use windows::Win32::Graphics::Direct3D12::*;

use crate::dx12::HResult;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SamplerIndex(u32);

#[derive(Debug, Copy, Clone)]
struct HashableSamplerDesc(D3D12_SAMPLER_DESC);

impl PartialEq for HashableSamplerDesc {
    fn eq(&self, other: &Self) -> bool {
        self.0.Filter == other.0.Filter
            && self.0.AddressU == other.0.AddressU
            && self.0.AddressV == other.0.AddressV
            && self.0.AddressW == other.0.AddressW
            && OrderedFloat(self.0.MipLODBias) == OrderedFloat(other.0.MipLODBias)
            && self.0.MaxAnisotropy == other.0.MaxAnisotropy
            && self.0.ComparisonFunc == other.0.ComparisonFunc
            && self.0.BorderColor.map(OrderedFloat) == other.0.BorderColor.map(OrderedFloat)
            && OrderedFloat(self.0.MinLOD) == OrderedFloat(other.0.MinLOD)
            && OrderedFloat(self.0.MaxLOD) == OrderedFloat(other.0.MaxLOD)
    }
}

impl Eq for HashableSamplerDesc {}

impl core::hash::Hash for HashableSamplerDesc {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.Filter.0.hash(state);
        self.0.AddressU.0.hash(state);
        self.0.AddressV.0.hash(state);
        self.0.AddressW.0.hash(state);
        OrderedFloat(self.0.MipLODBias).hash(state);
        self.0.MaxAnisotropy.hash(state);
        self.0.ComparisonFunc.0.hash(state);
        self.0.BorderColor.map(OrderedFloat).hash(state);
        OrderedFloat(self.0.MinLOD).hash(state);
        OrderedFloat(self.0.MaxLOD).hash(state);
    }
}

struct CacheEntry {
    index: SamplerIndex,
    ref_count: u32,
}

pub(crate) struct SamplerHeapState {

    mapping: HashMap<HashableSamplerDesc, CacheEntry>,

    freelist: Vec<SamplerIndex>,
}

pub(crate) struct SamplerHeap {

    state: Mutex<SamplerHeapState>,

    heap: ID3D12DescriptorHeap,

    heap_cpu_start_handle: D3D12_CPU_DESCRIPTOR_HANDLE,

    heap_gpu_start_handle: D3D12_GPU_DESCRIPTOR_HANDLE,

    descriptor_stride: u32,
}

impl SamplerHeap {
    pub fn new(
        device: &ID3D12Device,
        private_caps: &super::PrivateCapabilities,
    ) -> Result<Self, crate::DeviceError> {
        profiling::scope!("SamplerHeap::new");

        const SAMPLER_HEAP_SIZE_CLAMP: u32 = 64 * 1024;

        let max_unique_samplers = private_caps
            .max_sampler_descriptor_heap_size
            .min(SAMPLER_HEAP_SIZE_CLAMP);

        let desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
            NumDescriptors: max_unique_samplers,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        };
        let heap = unsafe { device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&desc) }
            .into_device_result("Failed to create global GPU-Visible Sampler Descriptor Heap")?;

        let heap_cpu_start_handle = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
        let heap_gpu_start_handle = unsafe { heap.GetGPUDescriptorHandleForHeapStart() };

        let descriptor_stride =
            unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER) };

        Ok(Self {
            state: Mutex::new(SamplerHeapState {
                mapping: HashMap::new(),

                freelist: (0..max_unique_samplers).map(SamplerIndex).rev().collect(),
            }),
            heap,
            heap_cpu_start_handle,
            heap_gpu_start_handle,
            descriptor_stride,
        })
    }

    pub fn heap(&self) -> &ID3D12DescriptorHeap {
        &self.heap
    }

    pub fn gpu_descriptor_table(&self) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        self.heap_gpu_start_handle
    }

    pub fn create_sampler(
        &self,
        device: &ID3D12Device,
        desc: D3D12_SAMPLER_DESC,
    ) -> Result<SamplerIndex, crate::DeviceError> {
        profiling::scope!("SamplerHeap::create_sampler");

        let hashable_desc = HashableSamplerDesc(desc);

        let state = &mut *self.state.lock();

        match state.mapping.entry(hashable_desc) {
            Entry::Occupied(occupied_entry) => {

                let entry = occupied_entry.into_mut();
                entry.ref_count += 1;
                Ok(entry.index)
            }
            Entry::Vacant(vacant_entry) => {

                let Some(index) = state.freelist.pop() else {

                    log::error!("There is no more room in the global sampler heap for more unique samplers. Your device supports a maximum of {} unique samplers.", state.mapping.len());
                    return Err(crate::DeviceError::OutOfMemory);
                };

                let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: self.heap_cpu_start_handle.ptr
                        + self.descriptor_stride as usize * index.0 as usize,
                };

                unsafe {
                    device.CreateSampler(&desc, handle);
                }

                vacant_entry.insert(CacheEntry {
                    index,
                    ref_count: 1,
                });

                Ok(index)
            }
        }
    }

    pub fn destroy_sampler(&self, desc: D3D12_SAMPLER_DESC, provided_index: SamplerIndex) {
        profiling::scope!("SamplerHeap::destroy_sampler");

        let state = &mut *self.state.lock();

        let Entry::Occupied(mut hash_map_entry) = state.mapping.entry(HashableSamplerDesc(desc))
        else {
            log::error!(
                "Tried to destroy a sampler that doesn't exist. Sampler description: {desc:#?}"
            );
            return;
        };
        let cache_entry = hash_map_entry.get_mut();

        assert_eq!(
            cache_entry.index, provided_index,
            "Mismatched sampler index, this is an implementation bug"
        );

        cache_entry.ref_count -= 1;

        if cache_entry.ref_count == 0 {
            state.freelist.push(cache_entry.index);
            hash_map_entry.remove();
        }
    }
}
