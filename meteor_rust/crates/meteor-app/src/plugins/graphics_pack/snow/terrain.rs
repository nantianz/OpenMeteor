use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, MeshTag};
use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, PrimitiveTopology, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::{Shader, ShaderRef};

use super::super::{
    config::Snow,
    ground_cover::{GroundCover, SPACING},
};
use crate::plugins::render_plugin::PlayingSceneEntity;

pub(super) const TILE: f32 = 8.0;
pub(super) const CONTACT_SIDE: u32 = 64;
pub(super) const CONTACT_STEP: f32 = TILE / CONTACT_SIDE as f32;
const MASK_SIDE: u32 = 32;
const MASK_STEP: f32 = SPACING;
const EDGE_WIDTH: f32 = 2.2;
const BASE_SEGMENTS: u32 = 8;
const COMPACT: bool = cfg!(any(target_arch = "wasm32", target_os = "android"));
const DETAIL_SEGMENTS: u32 = if COMPACT { 32 } else { 64 };
pub(super) const DETAIL_TILES: usize = if COMPACT { 4 } else { 8 };
const DETAIL_RADIUS: f32 = 22.0;
const MAX_TILES: u32 = 1024;
const MAX_TILES_PER_AXIS: u32 = 64;
const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("98cca7d2-aa61-4648-9a7c-3c63ddebd6ed");

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub origin: Vec2,
    pub tiles: UVec2,
    pub floor: f32,
    pub thickness: f32,
}

impl Layout {
    fn from_triangles(triangles: &[[Vec3; 3]], thickness: f32) -> Option<Self> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for point in triangles.iter().flatten() {
            min = min.min(*point);
            max = max.max(*point);
        }
        if !min.is_finite()
            || !max.is_finite()
            || max.y - min.y > 0.10
            || min.abs().max(max.abs()).max_element() > 32_768.0
        {
            return None;
        }
        let origin = ((min.xz() - MASK_STEP) / TILE).floor() * TILE;
        let tiles = ((max.xz() + MASK_STEP - origin) / TILE).ceil().as_uvec2();
        if tiles.min_element() == 0
            || tiles.max_element() > MAX_TILES_PER_AXIS
            || tiles.x * tiles.y > MAX_TILES
        {
            return None;
        }
        Some(Self {
            origin,
            tiles,
            floor: max.y + 0.012,
            thickness,
        })
    }

    pub fn contact_size(self) -> UVec2 {
        self.tiles * CONTACT_SIDE
    }

    pub fn tile_origin(self, tile: UVec2) -> Vec2 {
        self.origin + tile.as_vec2() * TILE
    }

    fn mask_size(self) -> UVec2 {
        self.tiles * MASK_SIDE
    }
}

pub(super) struct Coverage {
    pub layout: Layout,
    mask: GroundCover,
}

impl Coverage {
    pub fn build(triangles: &[[Vec3; 3]], profile: &Snow) -> Option<Self> {
        let layout = Layout::from_triangles(triangles, profile.thickness)?;
        let mask = GroundCover::raster(
            triangles,
            layout.origin,
            layout.mask_size(),
            None,
            SPACING,
            EDGE_WIDTH,
        )?;
        Some(Self { layout, mask })
    }

    pub fn at_contact_texel(&self, p: UVec2) -> u8 {
        let p = p * MASK_SIDE / CONTACT_SIDE;
        self.mask.depths[(p.y * self.layout.mask_size().x + p.x) as usize]
    }

    pub fn at_world(&self, p: Vec2) -> u8 {
        let cell = ((p - self.layout.origin) / MASK_STEP).floor();
        let size = self.layout.mask_size();
        if cell.cmplt(Vec2::ZERO).any() || cell.cmpge(size.as_vec2()).any() {
            return 0;
        }
        let cell = cell.as_uvec2();
        self.mask.depths[(cell.y * size.x + cell.x) as usize]
    }

    fn occupied(&self, cell: UVec2) -> bool {
        let first = cell * MASK_SIDE;
        let row_stride = self.layout.mask_size().x as usize;
        (first.y..first.y + MASK_SIDE).any(|y| {
            let start = y as usize * row_stride + first.x as usize;
            self.mask.depths[start..start + MASK_SIDE as usize]
                .iter()
                .any(|&x| x > 0)
        })
    }

