use crate::command_parser::Dir;

pub const DIRECTION_TAP_SECONDS: f64 = 0.3;

#[derive(Debug, Clone, Default)]
pub struct DirectionTap {
    last: Option<(Dir, f64)>,
}

impl DirectionTap {
    pub fn press(&mut self, direction: Dir, now: f64) -> bool {
        if self.last.is_some_and(|(previous, _)| previous == direction) {
            self.last = None;
            true
        } else {
            self.last = Some((direction, now));
            false
        }
    }

    pub fn update(&mut self, now: f64) {
        if self
            .last
            .is_some_and(|(_, at)| now < at || now - at >= DIRECTION_TAP_SECONDS)
        {
            self.last = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_consecutive_directions_form_a_double_tap() {
        let mut taps = DirectionTap::default();
        assert!(!taps.press(Dir::Up, 0.0));
        assert!(!taps.press(Dir::Right, 0.05));
        assert!(!taps.press(Dir::Up, 0.1));
        assert!(taps.press(Dir::Up, 0.15));
        assert!(!taps.press(Dir::Up, 0.2));
    }

    #[test]
    fn controller_update_expires_at_three_tenths_after_input_dispatch() {
        for interval in [0.25, 0.275, 0.299] {
            let mut taps = DirectionTap::default();
            taps.press(Dir::Left, 0.0);
            taps.update(interval);
            assert!(taps.press(Dir::Left, interval));
        }
        let mut taps = DirectionTap::default();
        taps.press(Dir::Left, 0.0);
        taps.update(0.3);
        assert!(!taps.press(Dir::Left, 0.3));

        let mut taps = DirectionTap::default();
        taps.press(Dir::Left, 0.0);
        assert!(
            taps.press(Dir::Left, 0.3),
            "input precedes the expiry update"
        );
        taps.update(0.3);
        assert!(!taps.press(Dir::Left, 0.3));
    }
}
