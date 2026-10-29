use super::RenderQueue;
use crate::render_resource::{
    BindGroup, BindGroupLayout, Buffer, ComputePipeline, RawRenderPipelineDescriptor,
    RenderPipeline, Sampler, Texture,
};
use crate::renderer::WgpuWrapper;
use bevy_ecs::resource::Resource;
use wgpu::{
    util::DeviceExt, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BufferAsyncError, BufferBindingType, PollError, PollStatus,
};

#[derive(Resource, Clone)]
pub struct RenderDevice {
    device: WgpuWrapper<wgpu::Device>,
}

impl From<wgpu::Device> for RenderDevice {
    fn from(device: wgpu::Device) -> Self {
        Self::new(WgpuWrapper::new(device))
    }
}

impl RenderDevice {
    pub fn new(device: WgpuWrapper<wgpu::Device>) -> Self {
        Self { device }
    }

    #[inline]
    pub fn features(&self) -> wgpu::Features {
        self.device.features()
    }

    #[inline]
    pub fn limits(&self) -> wgpu::Limits {
        self.device.limits()
    }

    #[inline]
    pub unsafe fn create_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor,
    ) -> wgpu::ShaderModule {
        #[cfg(feature = "spirv_shader_passthrough")]
        match &desc.source {
            wgpu::ShaderSource::SpirV(source)
                if self
                    .features()
                    .contains(wgpu::Features::PASSTHROUGH_SHADERS) =>
            {

                unsafe {
                    self.device.create_shader_module_passthrough(
                        wgpu::ShaderModuleDescriptorPassthrough {
                            label: desc.label,
                            spirv: Some(source.clone()),
                            ..Default::default()
                        },
                    )
                }
            }

            _ => unsafe {
                self.device
                    .create_shader_module_trusted(desc, wgpu::ShaderRuntimeChecks::unchecked())
            },
        }
        #[cfg(not(feature = "spirv_shader_passthrough"))]

        unsafe {
            self.device
                .create_shader_module_trusted(desc, wgpu::ShaderRuntimeChecks::unchecked())
        }
    }

    #[inline]
    pub fn create_and_validate_shader_module(
        &self,
        desc: wgpu::ShaderModuleDescriptor,
    ) -> wgpu::ShaderModule {
        #[cfg(feature = "spirv_shader_passthrough")]
        match &desc.source {
            wgpu::ShaderSource::SpirV(_source) => panic!("no safety checks are performed for spirv shaders. use `create_shader_module` instead"),
            _ => self.device.create_shader_module(desc),
        }
        #[cfg(not(feature = "spirv_shader_passthrough"))]
        self.device.create_shader_module(desc)
    }

    #[inline]
    pub fn poll(&self, maintain: wgpu::PollType) -> Result<PollStatus, PollError> {
        self.device.poll(maintain)
    }

    #[inline]
    pub fn create_command_encoder(
        &self,
        desc: &wgpu::CommandEncoderDescriptor,
    ) -> wgpu::CommandEncoder {
        self.device.create_command_encoder(desc)
    }

    #[inline]
    pub fn create_render_bundle_encoder(
        &self,
        desc: &wgpu::RenderBundleEncoderDescriptor,
    ) -> wgpu::RenderBundleEncoder<'_> {
        self.device.create_render_bundle_encoder(desc)
    }

    #[inline]
    pub fn create_bind_group<'a>(
        &self,
        label: impl Into<wgpu::Label<'a>>,
        layout: &'a BindGroupLayout,
        entries: &'a [BindGroupEntry<'a>],
    ) -> BindGroup {
        let wgpu_bind_group = self.device.create_bind_group(&BindGroupDescriptor {
            label: label.into(),
            layout,
            entries,
        });
        BindGroup::from(wgpu_bind_group)
    }

    #[inline]
    pub fn create_bind_group_layout<'a>(
        &self,
        label: impl Into<wgpu::Label<'a>>,
        entries: &'a [BindGroupLayoutEntry],
    ) -> BindGroupLayout {
        BindGroupLayout::from(
            self.device
                .create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: label.into(),
                    entries,
                }),
        )
    }

    #[inline]
    pub fn create_pipeline_layout(
        &self,
        desc: &wgpu::PipelineLayoutDescriptor,
    ) -> wgpu::PipelineLayout {
        self.device.create_pipeline_layout(desc)
    }

    #[inline]
    pub fn create_render_pipeline(&self, desc: &RawRenderPipelineDescriptor) -> RenderPipeline {
        let wgpu_render_pipeline = self.device.create_render_pipeline(desc);
        RenderPipeline::from(wgpu_render_pipeline)
    }

    #[inline]
    pub fn create_compute_pipeline(
        &self,
        desc: &wgpu::ComputePipelineDescriptor,
    ) -> ComputePipeline {
        let wgpu_compute_pipeline = self.device.create_compute_pipeline(desc);
        ComputePipeline::from(wgpu_compute_pipeline)
    }

    pub fn create_buffer(&self, desc: &wgpu::BufferDescriptor) -> Buffer {
        let wgpu_buffer = self.device.create_buffer(desc);
        Buffer::from(wgpu_buffer)
    }

    pub fn create_buffer_with_data(&self, desc: &wgpu::util::BufferInitDescriptor) -> Buffer {
        let wgpu_buffer = self.device.create_buffer_init(desc);
        Buffer::from(wgpu_buffer)
    }

    pub fn create_texture_with_data(
        &self,
        render_queue: &RenderQueue,
        desc: &wgpu::TextureDescriptor,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> Texture {
        let wgpu_texture =
            self.device
                .create_texture_with_data(render_queue.as_ref(), desc, order, data);
        Texture::from(wgpu_texture)
    }

    pub fn create_texture(&self, desc: &wgpu::TextureDescriptor) -> Texture {
        let wgpu_texture = self.device.create_texture(desc);
        Texture::from(wgpu_texture)
    }

    pub fn create_sampler(&self, desc: &wgpu::SamplerDescriptor) -> Sampler {
        let wgpu_sampler = self.device.create_sampler(desc);
        Sampler::from(wgpu_sampler)
    }

    pub fn configure_surface(&self, surface: &wgpu::Surface, config: &wgpu::SurfaceConfiguration) {
        surface.configure(&self.device, config);
    }

    pub fn wgpu_device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn map_buffer(
        &self,
        buffer: &wgpu::BufferSlice,
        map_mode: wgpu::MapMode,
        callback: impl FnOnce(Result<(), BufferAsyncError>) + Send + 'static,
    ) {
        buffer.map_async(map_mode, callback);
    }

    pub const fn align_copy_bytes_per_row(row_bytes: usize) -> usize {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;

        let over_aligned = row_bytes + align - 1;

        (over_aligned / align) * align
    }

    pub fn get_supported_read_only_binding_type(
        &self,
        buffers_per_shader_stage: u32,
    ) -> BufferBindingType {
        if self.limits().max_storage_buffers_per_shader_stage >= buffers_per_shader_stage {
            BufferBindingType::Storage { read_only: true }
        } else {
            BufferBindingType::Uniform
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_copy_bytes_per_row() {

        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;

        assert_eq!(RenderDevice::align_copy_bytes_per_row(0), 0);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(1), align);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(align + 1), align * 2);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(align), align);
    }
}
