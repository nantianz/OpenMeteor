use wgpu::*;

const WGSL: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba16float, write>;
fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c < vec3<f32>(0.0031308));
}
fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c < vec3<f32>(0.04045));
}
@compute @workgroup_size(8, 8, 1)
fn encode(@builtin(global_invocation_id) gid: vec3<u32>) {
    let d = textureDimensions(dst_tex);
    if (gid.x >= d.x || gid.y >= d.y) { return; }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(dst_tex, p, vec4<f32>(clamp(srgb_encode(textureLoad(src_tex, p, 0).rgb), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
@compute @workgroup_size(8, 8, 1)
fn decode(@builtin(global_invocation_id) gid: vec3<u32>) {
    let d = textureDimensions(dst_tex);
    if (gid.x >= d.x || gid.y >= d.y) { return; }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(dst_tex, p, vec4<f32>(srgb_decode(textureLoad(src_tex, p, 0).rgb), 1.0));
}
"#;

pub struct Bridge {
    pub encode: ComputePipeline,
    pub decode: ComputePipeline,
    pub bgl: BindGroupLayout,
    pub proxy_in: Texture,
    pub proxy_in_view: TextureView,
    pub proxy_out: Texture,
    pub proxy_out_view: TextureView,
}

impl Bridge {
    pub fn new(device: &Device, width: u32, height: u32) -> Self {
        let module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("dlss5_bridge"),
            source: ShaderSource::Wgsl(WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("dlss5_bridge_bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: false },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageTexture {
                        access: StorageTextureAccess::WriteOnly,
                        format: TextureFormat::Rgba16Float,
                        view_dimension: TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("dlss5_bridge_pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&ComputePipelineDescriptor {
                label: Some("dlss5_bridge_pipe"),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let tex = |label: &str| {
            let t = device.create_texture(&TextureDescriptor {
                label: Some(label),
                size: Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba16Float,
                usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let v = t.create_view(&TextureViewDescriptor::default());
            (t, v)
        };
        let (proxy_in, proxy_in_view) = tex("dlss5_proxy_in");
        let (proxy_out, proxy_out_view) = tex("dlss5_proxy_out");
        Self {
            encode: pipe("encode"),
            decode: pipe("decode"),
            bgl,
            proxy_in,
            proxy_in_view,
            proxy_out,
            proxy_out_view,
        }
    }

    pub fn pass(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        decode: bool,
        src: &TextureView,
        dst: &TextureView,
        (w, h): (u32, u32),
    ) {
        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("dlss5_bridge_bg"),
            layout: &self.bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(src),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(dst),
                },
            ],
        });
        let mut cp = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("dlss5_bridge_pass"),
            timestamp_writes: None,
        });
        cp.set_pipeline(if decode { &self.decode } else { &self.encode });
        cp.set_bind_group(0, &bg, &[]);
        cp.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
    }
}