    fn touches_edge(&self, cell: UVec2) -> bool {
        let size = self.layout.mask_size();
        let lo = (cell * MASK_SIDE).saturating_sub(UVec2::ONE);
        let hi = ((cell + UVec2::ONE) * MASK_SIDE + UVec2::ONE).min(size);
        (lo.y..hi.y).any(|y| {
            let start = (y * size.x + lo.x) as usize;
            let end = (y * size.x + hi.x) as usize;
            self.mask.depths[start..end]
                .iter()
                .any(|&depth| depth < 255)
        })
    }

    pub fn spawn(
        &self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
        materials: &mut Assets<SnowMaterial>,
        deformation: Handle<Image>,
    ) -> (SnowLod, usize) {
        let layout = self.layout;
        let mask = images.add(field_image(
            layout.mask_size(),
            self.mask.depths.clone(),
            TextureFormat::R8Unorm,
        ));
        let material = SnowMaterial {
            coverage: mask,
            deformation,
            crystal: images.add(crystal_texture()),
            edge: false,
            terrain: TerrainUniform {
                bounds: layout
                    .origin
                    .extend(layout.tiles.x as f32 * TILE)
                    .extend(layout.tiles.y as f32 * TILE),
                geometry: Vec4::new(
                    layout.thickness,
                    TILE,
                    TILE / BASE_SEGMENTS as f32,
                    CONTACT_STEP,
                ),
            },
        };
        let edge = materials.add(SnowMaterial {
            edge: true,
            ..material.clone()
        });
        let material = materials.add(material);
        let base = meshes.add(grid(BASE_SEGMENTS));
        let detail = meshes.add(grid(DETAIL_SEGMENTS));
        let aabb = Aabb::from_min_max(Vec3::ZERO, Vec3::new(TILE, layout.thickness * 1.45, TILE));
        let mut occupied = 0;
        for z in 0..layout.tiles.y {
            for x in 0..layout.tiles.x {
                let cell = UVec2::new(x, z);
                if !self.occupied(cell) {
                    continue;
                }
                occupied += 1;
                let pos = layout.tile_origin(cell);
                commands.spawn((
                    PlayingSceneEntity,
                    SnowTile {
                        cell,
                        center: pos + Vec2::splat(TILE * 0.5),
                    },
                    Mesh3d(base.clone()),
                    MeshMaterial3d(if self.touches_edge(cell) {
                        edge.clone()
                    } else {
                        material.clone()
                    }),
                    MeshTag(0),
                    Transform::from_xyz(pos.x, layout.floor, pos.y),
                    aabb,
                    NoAutoAabb,
                    bevy::light::NotShadowCaster,
                ));
            }
        }
        (SnowLod { base, detail }, occupied)
    }
}

