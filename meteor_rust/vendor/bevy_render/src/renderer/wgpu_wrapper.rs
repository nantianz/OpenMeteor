

#[derive(Debug, Clone)]
pub struct WgpuWrapper<T>(
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))] T,
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))] send_wrapper::SendWrapper<T>,
);

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[expect(unsafe_code, reason = "Blanket-impl Send requires unsafe.")]
unsafe impl<T> Send for WgpuWrapper<T> {}
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[expect(unsafe_code, reason = "Blanket-impl Sync requires unsafe.")]
unsafe impl<T> Sync for WgpuWrapper<T> {}

impl<T> WgpuWrapper<T> {

    pub fn new(t: T) -> Self {
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        return Self(t);
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        return Self(send_wrapper::SendWrapper::new(t));
    }

    pub fn into_inner(self) -> T {
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        return self.0;
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        return self.0.take();
    }
}

impl<T> core::ops::Deref for WgpuWrapper<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> core::ops::DerefMut for WgpuWrapper<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
