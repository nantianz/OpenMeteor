

use bytemuck::Pod;

pub trait AtomicPod: Pod + Default + Send + Sync + 'static {

    type Blob: AtomicPodBlob;

    fn read_from_blob(blob: &Self::Blob) -> Self;

    fn write_to_blob(&self, blob: &Self::Blob);
}

pub unsafe trait AtomicPodBlob: Default + Send + Sync + 'static {}

#[macro_export]
macro_rules! impl_atomic_pod {
    (
        $pod_ty: ty,
        $blob_ty: ident
        $(, field($field_name: ident : $field_ty: ty, $getter: ident $(, $($setter: ident)?)?))*
        $(,)?
    ) => {
        #[derive(Default, ::bevy_derive::Deref, ::bevy_derive::DerefMut)]
        #[repr(transparent)]
        pub struct $blob_ty(
            pub [::core::sync::atomic::AtomicU32; ::core::mem::size_of::<$pod_ty>() / 4],
        );

        impl $crate::render_resource::AtomicPod for $pod_ty {
            type Blob = $blob_ty;

            fn read_from_blob(blob: &Self::Blob) -> Self {
                const _ASSERT_POD_TYPE_SIZE: () = assert!(
                    ::core::mem::size_of::<$pod_ty>() % 4 == 0
                );

                let nonatomic_data: [u32; ::core::mem::size_of::<$pod_ty>() / 4] =
                    ::core::array::from_fn(|i| {
                        blob.0[i].load(::bevy_platform::sync::atomic::Ordering::Relaxed)
                    });
                ::bytemuck::must_cast(nonatomic_data)
            }

            fn write_to_blob(&self, blob: &Self::Blob) {

                let src: [u32; ::core::mem::size_of::<$pod_ty>() / 4] =
                    ::bytemuck::must_cast(*self);
                for (dest, src) in blob.0.iter().zip(src.iter()) {
                    dest.store(*src, ::bevy_platform::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        unsafe impl $crate::render_resource::AtomicPodBlob for $blob_ty {}

        impl<'a> ::core::convert::From<&'a $pod_ty> for $blob_ty {
            fn from(pod: &'a $pod_ty) -> Self {
                let blob = Self::default();
                pod.write_to_blob(&blob);
                blob
            }
        }

        impl $blob_ty {
            $(
                $(
                    pub fn $getter(&self) -> $field_ty {
                        const _ASSERT_FIELD_SIZE: () = assert!(
                            ::core::mem::size_of::<$field_ty>() % 4 == 0
                        );

                        let words: [u32; ::core::mem::size_of::<$field_ty>() / 4] =
                            ::core::array::from_fn(|i| {
                                self.0[offset_of!($pod_ty, $field_name) / 4 + i]
                                    .load(::bevy_platform::sync::atomic::Ordering::Relaxed)
                            });
                        *::bytemuck::must_cast_ref(&words)
                    }

                    $(
                        pub fn $setter(&self, value: $field_ty) {

                            let words: [u32; ::core::mem::size_of::<$field_ty>() / 4] =
                                ::bytemuck::must_cast(value);
                            for i in 0..(::core::mem::size_of::<$field_ty>() / 4) {
                                self.0[offset_of!($pod_ty, $field_name) / 4 + i]
                                    .store(words[i], ::bevy_platform::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    )?
                )*
            )?
        }
    };
}

impl_atomic_pod!((), AtomicPodUnitBlob);