pub(super) fn field_image(size: UVec2, data: Vec<u8>, format: TextureFormat) -> Image {
    let mut image = Image::new(
        extent(size),
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

pub(super) fn extent(size: UVec2) -> Extent3d {
    Extent3d {
        width: size.x,
        height: size.y,
        depth_or_array_layers: 1,
    }
}

#[derive(Clone, ShaderType)]
struct TerrainUniform {
    bounds: Vec4,
    geometry: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(in crate::plugins) struct SnowMaterial {
    edge: bool,
    #[uniform(0)]
    terrain: TerrainUniform,
    #[texture(1, visibility(vertex, fragment))]
    #[sampler(2, visibility(vertex, fragment))]
    coverage: Handle<Image>,
    #[texture(3, visibility(vertex, fragment))]
    #[sampler(4, visibility(vertex, fragment))]
    deformation: Handle<Image>,
    #[texture(5)]
    #[sampler(6)]
    crystal: Handle<Image>,
}

impl Material for SnowMaterial {
    fn alpha_mode(&self) -> AlphaMode {
        if self.edge {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        }
    }

    fn depth_bias(&self) -> f32 {
        -1000.0
    }

    fn vertex_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<SnowMaterial>::default());
    bevy::asset::load_internal_asset!(app, SHADER, "../snow.wgsl", Shader::from_wgsl);
}

fn grid(segments: u32) -> Mesh {
    let side = segments + 1;
    let step = TILE / segments as f32;
    let positions: Vec<_> = (0..side)
        .flat_map(|z| (0..side).map(move |x| [x as f32 * step, 0.0, z as f32 * step]))
        .collect();
    let mut indices = Vec::with_capacity((segments * segments * 6) as usize);
    for z in 0..segments {
        for x in 0..segments {
            let a = z * side + x;
            indices.extend_from_slice(&[a, a + side, a + 1, a + 1, a + side, a + side + 1]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_indices(Indices::U32(indices))
}

fn crystal_texture() -> Image {
    const SIDE: u32 = 128;
    let noise = |x: f32, y: f32, period: u32| {
        let a = x * period as f32 / SIDE as f32;
        let b = y * period as f32 / SIDE as f32;
        let cell = UVec2::new(a.floor() as u32, b.floor() as u32);
        let t = Vec2::new(a.fract(), b.fract());
        let t = t * t * (Vec2::splat(3.0) - 2.0 * t);
        let hash = |dx: u32, dy: u32| {
            let x = (cell.x + dx) % period;
            let y = (cell.y + dy) % period;
            let seed = (x + y * 251 + period * 367).wrapping_mul(0x9e37_79b9);
            let seed = (seed ^ (seed >> 16)).wrapping_mul(0x85eb_ca6b);
            ((seed >> 24) as f32) / 255.0
        };
        let lower = hash(0, 0) + (hash(1, 0) - hash(0, 0)) * t.x;
        let upper = hash(0, 1) + (hash(1, 1) - hash(0, 1)) * t.x;
        lower + (upper - lower) * t.y
    };
    let mut bytes = Vec::new();
    let mut mip = Vec::with_capacity((SIDE * SIDE) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (x, y) = (x as f32, y as f32);
            let grain = 0.42 * noise(x, y, 9)
                + 0.34 * noise(x, y, 23)
                + 0.16 * noise(x, y, 53)
                + 0.08 * noise(x, y, 107);
            mip.push((grain * 255.0).round() as u8);
        }
    }
    let mut width = SIDE as usize;
    loop {
        bytes.extend(&mip);
        if width == 1 {
            break;
        }
        let mut smaller = Vec::with_capacity(width * width / 4);
        for y in (0..width).step_by(2) {
            for x in (0..width).step_by(2) {
                let i = y * width + x;
                let sum = [mip[i], mip[i + 1], mip[i + width], mip[i + width + 1]]
                    .into_iter()
                    .map(u16::from)
                    .sum::<u16>();
                smaller.push((sum / 4) as u8);
            }
        }
        mip = smaller;
        width /= 2;
    }
    let mut image = Image::new_uninit(
        extent(UVec2::splat(SIDE)),
        TextureDimension::D2,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(bytes);
    image.texture_descriptor.mip_level_count = SIDE.ilog2() + 1;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

#[derive(Component)]
pub(in crate::plugins) struct SnowTile {
    cell: UVec2,
    center: Vec2,
}

pub(super) struct SnowLod {
    base: Handle<Mesh>,
    detail: Handle<Mesh>,
}

impl SnowLod {
    pub fn update(
        &self,
        camera: Vec2,
        disturbed: impl Fn(UVec2) -> bool,
        tiles: &mut Query<(&SnowTile, &mut Mesh3d, &mut MeshTag), Without<super::SnowField>>,
    ) {
        let mut near = Vec::new();
        for (tile, _, _) in tiles.iter() {
            let distance = tile.center.distance_squared(camera);
            if distance < DETAIL_RADIUS * DETAIL_RADIUS && disturbed(tile.cell) {
                near.push((distance, tile.cell));
            }
        }
        near.sort_unstable_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.to_array().cmp(&b.1.to_array()))
        });
        near.truncate(DETAIL_TILES);
        let fine = |cell: IVec2| near.iter().any(|&(_, c)| c.as_ivec2() == cell);
        for (tile, mut mesh, mut tag) in tiles {
            let cell = tile.cell.as_ivec2();
            let detailed = fine(cell);
            let target = if detailed { &self.detail } else { &self.base };
            if mesh.0 != *target {
                mesh.0 = target.clone();
            }

            let changed = disturbed(tile.cell)
                || [IVec2::NEG_X, IVec2::X, IVec2::NEG_Y, IVec2::Y]
                    .iter()
                    .any(|side| {
                        let next = cell + *side;
                        next.cmpge(IVec2::ZERO).all() && disturbed(next.as_uvec2())
                    });
            let mut mask = u32::from(changed);
            if detailed {
                mask |= 32;
                for (bit, side) in [IVec2::NEG_X, IVec2::X, IVec2::NEG_Y, IVec2::Y]
                    .into_iter()
                    .enumerate()
                {
                    mask |= u32::from(!fine(cell + side)) << (1 + bit);
                }
            }
            if tag.0 != mask {
                tag.0 = mask;
            }
        }
    }
}
