use super::CachedTexture;
use crate::render_resource::{TextureFormat, TextureView};
use alloc::sync::Arc;
use bevy_color::LinearRgba;
use core::sync::atomic::{AtomicBool, Ordering};
use wgpu::{
    Color as WgpuColor, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, StoreOp,
};

#[derive(Clone)]
pub struct ColorAttachment {
    pub texture: CachedTexture,
    pub resolve_target: Option<CachedTexture>,
    pub previous_frame_texture: Option<CachedTexture>,
    clear_color: Option<WgpuColor>,
    is_first_call: Arc<AtomicBool>,
}

impl ColorAttachment {
    pub fn new(
        texture: CachedTexture,
        resolve_target: Option<CachedTexture>,
        previous_frame_texture: Option<CachedTexture>,
        clear_color: Option<WgpuColor>,
    ) -> Self {
        Self {
            texture,
            resolve_target,
            previous_frame_texture,
            clear_color,
            is_first_call: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn get_attachment(&self) -> RenderPassColorAttachment<'_> {
        if let Some(resolve_target) = self.resolve_target.as_ref() {
            let first_call = self.is_first_call.fetch_and(false, Ordering::SeqCst);

            RenderPassColorAttachment {
                view: &resolve_target.default_view,
                depth_slice: None,
                resolve_target: Some(&self.texture.default_view),
                ops: Operations {
                    load: match (self.clear_color, first_call) {
                        (Some(clear_color), true) => LoadOp::Clear(clear_color),
                        (None, _) | (Some(_), false) => LoadOp::Load,
                    },
                    store: StoreOp::Store,
                },
            }
        } else {
            self.get_unsampled_attachment()
        }
    }

    pub fn get_unsampled_attachment(&self) -> RenderPassColorAttachment<'_> {
        let first_call = self.is_first_call.fetch_and(false, Ordering::SeqCst);

        RenderPassColorAttachment {
            view: &self.texture.default_view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: match (self.clear_color, first_call) {
                    (Some(clear_color), true) => LoadOp::Clear(clear_color),
                    (None, _) | (Some(_), false) => LoadOp::Load,
                },
                store: StoreOp::Store,
            },
        }
    }

    pub(crate) fn mark_as_cleared(&self) {
        self.is_first_call.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct DepthAttachment {
    pub view: TextureView,
    clear_value: Option<f32>,
    is_first_call: Arc<AtomicBool>,
}

impl DepthAttachment {
    pub fn new(view: TextureView, clear_value: Option<f32>) -> Self {
        Self {
            view,
            clear_value,
            is_first_call: Arc::new(AtomicBool::new(clear_value.is_some())),
        }
    }

    pub fn get_attachment(&self, store: StoreOp) -> RenderPassDepthStencilAttachment<'_> {
        let first_call = self
            .is_first_call
            .fetch_and(store != StoreOp::Store, Ordering::Relaxed);

        RenderPassDepthStencilAttachment {
            view: &self.view,
            depth_ops: Some(Operations {
                load: if first_call {

                    LoadOp::Clear(self.clear_value.unwrap())
                } else {
                    LoadOp::Load
                },
                store,
            }),
            stencil_ops: None,
        }
    }

    pub fn prepare_for_new_frame(&self) {
        self.is_first_call.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct OutputColorAttachment {
    pub view: TextureView,
    pub view_format: TextureFormat,
    is_first_call: Arc<AtomicBool>,
}

impl OutputColorAttachment {
    pub fn new(view: TextureView, view_format: TextureFormat) -> Self {
        Self {
            view,
            view_format,
            is_first_call: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn get_attachment(&self, clear_color: Option<LinearRgba>) -> RenderPassColorAttachment<'_> {
        let first_call = self.is_first_call.fetch_and(false, Ordering::SeqCst);

        RenderPassColorAttachment {
            view: &self.view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: match (clear_color, first_call) {
                    (Some(clear_color), true) => LoadOp::Clear(clear_color.into()),
                    (None, _) | (Some(_), false) => LoadOp::Load,
                },
                store: StoreOp::Store,
            },
        }
    }

    pub fn needs_present(&self) -> bool {
        !self.is_first_call.load(Ordering::SeqCst)
    }
}
