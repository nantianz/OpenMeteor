use super::*;
use bevy::ecs::system::SystemParamItem;
use bevy::render::render_resource::{
    binding_types::uniform_buffer, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry,
    OwnedBindingResource, ShaderStages, UniformBuffer, UnpreparedBindGroup,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn new_uniform_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn set_color(
    materials: &mut Assets<PonEfMaterial>,
    handle: &Handle<PonEfMaterial>,
    color: Vec4,
) {
    if let Some(material) = materials.get_mut_untracked(handle) {
        material.params.color = color;
    }
}

pub(super) fn set_particle_color(
    materials: &mut Assets<PonEfMaterial>,
    particle: &PonEfParticle,
    color: Vec4,
) {
    if let Some(material) = materials.get_mut_untracked(&particle.material) {
        if let Some(colors) = &mut material.particle_colors {
            colors[particle.color_slot as usize] = color;
        }
    }
}

#[derive(Resource, Default)]
struct ExtractedParams {
    values: Vec<(u64, PonEfMaterialParams)>,
    live: HashSet<u64>,
    particles: HashMap<u64, ParamsPage>,
    particle_materials: HashSet<bevy::asset::UntypedAssetId>,
}

pub(super) const PARAMS_PER_PAGE: usize = 128;

#[derive(ShaderType, Clone)]
struct ParamsPage {
    values: [PonEfMaterialParams; PARAMS_PER_PAGE],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    page: usize,
    index: usize,
}

struct Page {
    uniform: UniformBuffer<ParamsPage>,
    free: Vec<usize>,
    dirty: bool,
}

impl Page {
    fn new() -> Self {
        Self {
            uniform: UniformBuffer::from(ParamsPage {
                values: std::array::from_fn(|_| PonEfMaterialParams {
                    color: Vec4::ZERO,
                    has_texture: 0,
                }),
            }),
            free: (0..PARAMS_PER_PAGE).rev().collect(),
            dirty: true,
        }
    }
}

#[derive(Resource, Default)]
pub struct ParamBuffers {
    locations: HashMap<u64, Slot>,
    pages: Vec<Option<Page>>,

    selectors: HashMap<usize, UniformBuffer<UVec4>>,
    particles: HashMap<u64, UniformBuffer<ParamsPage>>,
}

impl ParamBuffers {
    #[cfg(test)]
    pub(super) fn particle_page_count(&self) -> usize {
        self.particles.len()
    }
    fn allocate(&mut self, id: u64) -> Slot {
        if let Some(slot) = self.locations.get(&id) {
            return *slot;
        }
        let page = self
            .pages
            .iter()
            .position(|page| page.as_ref().is_some_and(|page| !page.free.is_empty()))
            .unwrap_or_else(|| {
                if let Some(index) = self.pages.iter().position(Option::is_none) {
                    self.pages[index] = Some(Page::new());
                    index
                } else {
                    self.pages.push(Some(Page::new()));
                    self.pages.len() - 1
                }
            });
        let index = self.pages[page].as_mut().unwrap().free.pop().unwrap();
        let slot = Slot { page, index };
        self.locations.insert(id, slot);
        slot
    }
}

#[derive(AsBindGroup)]
pub struct Textures {
    #[texture(1)]
    #[sampler(2)]
    texture: Option<Handle<Image>>,
}

impl AsBindGroup for PonEfMaterial {
    type Data = PonEfMaterialKey;
    type Param = (<Textures as AsBindGroup>::Param, Res<'static, ParamBuffers>);

    fn label() -> &'static str {
        "PonEF persistent instance uniforms"
    }

    fn bind_group_data(&self) -> Self::Data {
        self.into()
    }

    fn unprepared_bind_group(
        &self,
        layout: &BindGroupLayout,
        device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let (textures, buffers) = param;
        let (selector_index, page) = if self.particle_colors.is_some() {
            (
                0,
                buffers
                    .particles
                    .get(&self.uniform_id)
                    .and_then(|u| u.buffer()),
            )
        } else {
            let slot = buffers
                .locations
                .get(&self.uniform_id)
                .ok_or(AsBindGroupError::RetryNextUpdate)?;
            (
                slot.index,
                buffers
                    .pages
                    .get(slot.page)
                    .and_then(Option::as_ref)
                    .and_then(|page| page.uniform.buffer()),
            )
        };
        let page = page.ok_or(AsBindGroupError::RetryNextUpdate)?;
        let selector = buffers
            .selectors
            .get(&selector_index)
            .and_then(|u| u.buffer())
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let mut result = Textures {
            texture: self.texture.clone(),
        }
        .unprepared_bind_group(layout, device, textures, force_no_bindless)?;
        result
            .bindings
            .push((0, OwnedBindingResource::Buffer(selector.clone())));
        result
            .bindings
            .push((3, OwnedBindingResource::Buffer(page.clone())));
        #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
        crate::plugins::perf_plugin::count_event(5);
        Ok(result)
    }

    fn bind_group_layout_entries(
        device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let mut entries = Textures::bind_group_layout_entries(device, force_no_bindless);
        entries.push(uniform_buffer::<UVec4>(false).build(0, ShaderStages::FRAGMENT));
        entries.push(uniform_buffer::<ParamsPage>(false).build(3, ShaderStages::VERTEX_FRAGMENT));
        entries
    }
}

pub(super) fn install(app: &mut App) {
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<ExtractedParams>()
            .init_resource::<ParamBuffers>()
            .add_systems(ExtractSchedule, extract_params)
            .add_systems(
                Render,
                refresh_particle_sort_centers
                    .after(RenderSystems::Queue)
                    .before(RenderSystems::PhaseSort),
            )
            .add_systems(
                Render,
                upload_params
                    .after(RenderSystems::ExtractCommands)
                    .before(RenderSystems::PrepareAssets),
            );
    }
}

