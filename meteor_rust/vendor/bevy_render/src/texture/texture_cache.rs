use crate::{
    render_resource::{Texture, TextureView},
    renderer::RenderDevice,
};
use bevy_ecs::{prelude::ResMut, resource::Resource};
use bevy_platform::collections::{hash_map::Entry, HashMap};
use wgpu::{TextureDescriptor, TextureViewDescriptor};

struct CachedTextureMeta {
    texture: Texture,
    default_view: TextureView,
    taken: bool,
    frames_since_last_use: usize,
}

#[derive(Clone)]
pub struct CachedTexture {
    pub texture: Texture,
    pub default_view: TextureView,
}

#[derive(Resource, Default)]
pub struct TextureCache {
    textures: HashMap<TextureDescriptor<'static>, Vec<CachedTextureMeta>>,
}

impl TextureCache {

    pub fn get(
        &mut self,
        render_device: &RenderDevice,
        descriptor: TextureDescriptor<'static>,
    ) -> CachedTexture {
        match self.textures.entry(descriptor) {
            Entry::Occupied(mut entry) => {
                for texture in entry.get_mut().iter_mut() {
                    if !texture.taken {
                        texture.frames_since_last_use = 0;
                        texture.taken = true;
                        return CachedTexture {
                            texture: texture.texture.clone(),
                            default_view: texture.default_view.clone(),
                        };
                    }
                }

                let texture = render_device.create_texture(&entry.key().clone());
                let default_view = texture.create_view(&TextureViewDescriptor::default());
                entry.get_mut().push(CachedTextureMeta {
                    texture: texture.clone(),
                    default_view: default_view.clone(),
                    frames_since_last_use: 0,
                    taken: true,
                });
                CachedTexture {
                    texture,
                    default_view,
                }
            }
            Entry::Vacant(entry) => {
                let texture = render_device.create_texture(entry.key());
                let default_view = texture.create_view(&TextureViewDescriptor::default());
                entry.insert(vec![CachedTextureMeta {
                    texture: texture.clone(),
                    default_view: default_view.clone(),
                    taken: true,
                    frames_since_last_use: 0,
                }]);
                CachedTexture {
                    texture,
                    default_view,
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    pub fn update(&mut self) {
        self.textures.retain(|_, textures| {
            for texture in textures.iter_mut() {
                texture.frames_since_last_use += 1;
                texture.taken = false;
            }

            textures.retain(|texture| texture.frames_since_last_use < 3);
            !textures.is_empty()
        });
    }
}

pub fn update_texture_cache_system(mut texture_cache: ResMut<TextureCache>) {
    texture_cache.update();
}
