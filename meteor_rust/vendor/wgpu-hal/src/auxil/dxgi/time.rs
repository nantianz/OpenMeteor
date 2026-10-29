#![allow(dead_code)]

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

pub enum PresentationTimer {

    Dxgi {

        frequency: u64,
    },

    #[allow(non_snake_case)]
    IPresentationManager {
        fnQueryInterruptTimePrecise: unsafe extern "system" fn(*mut u64),
    },
}

impl core::fmt::Debug for PresentationTimer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Dxgi { frequency } => f
                .debug_struct("DXGI")
                .field("frequency", &frequency)
                .finish(),
            Self::IPresentationManager {
                fnQueryInterruptTimePrecise,
            } => f
                .debug_struct("IPresentationManager")
                .field(
                    "QueryInterruptTimePrecise",
                    &(fnQueryInterruptTimePrecise as usize),
                )
                .finish(),
        }
    }
}

impl PresentationTimer {

    pub fn new_dxgi() -> Self {
        let mut frequency = 0;
        unsafe { QueryPerformanceFrequency(&mut frequency) }.unwrap();

        Self::Dxgi {
            frequency: frequency
                .try_into()
                .expect("Frequency should not be negative"),
        }
    }

    pub fn new_ipresentation_manager() -> Self {

        let kernelbase =
            libloading::os::windows::Library::open_already_loaded("kernelbase.dll").unwrap();

        let ptr = unsafe {
            kernelbase
                .get(c"QueryInterruptTimePrecise".to_bytes())
                .unwrap()
        };
        Self::IPresentationManager {
            fnQueryInterruptTimePrecise: *ptr,
        }
    }

    pub fn get_timestamp_ns(&self) -> u128 {

        match *self {
            PresentationTimer::Dxgi { frequency } => {
                let mut counter = 0;
                unsafe { QueryPerformanceCounter(&mut counter) }.unwrap();

                (counter as u128 * 1_000_000_000) / frequency as u128
            }
            PresentationTimer::IPresentationManager {
                fnQueryInterruptTimePrecise,
            } => {
                let mut counter = 0;
                unsafe { fnQueryInterruptTimePrecise(&mut counter) };

                counter as u128 * 100
            }
        }
    }
}