fn extract_params(
    mut extracted: ResMut<ExtractedParams>,
    materials: Extract<Res<Assets<PonEfMaterial>>>,
) {
    extracted.values.clear();
    extracted.live.clear();
    extracted.particles.clear();
    extracted.particle_materials.clear();
    for (id, material) in materials.iter() {
        if let Some(colors) = &material.particle_colors {
            extracted.particle_materials.insert(id.untyped());
            extracted.particles.insert(
                material.uniform_id,
                ParamsPage {
                    values: std::array::from_fn(|i| PonEfMaterialParams {
                        color: colors[i],
                        has_texture: material.params.has_texture,
                    }),
                },
            );
            continue;
        }
        extracted
            .values
            .push((material.uniform_id, material.params.clone()));
        extracted.live.insert(material.uniform_id);
    }
}

fn refresh_particle_sort_centers(
    extracted: Res<ExtractedParams>,
    materials: Res<bevy::pbr::RenderMaterialInstances>,
    meshes: Res<bevy::pbr::RenderMeshInstances>,
    gpu_instances: Option<
        Res<
            bevy::render::batching::gpu_preprocessing::BatchedInstanceBuffers<
                bevy::pbr::MeshUniform,
                bevy::pbr::MeshInputUniform,
            >,
        >,
    >,
    mut phases: ResMut<
        bevy::render::render_phase::ViewSortedRenderPhases<
            bevy::core_pipeline::core_3d::Transparent3d,
        >,
    >,
) {
    if extracted.particle_materials.is_empty() {
        return;
    }
    for phase in phases.values_mut() {
        for item in phase.items.values_mut() {
            let entity = item.entity.1;
            if !materials
                .instances
                .get(&entity)
                .is_some_and(|m| extracted.particle_materials.contains(&m.asset_id))
            {
                continue;
            }
            let Some(mesh) = meshes.render_mesh_queue_data(entity) else {
                continue;
            };
            let world = bevy::pbr::get_mesh_instance_world_from_local(
                entity,
                mesh.current_uniform_index,
                &meshes,
                gpu_instances.as_deref(),
            );
            if let bevy::core_pipeline::core_3d::TransparentSortingInfo3d::Sorted {
                mesh_center,
                ..
            } = &mut item.sorting_info
            {
                *mesh_center = world.transform_point3(Vec3::ZERO);
            }
        }
    }
}

