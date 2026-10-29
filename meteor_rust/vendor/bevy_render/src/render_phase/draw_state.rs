use crate::{
    diagnostic::internal::{Pass, PassKind, WritePipelineStatistics, WriteTimestamp},
    render_resource::{
        BindGroup, BindGroupId, Buffer, BufferId, BufferSlice, RenderPipeline, RenderPipelineId,
    },
    renderer::RenderDevice,
};
use bevy_camera::Viewport;
use bevy_color::LinearRgba;
use bevy_utils::default;
use core::ops::Range;
use wgpu::{IndexFormat, QuerySet, RenderPass};

#[cfg(feature = "detailed_trace")]
use bevy_log::trace;

type BufferSliceKey = (BufferId, wgpu::BufferAddress, wgpu::BufferSize);

#[derive(Debug, Default)]
struct DrawState {
    pipeline: Option<RenderPipelineId>,
    bind_groups: Vec<(Option<BindGroupId>, Vec<u32>)>,

    vertex_buffers: Vec<Option<BufferSliceKey>>,
    index_buffer: Option<(BufferSliceKey, IndexFormat)>,

    stores_state: bool,
}

impl DrawState {

    fn set_pipeline(&mut self, pipeline: RenderPipelineId) {

        self.pipeline = Some(pipeline);
        self.stores_state = true;
    }

    fn is_pipeline_set(&self, pipeline: RenderPipelineId) -> bool {
        self.pipeline == Some(pipeline)
    }

    fn set_bind_group(&mut self, index: usize, bind_group: BindGroupId, dynamic_indices: &[u32]) {
        let group = &mut self.bind_groups[index];
        group.0 = Some(bind_group);
        group.1.clear();
        group.1.extend(dynamic_indices);
        self.stores_state = true;
    }

    fn is_bind_group_set(
        &self,
        index: usize,
        bind_group: BindGroupId,
        dynamic_indices: &[u32],
    ) -> bool {
        if let Some(current_bind_group) = self.bind_groups.get(index) {
            current_bind_group.0 == Some(bind_group) && dynamic_indices == current_bind_group.1
        } else {
            false
        }
    }

    fn set_vertex_buffer(&mut self, index: usize, buffer_slice: BufferSlice) {
        self.vertex_buffers[index] = Some(self.buffer_slice_key(&buffer_slice));
        self.stores_state = true;
    }

    fn is_vertex_buffer_set(&self, index: usize, buffer_slice: &BufferSlice) -> bool {
        if let Some(current) = self.vertex_buffers.get(index) {
            *current == Some(self.buffer_slice_key(buffer_slice))
        } else {
            false
        }
    }

    fn buffer_slice_key(&self, buffer_slice: &BufferSlice) -> BufferSliceKey {
        (
            buffer_slice.id(),
            buffer_slice.offset(),
            buffer_slice.size(),
        )
    }

    fn set_index_buffer(&mut self, buffer_slice: &BufferSlice, index_format: IndexFormat) {
        self.index_buffer = Some((self.buffer_slice_key(buffer_slice), index_format));
        self.stores_state = true;
    }

    fn is_index_buffer_set(&self, buffer: &BufferSlice, index_format: IndexFormat) -> bool {
        self.index_buffer == Some((self.buffer_slice_key(buffer), index_format))
    }

    pub fn reset_tracking(&mut self) {
        if !self.stores_state {
            return;
        }
        self.pipeline = None;
        self.bind_groups.iter_mut().for_each(|val| {
            val.0 = None;
            val.1.clear();
        });
        self.vertex_buffers.iter_mut().for_each(|val| {
            *val = None;
        });
        self.index_buffer = None;
        self.stores_state = false;
    }
}

pub struct TrackedRenderPass<'a> {
    pass: RenderPass<'a>,
    state: DrawState,
}

impl<'a> TrackedRenderPass<'a> {

