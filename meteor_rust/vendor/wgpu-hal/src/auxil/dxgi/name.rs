use windows::Win32::Graphics::Direct3D12::ID3D12Object;

use crate::auxil::dxgi::result::HResult;

pub trait ObjectExt {
    fn set_name(&self, name: &str) -> Result<(), crate::DeviceError>;
}

impl<T> ObjectExt for T
where

    for<'a> &'a ID3D12Object: From<&'a T>,
{
    fn set_name(&self, name: &str) -> Result<(), crate::DeviceError> {
        let name = windows::core::HSTRING::from(name);
        let object: &ID3D12Object = self.into();
        unsafe { object.SetName(&name).into_device_result("SetName") }
    }
}
