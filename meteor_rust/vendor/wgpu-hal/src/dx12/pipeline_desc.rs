

use core::{ffi::c_void, mem::ManuallyDrop, ptr::NonNull};

use alloc::vec::Vec;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_core::Interface;

use crate::dx12::borrow_interface_temporarily;

#[repr(transparent)]
#[derive(Copy, Clone)]

struct RootSignature(Option<NonNull<c_void>>);

#[repr(transparent)]
#[derive(Copy, Clone)]
struct VertexShader(D3D12_SHADER_BYTECODE);
#[repr(transparent)]
#[derive(Copy, Clone)]
struct PixelShader(D3D12_SHADER_BYTECODE);

#[repr(transparent)]
#[derive(Copy, Clone)]
struct MeshShader(D3D12_SHADER_BYTECODE);

#[repr(transparent)]
#[derive(Copy, Clone)]
struct TaskShader(D3D12_SHADER_BYTECODE);

#[repr(transparent)]
#[derive(Copy, Clone)]
struct SampleMask(u32);

#[repr(transparent)]
#[derive(Copy, Clone)]
struct NodeMask(u32);

unsafe trait RenderPipelineStreamObject: Copy {
    const SUBOBJECT_TYPE: D3D12_PIPELINE_STATE_SUBOBJECT_TYPE;
}

macro_rules! implement_stream_object {
    (unsafe $ty:ty => $variant:expr) => {
        unsafe impl RenderPipelineStreamObject for $ty {
            const SUBOBJECT_TYPE: D3D12_PIPELINE_STATE_SUBOBJECT_TYPE = $variant;
        }
    };
}

implement_stream_object! { unsafe RootSignature => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE }
implement_stream_object! { unsafe VertexShader => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_VS }
implement_stream_object! { unsafe PixelShader => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS }
implement_stream_object! { unsafe MeshShader => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS }
implement_stream_object! { unsafe TaskShader => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_AS }
implement_stream_object! { unsafe D3D12_BLEND_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND }
implement_stream_object! { unsafe SampleMask => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_MASK }
implement_stream_object! { unsafe D3D12_RASTERIZER_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER }
implement_stream_object! { unsafe D3D12_DEPTH_STENCIL_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL }
implement_stream_object! { unsafe D3D12_PRIMITIVE_TOPOLOGY_TYPE => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PRIMITIVE_TOPOLOGY }
implement_stream_object! { unsafe D3D12_RT_FORMAT_ARRAY => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS }
implement_stream_object! { unsafe DXGI_FORMAT => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL_FORMAT }
implement_stream_object! { unsafe DXGI_SAMPLE_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_DESC }
implement_stream_object! { unsafe NodeMask => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_NODE_MASK }
implement_stream_object! { unsafe D3D12_CACHED_PIPELINE_STATE => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CACHED_PSO }
implement_stream_object! { unsafe D3D12_PIPELINE_STATE_FLAGS => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_FLAGS }
implement_stream_object! { unsafe D3D12_INPUT_LAYOUT_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_INPUT_LAYOUT }
implement_stream_object! { unsafe D3D12_INDEX_BUFFER_STRIP_CUT_VALUE => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_IB_STRIP_CUT_VALUE }
implement_stream_object! { unsafe D3D12_STREAM_OUTPUT_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_STREAM_OUTPUT }
implement_stream_object! { unsafe D3D12_VIEW_INSTANCING_DESC => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_VIEW_INSTANCING }

pub(super) struct RenderPipelineStateStream<'a> {
    bytes: Vec<u8>,
    _marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> RenderPipelineStateStream<'a> {
    fn new() -> Self {

        let size_of_stream_desc = size_of::<RenderPipelineStateStreamDesc>();
        let members = 20;
        let capacity = size_of_stream_desc + members * 8;
        Self {
            bytes: Vec::with_capacity(capacity),
            _marker: core::marker::PhantomData,
        }
    }

    fn align_to(&mut self, alignment: usize) {
        let aligned_length = self.bytes.len().next_multiple_of(alignment);
        self.bytes.resize(aligned_length, 0);
    }

    fn add_object<T: RenderPipelineStreamObject>(&mut self, object: T) {

        self.align_to(8);

        let tag: u32 = T::SUBOBJECT_TYPE.0 as u32;
        self.bytes.extend_from_slice(&tag.to_ne_bytes());

        self.align_to(align_of_val::<T>(&object));

        let data_ptr: *const T = &object;
        let data_u8_ptr: *const u8 = data_ptr.cast::<u8>();
        let data_size = size_of_val::<T>(&object);
        let slice = unsafe { core::slice::from_raw_parts::<u8>(data_u8_ptr, data_size) };
        self.bytes.extend_from_slice(slice);
    }

    pub unsafe fn create_pipeline_state(
        &mut self,
        device: &ID3D12Device2,
    ) -> windows::core::Result<ID3D12PipelineState> {
        let stream_desc = D3D12_PIPELINE_STATE_STREAM_DESC {
            SizeInBytes: self.bytes.len(),
            pPipelineStateSubobjectStream: self.bytes.as_mut_ptr().cast(),
        };

        unsafe { device.CreatePipelineState(&stream_desc) }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct RenderPipelineStateStreamDesc<'a> {
    pub root_signature: Option<&'a ID3D12RootSignature>,
    pub pixel_shader: D3D12_SHADER_BYTECODE,
    pub blend_state: D3D12_BLEND_DESC,
    pub sample_mask: u32,
    pub rasterizer_state: D3D12_RASTERIZER_DESC,
    pub depth_stencil_state: D3D12_DEPTH_STENCIL_DESC,
    pub primitive_topology_type: D3D12_PRIMITIVE_TOPOLOGY_TYPE,
    pub rtv_formats: D3D12_RT_FORMAT_ARRAY,
    pub dsv_format: DXGI_FORMAT,
    pub sample_desc: DXGI_SAMPLE_DESC,
    pub node_mask: u32,
    pub cached_pso: D3D12_CACHED_PIPELINE_STATE,
    pub flags: D3D12_PIPELINE_STATE_FLAGS,
    pub view_instancing: Option<D3D12_VIEW_INSTANCING_DESC>,

    pub vertex_shader: D3D12_SHADER_BYTECODE,
    pub input_layout: D3D12_INPUT_LAYOUT_DESC,
    pub index_buffer_strip_cut_value: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE,
    pub stream_output: D3D12_STREAM_OUTPUT_DESC,

    pub task_shader: D3D12_SHADER_BYTECODE,
    pub mesh_shader: D3D12_SHADER_BYTECODE,
}