fn upload_params(
    extracted: Res<ExtractedParams>,
    mut buffers: ResMut<ParamBuffers>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let buffers = &mut *buffers;
    buffers
        .particles
        .retain(|id, _| extracted.particles.contains_key(id));
    if !extracted.particles.is_empty() {
        buffers.selectors.entry(0).or_insert_with(|| {
            let mut selector = UniformBuffer::from(UVec4::ZERO);
            selector.write_buffer(&device, &queue);
            selector
        });
    }
    for (id, page) in &extracted.particles {
        let buffer = buffers
            .particles
            .entry(*id)
            .or_insert_with(|| UniformBuffer::from(page.clone()));
        if buffer.buffer().is_none() || buffer.get().values != page.values {
            #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
            if buffer.buffer().is_none() {
                crate::plugins::perf_plugin::count_event(6);
            }
            *buffer.get_mut() = page.clone();
            buffer.write_buffer(&device, &queue);
            #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
            crate::plugins::perf_plugin::count_event(7);
        }
    }

    let pages = &mut buffers.pages;
    buffers.locations.retain(|id, slot| {
        if extracted.live.contains(id) {
            return true;
        }
        pages[slot.page].as_mut().unwrap().free.push(slot.index);
        false
    });
    for (id, params) in &extracted.values {
        let slot = buffers.allocate(*id);
        buffers.selectors.entry(slot.index).or_insert_with(|| {
            let mut selector = UniformBuffer::from(UVec4::new(slot.index as u32, 0, 0, 0));
            selector.write_buffer(&device, &queue);
            #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
            {
                crate::plugins::perf_plugin::count_event(6);
                crate::plugins::perf_plugin::count_event(7);
            }
            selector
        });
        let page = buffers.pages[slot.page].as_mut().unwrap();
        if page.uniform.get().values[slot.index] != *params {
            page.uniform.get_mut().values[slot.index] = params.clone();
            page.dirty = true;
        }
    }
    for page in &mut buffers.pages {
        let Some(active) = page.as_mut() else {
            continue;
        };
        if active.free.len() == PARAMS_PER_PAGE {
            *page = None;
            continue;
        }
        if active.dirty {
            #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
            if active.uniform.buffer().is_none() {
                crate::plugins::perf_plugin::count_event(6);
            }
            active.uniform.write_buffer(&device, &queue);
            active.dirty = false;
            #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
            crate::plugins::perf_plugin::count_event(7);
        }
    }
    while buffers.pages.last().is_some_and(Option::is_none) {
        buffers.pages.pop();
    }
    if buffers.locations.is_empty() && buffers.particles.is_empty() {
        buffers.selectors.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rendered_params(app: &App, ids: &[u64]) {
        use bevy::render::render_resource::*;
        let device = app.world().resource::<RenderDevice>();
        let queue = app.world().resource::<RenderQueue>();
        let buffers = app.world().resource::<ParamBuffers>();
        let source = include_str!("../pon_ef_material.wgsl")
            .replace("#import bevy_pbr::forward_io::VertexOutput", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, };")
            .replace("#{MATERIAL_BIND_GROUP}", "0")
            + "\n@vertex fn test_vertex(@builtin(vertex_index) i: u32) -> VertexOutput {\n\
                let positions = array<vec2<f32>, 3>(vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0));\n\
                var o: VertexOutput; o.position=vec4<f32>(positions[i],0.0,1.0); o.uv=vec2<f32>(0.5); return o; }";
        let shader = device.create_and_validate_shader_module(ShaderModuleDescriptor {
            label: None,
            source: ShaderSource::Wgsl(source.into()),
        });
        let layout = PonEfMaterial::bind_group_layout(device);
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RawRenderPipelineDescriptor {
            label: Some("PonEF page readback"),
            layout: Some(&pipeline_layout),
            vertex: RawVertexState {
                module: &shader,
                entry_point: Some("test_vertex"),
                compilation_options: default(),
                buffers: &[],
            },
            primitive: default(),
            depth_stencil: None,
            multisample: default(),
            fragment: Some(RawFragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: default(),
                targets: &[Some(ColorTargetState {
                    format: TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let texture_desc = TextureDescriptor {
            label: None,
            size: Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let texture = device.create_texture_with_data(
            queue,
            &texture_desc,
            TextureDataOrder::LayerMajor,
            &[255, 128, 64, 255],
        );
        let texture_view = texture.create_view(&default());
        let sampler = device.create_sampler(&default());
        let target = device.create_texture(&TextureDescriptor {
            size: Extent3d {
                width: ids.len() as u32,
                ..texture_desc.size
            },
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            ..texture_desc
        });
        let target_view = target.create_view(&default());
        let groups: Vec<_> = ids
            .iter()
            .map(|id| {
                let slot = buffers.locations[id];
                device.create_bind_group(
                    None,
                    &layout,
                    &[
                        BindGroupEntry {
                            binding: 0,
                            resource: buffers.selectors[&slot.index]
                                .buffer()
                                .unwrap()
                                .as_entire_binding(),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::TextureView(&texture_view),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: BindingResource::Sampler(&sampler),
                        },
                        BindGroupEntry {
                            binding: 3,
                            resource: buffers.pages[slot.page]
                                .as_ref()
                                .unwrap()
                                .uniform
                                .buffer()
                                .unwrap()
                                .as_entire_binding(),
                        },
                    ],
                )
            })
            .collect();
        let row_bytes = (ids.len() as u64 * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&BufferDescriptor {
            label: None,
            size: row_bytes,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(default()),
                        store: StoreOp::Store,
                    },
                })],
                ..default()
            });
            pass.set_pipeline(&pipeline);
            for (index, group) in groups.iter().enumerate() {
                pass.set_viewport(index as f32, 0.0, 1.0, 1.0, 0.0, 1.0);
                pass.set_scissor_rect(index as u32, 0, 1, 1);
                pass.set_bind_group(0, &**group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &readback,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes as u32),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(MapMode::Read, move |result| tx.send(result).unwrap());
        device
            .poll(PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let pixels = readback.slice(..).get_mapped_range();
        for (index, id) in ids.iter().enumerate() {
            let params = &app
                .world()
                .resource::<ExtractedParams>()
                .values
                .iter()
                .find(|(key, _)| key == id)
                .unwrap()
                .1;
            let texel = if params.has_texture == 1 {
                Vec4::new(1.0, 128.0 / 255.0, 64.0 / 255.0, 1.0)
            } else {
                Vec4::ONE
            };
            for (channel, expected) in (params.color * texel * 255.0).to_array().iter().enumerate()
            {
                assert!(
                    (pixels[index * 4 + channel] as f32 - expected).abs() <= 1.0,
                    "id={id} channel={channel}: pixel={} expected={expected}",
                    pixels[index * 4 + channel]
                );
            }
        }
        drop(pixels);
        readback.unmap();
    }

    #[test]
    fn gpu_pages_keep_live_bindings_and_recycle_bursts_without_stale_colors() {
        use bevy::render::settings::{Backends, WgpuSettings};
        let resources = bevy::tasks::block_on(bevy::render::renderer::initialize_renderer(
            Backends::PRIMARY,
            None,
            &WgpuSettings::default(),
            #[cfg(all(
                feature = "dlss5",
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc"
            ))]
            Default::default(),
        ));
        let mut app = App::new();
        app.insert_resource(resources.0)
            .insert_resource(resources.1)
            .init_resource::<ExtractedParams>()
            .init_resource::<ParamBuffers>()
            .add_systems(Update, upload_params);
        let mut ids: Vec<_> = (0..260).map(|_| new_uniform_id()).collect();
        for burst in 0..12 {
            if burst != 0 {
                for (i, id) in ids.iter_mut().enumerate() {
                    if i != 127 && i != 128 {
                        *id = new_uniform_id();
                    }
                }
            }
            let previous: Vec<_> = [127, 128]
                .into_iter()
                .filter_map(|i| {
                    let buffers = app.world().resource::<ParamBuffers>();
                    let slot = *buffers.locations.get(&ids[i])?;
                    Some((
                        ids[i],
                        slot,
                        buffers.pages[slot.page]
                            .as_ref()
                            .unwrap()
                            .uniform
                            .buffer()
                            .unwrap()
                            .id(),
                    ))
                })
                .collect();
            *app.world_mut().resource_mut::<ExtractedParams>() = ExtractedParams {
                particles: HashMap::new(),
                particle_materials: HashSet::new(),
                live: ids.iter().copied().collect(),
                values: ids
                    .iter()
                    .enumerate()
                    .map(|(i, id)| {
                        (
                            *id,
                            PonEfMaterialParams {
                                color: Vec4::new(
                                    (i % 5) as f32 / 4.0,
                                    burst as f32 / 12.0,
                                    (i % 3) as f32 / 2.0,
                                    0.25 + burst as f32 / 24.0,
                                ),
                                has_texture: ((i + burst) % 2) as u32,
                            },
                        )
                    })
                    .collect(),
            };
            app.update();
            let buffers = app.world().resource::<ParamBuffers>();
            assert_eq!(buffers.locations.len(), 260);
            assert_eq!(buffers.pages.len(), 3);
            assert_eq!(buffers.selectors.len(), PARAMS_PER_PAGE);
            for (id, slot, buffer_id) in previous {
                assert_eq!(
                    buffers.locations[&id], slot,
                    "live bind groups must not move"
                );
                assert_eq!(
                    buffers.pages[slot.page]
                        .as_ref()
                        .unwrap()
                        .uniform
                        .buffer()
                        .unwrap()
                        .id(),
                    buffer_id
                );
            }
            if [0, 1, 11].contains(&burst) {
                assert_rendered_params(&app, &ids);
            }
        }

        let survivor = ids[128];
        let slot = app.world().resource::<ParamBuffers>().locations[&survivor];
        {
            let mut extracted = app.world_mut().resource_mut::<ExtractedParams>();
            extracted.live = HashSet::from([survivor]);
            extracted.values.retain(|(id, _)| *id == survivor);
        }
        app.update();
        let buffers = app.world().resource::<ParamBuffers>();
        assert_eq!(buffers.locations[&survivor], slot);
        assert_eq!(buffers.pages.iter().flatten().count(), 1);
        assert_rendered_params(&app, &[survivor]);
        *app.world_mut().resource_mut::<ExtractedParams>() = ExtractedParams::default();
        app.update();
        let buffers = app.world().resource::<ParamBuffers>();
        assert!(buffers.pages.is_empty() && buffers.selectors.is_empty());
    }

    #[test]
    fn tint_updates_skip_asset_reprepare_but_ifl_texture_changes_notify() {
        use bevy::asset::{AssetApp, AssetEvent, AssetPlugin};
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()))
            .init_asset::<PonEfMaterial>()
            .init_resource::<Time>()
            .add_systems(Update, tick_ifl_animations);
        let first = bevy::asset::uuid_handle!("00000000-0000-0000-0000-000000000101");
        let second = bevy::asset::uuid_handle!("00000000-0000-0000-0000-000000000102");
        let (red, green, key) = {
            let mut materials = app.world_mut().resource_mut::<Assets<PonEfMaterial>>();
            let template = materials.add(PonEfMaterial {
                uniform_id: new_uniform_id(),
                particle_colors: None,
                params: PonEfMaterialParams {
                    color: Vec4::ONE,
                    has_texture: 1,
                },
                texture: Some(first.clone()),
                blend: PonEfBlendMode::SrcAlphaOne,
                two_sided: true,
            });
            let red = clone_effect_material(&mut materials, &template);
            let green = clone_effect_material(&mut materials, &template);
            let key = materials.get(&red).unwrap().uniform_id;
            assert_ne!(key, materials.get(&green).unwrap().uniform_id);
            assert_ne!(key, materials.get(&template).unwrap().uniform_id);
            assert_eq!(
                key,
                materials.get(&red).unwrap().clone().uniform_id,
                "render extraction clones must keep the instance key"
            );
            (red, green, key)
        };
        app.update();
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<PonEfMaterial>>>()
            .clear();
        for i in 0..20 {
            let color = Vec4::new(1.0, 0.0, 0.0, i as f32 / 20.0);
            set_color(
                &mut app.world_mut().resource_mut::<Assets<PonEfMaterial>>(),
                &red,
                color,
            );
            app.update();
            let modified = app
                .world_mut()
                .resource_mut::<Messages<AssetEvent<PonEfMaterial>>>()
                .drain()
                .any(|event| event.is_modified(red.id()));
            assert!(
                !modified,
                "keyframe alpha must not rebuild material bindings"
            );
            let materials = app.world().resource::<Assets<PonEfMaterial>>();
            assert_eq!(materials.get(&red).unwrap().params.color, color);
            assert_eq!(materials.get(&green).unwrap().params.color, Vec4::ONE);
        }
        app.world_mut().spawn(IflAnimated {
            material: red.clone(),
            frames: vec![first.clone(), first, second.clone()],
            index: 0,
            age: 0.0,
        });
        for expected in [false, true] {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
            app.update();
            let modified = app
                .world_mut()
                .resource_mut::<Messages<AssetEvent<PonEfMaterial>>>()
                .drain()
                .any(|event| event.is_modified(red.id()));
            assert_eq!(
                modified, expected,
                "only a different IFL frame changes the texture binding"
            );
        }
        let material = app
            .world()
            .resource::<Assets<PonEfMaterial>>()
            .get(&red)
            .unwrap();
        assert_eq!(material.texture.as_ref(), Some(&second));
        assert_eq!(
            material.uniform_id, key,
            "IFL must reuse its parameter buffer"
        );
    }

    #[test]
    fn gpu_instance_buffers_reuse_uploads_and_release_removed_materials() {
        use bevy::render::settings::{Backends, WgpuSettings};
        let resources = bevy::tasks::block_on(bevy::render::renderer::initialize_renderer(
            Backends::PRIMARY,
            None,
            &WgpuSettings::default(),
            #[cfg(all(
                feature = "dlss5",
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc"
            ))]
            Default::default(),
        ));
        let device = resources.0.clone();
        let queue = resources.1.clone();
        let mut app = App::new();
        app.insert_resource(device.clone())
            .insert_resource(queue)
            .init_resource::<ExtractedParams>()
            .init_resource::<ParamBuffers>()
            .add_systems(Update, upload_params);
        let keys = [new_uniform_id(), new_uniform_id()];
        for tick in 0..60 {
            {
                let mut extracted = app.world_mut().resource_mut::<ExtractedParams>();
                extracted.live = HashSet::from(keys);
                extracted.values = vec![
                    (
                        keys[0],
                        PonEfMaterialParams {
                            color: Vec4::new(1.0, 0.0, 0.0, tick as f32 / 60.0),
                            has_texture: u32::from(tick > 30),
                        },
                    ),
                    (
                        keys[1],
                        PonEfMaterialParams {
                            color: Vec4::Y,
                            has_texture: 0,
                        },
                    ),
                ];
            }
            let previous = app
                .world()
                .resource::<ParamBuffers>()
                .pages
                .first()
                .and_then(Option::as_ref)
                .and_then(|p| p.uniform.buffer())
                .map(|b| b.id());
            app.update();
            let buffers = app.world().resource::<ParamBuffers>();
            assert_eq!(buffers.locations.len(), 2);
            let first = buffers.locations[&keys[0]];
            let second = buffers.locations[&keys[1]];
            assert_eq!(first.page, second.page);
            assert_ne!(first.index, second.index);
            let page = buffers.pages[first.page].as_ref().unwrap();
            if let Some(previous) = previous {
                assert_eq!(page.uniform.buffer().unwrap().id(), previous);
            }
            assert_eq!(
                page.uniform.get().values[first.index].color.w,
                tick as f32 / 60.0
            );
            assert_eq!(
                page.uniform.get().values[first.index].has_texture,
                u32::from(tick > 30)
            );
            assert_eq!(page.uniform.get().values[second.index].color, Vec4::Y);
        }
        *app.world_mut().resource_mut::<ExtractedParams>() = ExtractedParams::default();
        app.update();
        let buffers = app.world().resource::<ParamBuffers>();
        assert!(
            buffers.locations.is_empty()
                && buffers.pages.is_empty()
                && buffers.selectors.is_empty()
        );

        use bevy::render::render_resource::*;
        let source = include_str!("../pon_ef_material.wgsl")
            .replace("#import bevy_pbr::forward_io::VertexOutput", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, };")
            .replace("#{MATERIAL_BIND_GROUP}", "3")
            + "\n@vertex fn test_vertex() -> VertexOutput { var o: VertexOutput; o.position=vec4<f32>(0.0,0.0,0.0,1.0); o.uv=vec2<f32>(0.0); return o; }";
        let shader = device.create_and_validate_shader_module(ShaderModuleDescriptor {
            label: None,
            source: ShaderSource::Wgsl(source.into()),
        });
        let empty = device.create_bind_group_layout("test empty", &[]);
        let layout = PonEfMaterial::bind_group_layout(&device);
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&empty), Some(&empty), Some(&empty), Some(&layout)],
            immediate_size: 0,
        });
        let _pipeline = device.create_render_pipeline(&RawRenderPipelineDescriptor {
            label: Some("PonEF persistent uniform validation"),
            layout: Some(&pipeline_layout),
            vertex: RawVertexState {
                module: &shader,
                entry_point: Some("test_vertex"),
                compilation_options: default(),
                buffers: &[],
            },
            primitive: default(),
            depth_stencil: None,
            multisample: default(),
            fragment: Some(RawFragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: default(),
                targets: &[Some(ColorTargetState {
                    format: TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
    }
}