    pub fn new(device: &RenderDevice, pass: RenderPass<'a>) -> Self {
        let limits = device.limits();
        let max_bind_groups = limits.max_bind_groups as usize;
        let max_vertex_buffers = limits.max_vertex_buffers as usize;
        Self {
            state: DrawState {
                bind_groups: vec![(None, Vec::new()); max_bind_groups],
                vertex_buffers: vec![None; max_vertex_buffers],
                ..default()
            },
            pass,
        }
    }

    pub fn wgpu_pass(&mut self) -> &mut RenderPass<'a> {
        self.state.reset_tracking();
        &mut self.pass
    }

    pub fn set_render_pipeline(&mut self, pipeline: &'a RenderPipeline) {
        #[cfg(feature = "detailed_trace")]
        trace!("set pipeline: {:?}", pipeline);
        if self.state.is_pipeline_set(pipeline.id()) {
            return;
        }
        self.pass.set_pipeline(pipeline);
        self.state.set_pipeline(pipeline.id());
    }

    pub fn set_bind_group(
        &mut self,
        index: usize,
        bind_group: &'a BindGroup,
        dynamic_uniform_indices: &[u32],
    ) {
        if self
            .state
            .is_bind_group_set(index, bind_group.id(), dynamic_uniform_indices)
        {
            #[cfg(feature = "detailed_trace")]
            trace!(
                "set bind_group {} (already set): {:?} ({:?})",
                index,
                bind_group,
                dynamic_uniform_indices
            );
            return;
        }
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set bind_group {}: {:?} ({:?})",
            index,
            bind_group,
            dynamic_uniform_indices
        );

        self.pass
            .set_bind_group(index as u32, bind_group, dynamic_uniform_indices);
        self.state
            .set_bind_group(index, bind_group.id(), dynamic_uniform_indices);
    }

    pub fn set_vertex_buffer(&mut self, slot_index: usize, buffer_slice: BufferSlice<'a>) {
        if self.state.is_vertex_buffer_set(slot_index, &buffer_slice) {
            #[cfg(feature = "detailed_trace")]
            trace!(
                "set vertex buffer {} (already set): {:?} (offset = {}, size = {})",
                slot_index,
                buffer_slice.id(),
                buffer_slice.offset(),
                buffer_slice.size(),
            );
            return;
        }
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set vertex buffer {}: {:?} (offset = {}, size = {})",
            slot_index,
            buffer_slice.id(),
            buffer_slice.offset(),
            buffer_slice.size(),
        );

        self.pass
            .set_vertex_buffer(slot_index as u32, *buffer_slice);
        self.state.set_vertex_buffer(slot_index, buffer_slice);
    }

    pub fn set_index_buffer(&mut self, buffer_slice: BufferSlice<'a>, index_format: IndexFormat) {
        let already_set = self.state.is_index_buffer_set(&buffer_slice, index_format);
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set index buffer{}: {:?} (offset = {}, size = {})",
            if already_set { " (already set)" } else { "" },
            buffer_slice.id(),
            buffer_slice.offset(),
            buffer_slice.size(),
        );
        if already_set {
            return;
        }
        self.pass.set_index_buffer(*buffer_slice, index_format);
        self.state.set_index_buffer(&buffer_slice, index_format);
    }

    pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        #[cfg(feature = "detailed_trace")]
        trace!("draw: {:?} {:?}", vertices, instances);
        self.pass.draw(vertices, instances);
    }

    pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "draw indexed: {:?} {} {:?}",
            indices,
            base_vertex,
            instances
        );
        self.pass.draw_indexed(indices, base_vertex, instances);
    }

    pub fn draw_indirect(&mut self, indirect_buffer: &'a Buffer, indirect_offset: u64) {
        #[cfg(feature = "detailed_trace")]
        trace!("draw indirect: {:?} {}", indirect_buffer, indirect_offset);
        self.pass.draw_indirect(indirect_buffer, indirect_offset);
    }

    pub fn draw_indexed_indirect(&mut self, indirect_buffer: &'a Buffer, indirect_offset: u64) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "draw indexed indirect: {:?} {}",
            indirect_buffer,
            indirect_offset
        );
        self.pass
            .draw_indexed_indirect(indirect_buffer, indirect_offset);
    }

    pub fn multi_draw_indirect(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indirect: {:?} {}, {}x",
            indirect_buffer,
            indirect_offset,
            count
        );
        self.pass
            .multi_draw_indirect(indirect_buffer, indirect_offset, count);
    }

    pub fn multi_draw_indirect_count(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count_buffer: &'a Buffer,
        count_offset: u64,
        max_count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indirect count: {:?} {}, ({:?} {})x, max {}x",
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count
        );
        self.pass.multi_draw_indirect_count(
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count,
        );
    }

    pub fn multi_draw_indexed_indirect(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indexed indirect: {:?} {}, {}x",
            indirect_buffer,
            indirect_offset,
            count
        );
        self.pass
            .multi_draw_indexed_indirect(indirect_buffer, indirect_offset, count);
    }

    pub fn multi_draw_indexed_indirect_count(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count_buffer: &'a Buffer,
        count_offset: u64,
        max_count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indexed indirect count: {:?} {}, ({:?} {})x, max {}x",
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count
        );
        self.pass.multi_draw_indexed_indirect_count(
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count,
        );
    }

    pub fn set_stencil_reference(&mut self, reference: u32) {
        #[cfg(feature = "detailed_trace")]
        trace!("set stencil reference: {}", reference);
        self.pass.set_stencil_reference(reference);
    }

    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        #[cfg(feature = "detailed_trace")]
        trace!("set_scissor_rect: {} {} {} {}", x, y, width, height);
        self.pass.set_scissor_rect(x, y, width, height);
    }

    pub fn set_immediates(&mut self, offset: u32, data: &[u8]) {
        #[cfg(feature = "detailed_trace")]
        trace!("set immediates offset: {} data.len: {}", offset, data.len());
        self.pass.set_immediates(offset, data);
    }

    pub fn set_viewport(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set viewport: {} {} {} {} {} {}",
            x,
            y,
            width,
            height,
            min_depth,
            max_depth
        );
        self.pass
            .set_viewport(x, y, width, height, min_depth, max_depth);
    }

    pub fn set_camera_viewport(&mut self, viewport: &Viewport) {
        self.set_viewport(
            viewport.physical_position.x as f32,
            viewport.physical_position.y as f32,
            viewport.physical_size.x as f32,
            viewport.physical_size.y as f32,
            viewport.depth.start,
            viewport.depth.end,
        );
    }

    pub fn insert_debug_marker(&mut self, label: &str) {
        #[cfg(feature = "detailed_trace")]
        trace!("insert debug marker: {}", label);
        self.pass.insert_debug_marker(label);
    }

    pub fn push_debug_group(&mut self, label: &str) {
        #[cfg(feature = "detailed_trace")]
        trace!("push_debug_group marker: {}", label);
        self.pass.push_debug_group(label);
    }

    pub fn pop_debug_group(&mut self) {
        #[cfg(feature = "detailed_trace")]
        trace!("pop_debug_group");
        self.pass.pop_debug_group();
    }

    pub fn set_blend_constant(&mut self, color: LinearRgba) {
        #[cfg(feature = "detailed_trace")]
        trace!("set blend constant: {:?}", color);
        self.pass.set_blend_constant(wgpu::Color::from(color));
    }
}

impl WriteTimestamp for TrackedRenderPass<'_> {
    fn write_timestamp(&mut self, query_set: &QuerySet, index: u32) {
        self.pass.write_timestamp(query_set, index);
    }
}

impl WritePipelineStatistics for TrackedRenderPass<'_> {
    fn begin_pipeline_statistics_query(&mut self, query_set: &QuerySet, index: u32) {
        self.pass.begin_pipeline_statistics_query(query_set, index);
    }

    fn end_pipeline_statistics_query(&mut self) {
        self.pass.end_pipeline_statistics_query();
    }
}

impl Pass for TrackedRenderPass<'_> {
    const KIND: PassKind = PassKind::Render;
}