impl RenderPipelineStateStreamDesc<'_> {
    pub fn to_stream(&self) -> RenderPipelineStateStream<'_> {
        let mut stream = RenderPipelineStateStream::new();

        let root_sig_pointer = self
            .root_signature
            .map(|a| NonNull::new(a.as_raw()).unwrap());

        stream.add_object(RootSignature(root_sig_pointer));

        stream.add_object(self.blend_state);
        stream.add_object(SampleMask(self.sample_mask));
        stream.add_object(self.rasterizer_state);
        stream.add_object(self.depth_stencil_state);
        stream.add_object(self.primitive_topology_type);
        if self.rtv_formats.NumRenderTargets != 0 {
            stream.add_object(self.rtv_formats);
        }
        if self.dsv_format != DXGI_FORMAT_UNKNOWN {
            stream.add_object(self.dsv_format);
        }
        stream.add_object(self.sample_desc);
        if self.node_mask != 0 {
            stream.add_object(NodeMask(self.node_mask));
        }
        if !self.cached_pso.pCachedBlob.is_null() {
            stream.add_object(self.cached_pso);
        }
        stream.add_object(self.flags);
        if let Some(view_instancing) = self.view_instancing {
            stream.add_object(view_instancing);
        }
        if !self.pixel_shader.pShaderBytecode.is_null() {
            stream.add_object(PixelShader(self.pixel_shader));
        }
        if !self.vertex_shader.pShaderBytecode.is_null() {
            stream.add_object(VertexShader(self.vertex_shader));
            stream.add_object(self.input_layout);
            stream.add_object(self.index_buffer_strip_cut_value);
            stream.add_object(self.stream_output);
        }
        if !self.task_shader.pShaderBytecode.is_null() {
            stream.add_object(TaskShader(self.task_shader));
        }
        if !self.mesh_shader.pShaderBytecode.is_null() {
            stream.add_object(MeshShader(self.mesh_shader));
        }

        stream
    }

    pub unsafe fn to_graphics_pipeline_descriptor(&self) -> D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: if let Some(rsig) = self.root_signature {
                unsafe { borrow_interface_temporarily(rsig) }
            } else {
                ManuallyDrop::new(None)
            },
            VS: self.vertex_shader,
            PS: self.pixel_shader,
            DS: D3D12_SHADER_BYTECODE::default(),
            HS: D3D12_SHADER_BYTECODE::default(),
            GS: D3D12_SHADER_BYTECODE::default(),
            StreamOutput: self.stream_output,
            BlendState: self.blend_state,
            SampleMask: self.sample_mask,
            RasterizerState: self.rasterizer_state,
            DepthStencilState: self.depth_stencil_state,
            InputLayout: self.input_layout,
            IBStripCutValue: self.index_buffer_strip_cut_value,
            PrimitiveTopologyType: self.primitive_topology_type,
            NumRenderTargets: self.rtv_formats.NumRenderTargets,
            RTVFormats: self.rtv_formats.RTFormats,
            DSVFormat: self.dsv_format,
            SampleDesc: self.sample_desc,
            NodeMask: self.node_mask,
            CachedPSO: self.cached_pso,
            Flags: self.flags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrappers() {
        assert_eq!(size_of::<RootSignature>(), size_of::<ID3D12RootSignature>());
        assert_eq!(
            align_of::<RootSignature>(),
            align_of::<ID3D12RootSignature>()
        )
    }

    implement_stream_object!(unsafe u16 => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE(1));
    implement_stream_object!(unsafe u32 => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE(2));
    implement_stream_object!(unsafe u64 => D3D12_PIPELINE_STATE_SUBOBJECT_TYPE(3));

    #[test]
    fn stream() {
        let mut stream = RenderPipelineStateStream::new();

        stream.add_object(42u16);
        stream.add_object(84u32);
        stream.add_object(168u64);

        assert_eq!(stream.bytes.len(), 32);

        assert_eq!(&stream.bytes[0..4], &1u32.to_ne_bytes());

        assert_eq!(&stream.bytes[4..6], &42u16.to_ne_bytes());

        assert_eq!(&stream.bytes[6..8], &[0, 0]);

        assert_eq!(&stream.bytes[8..12], &2u32.to_ne_bytes());

        assert_eq!(&stream.bytes[12..16], &84u32.to_ne_bytes());

        assert_eq!(&stream.bytes[16..20], &3u32.to_ne_bytes());

        assert_eq!(&stream.bytes[20..24], &[0, 0, 0, 0]);

        assert_eq!(&stream.bytes[24..32], &168u64.to_ne_bytes());
    }
}
