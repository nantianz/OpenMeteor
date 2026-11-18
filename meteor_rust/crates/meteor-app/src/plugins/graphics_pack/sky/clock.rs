const MINUTES_PER_DAY: f64 = 1440.0;
const SECONDS_PER_MINUTE: f64 = 60.0;

pub(super) struct Cycle {
    start_minutes: u16,
    scale: f64,
    update_interval: f64,
    origin: Option<f64>,
    last_sample: f64,
}

impl Cycle {
    pub(super) fn new(start_minutes: u16, scale: f64, updates_per_second: u32) -> Self {
        Self {
            start_minutes,
            scale,
            update_interval: 1.0 / f64::from(updates_per_second),
            origin: None,
            last_sample: 0.0,
        }
    }

    pub(super) fn sample(&mut self, now: f64) -> Option<f32> {
        let Some(origin) = self.origin else {
            self.origin = Some(now);
            self.last_sample = now;
            return None;
        };
        if now - self.last_sample < self.update_interval {
            return None;
        }
        self.last_sample = now;
        let period = MINUTES_PER_DAY * SECONDS_PER_MINUTE / self.scale;
        let elapsed = ((now - origin) % period) * self.scale / SECONDS_PER_MINUTE;
        Some(((f64::from(self.start_minutes) + elapsed) % MINUTES_PER_DAY) as f32)
    }
}

#[cfg(windows)]
pub(super) fn local_minutes() -> Option<u16> {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};

    let mut local = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut local) };
    Some(local.wHour * 60 + local.wMinute)
}

#[cfg(unix)]
pub(super) fn local_minutes() -> Option<u16> {
    let utc = unsafe { libc::time(std::ptr::null_mut()) };
    let mut local = std::mem::MaybeUninit::uninit();
    if utc == -1 || unsafe { libc::localtime_r(&utc, local.as_mut_ptr()) }.is_null() {
        return None;
    }
    let local = unsafe { local.assume_init() };
    Some((local.tm_hour * 60 + local.tm_min) as u16)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn local_minutes() -> Option<u16> {
    let local = js_sys::Date::new_0();
    Some((local.get_hours() * 60 + local.get_minutes()) as u16)
}

#[cfg(not(any(unix, windows, target_arch = "wasm32")))]
pub(super) fn local_minutes() -> Option<u16> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_wraps_midnight_and_does_not_count_before_first_frame() {
        let mut cycle = Cycle::new(23 * 60 + 30, 60.0, 15);
        assert_eq!(cycle.sample(123.0), None);
        assert_eq!(cycle.sample(123.01), None);
        assert_eq!(cycle.sample(153.0), Some(0.0));
        assert_eq!(cycle.sample(153.0), None);
        assert_eq!(cycle.sample(183.0), Some(30.0));
        assert_eq!(cycle.sample(183.0 + 1440.0), Some(30.0));
    }
}
