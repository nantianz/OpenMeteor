pub const INPUT_GAP_MS: f64 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub fn command_char(&self) -> char {
        match self {
            Dir::Up => 'u',
            Dir::Down => 'd',
            Dir::Left => 'l',
            Dir::Right => 'r',
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandInput {
    pub history: Vec<Dir>,

    pub last_direction_at: Option<f64>,

    pub generation: u64,

    held: Vec<Dir>,

    virtual_held: Vec<Dir>,
}

impl CommandInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_direction(&mut self, dir: Dir, now_ms: f64) {
        if self.held.contains(&dir) {
            return;
        }
        self.held.push(dir);
        self.history.push(dir);
        self.last_direction_at = Some(now_ms);
    }

    pub fn release_direction(&mut self, dir: Dir) {
        self.held.retain(|d| *d != dir);
    }

    pub fn held_direction(&self) -> Option<Dir> {
        let forward = self.held.contains(&Dir::Up) || self.virtual_held.contains(&Dir::Up);
        let backward = self.held.contains(&Dir::Down) || self.virtual_held.contains(&Dir::Down);
        let right = self.held.contains(&Dir::Right) || self.virtual_held.contains(&Dir::Right);
        let left = self.held.contains(&Dir::Left) || self.virtual_held.contains(&Dir::Left);
        let longitudinal = (forward as i8) - (backward as i8);
        let lateral = (right as i8) - (left as i8);
        if longitudinal > 0 {
            Some(Dir::Up)
        } else if longitudinal < 0 {
            Some(Dir::Down)
        } else if lateral > 0 {
            Some(Dir::Right)
        } else if lateral < 0 {
            Some(Dir::Left)
        } else {
            None
        }
    }

    pub fn set_virtual_direction(&mut self, dir: Dir, down: bool, record: bool) -> bool {
        let was_held = self.virtual_held.contains(&dir);
        if down == was_held {
            return false;
        }
        if down {
            self.virtual_held.push(dir);
            if record {
                self.pulse_virtual_direction(dir);
            }
        } else {
            self.virtual_held.retain(|d| *d != dir);
        }
        true
    }

    pub fn pulse_virtual_direction(&mut self, dir: Dir) -> bool {
        self.history.push(dir);
        self.last_direction_at = Some(0.0);
        true
    }

    pub fn update(&mut self, now_ms: f64) {
        match self.last_direction_at {
            None => return,
            Some(last) if now_ms - last < INPUT_GAP_MS => return,
            Some(_) => {}
        }
        self.history.clear();
        self.generation += 1;
        if let Some(held) = self.held_direction() {
            self.history.push(held);
        }
    }

    pub fn peek(&self) -> Vec<Dir> {
        self.history.clone()
    }

    pub fn take(&mut self) -> Vec<Dir> {
        let command = self.history.clone();
        self.history.clear();
        self.generation += 1;
        command
    }

    pub fn snapshot(&self) -> (u64, usize) {
        (self.generation, self.history.len())
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.held.clear();
        self.virtual_held.clear();
        self.last_direction_at = None;
        self.generation += 1;
    }

    pub fn command_string(&self) -> String {
        self.history.iter().map(|d| d.command_char()).collect()
    }
}

pub fn parse_command_string(s: &str) -> Vec<Dir> {
    s.chars()
        .filter_map(|c| match c {
            'u' | 'U' => Some(Dir::Up),
            'd' | 'D' => Some(Dir::Down),
            'l' | 'L' => Some(Dir::Left),
            'r' | 'R' => Some(Dir::Right),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_timeout_matches_exe() {
        assert_eq!(INPUT_GAP_MS, 300.0);
    }

    #[test]
    fn history_builds_and_takes() {
        let mut input = CommandInput::new();
        input.push_direction(Dir::Up, 0.0);
        input.push_direction(Dir::Left, 100.0);
        assert_eq!(input.command_string(), "ul");

        let cmd = input.take();
        assert_eq!(cmd, vec![Dir::Up, Dir::Left]);
        assert!(input.history.is_empty());
        assert_eq!(input.generation, 1);
    }

    #[test]
    fn gap_timeout_reseeds_held() {
        let mut input = CommandInput::new();
        input.push_direction(Dir::Up, 0.0);
        input.push_direction(Dir::Left, 100.0);

        input.update(500.0);

        assert_eq!(input.command_string(), "u");
    }

    #[test]
    fn no_timeout_keeps_history() {
        let mut input = CommandInput::new();
        input.push_direction(Dir::Up, 0.0);
        input.push_direction(Dir::Left, 100.0);
        input.update(200.0);
        assert_eq!(input.command_string(), "ul");
    }

    #[test]
    fn held_direction_priority() {
        let mut input = CommandInput::new();

        input.push_direction(Dir::Up, 0.0);
        input.push_direction(Dir::Right, 10.0);
        assert_eq!(input.held_direction(), Some(Dir::Up));

        input.release_direction(Dir::Up);
        assert_eq!(input.held_direction(), Some(Dir::Right));

        input.release_direction(Dir::Right);
        assert_eq!(input.held_direction(), None);
    }

    #[test]
    fn virtual_direction_edges() {
        let mut input = CommandInput::new();

        assert!(input.set_virtual_direction(Dir::Down, true, true));
        assert_eq!(input.command_string(), "d");

        assert!(!input.set_virtual_direction(Dir::Down, true, true));

        assert!(input.set_virtual_direction(Dir::Down, false, true));
        assert_eq!(input.command_string(), "d");
        assert_eq!(input.held_direction(), None);
    }

    #[test]
    fn repeat_ignored() {
        let mut input = CommandInput::new();
        input.push_direction(Dir::Up, 0.0);
        input.push_direction(Dir::Up, 50.0);
        assert_eq!(input.command_string(), "u");
    }

    #[test]
    fn parse_command_string_roundtrip() {
        assert_eq!(
            parse_command_string("uldr"),
            vec![Dir::Up, Dir::Left, Dir::Down, Dir::Right]
        );
        assert_eq!(
            parse_command_string("UDLR"),
            vec![Dir::Up, Dir::Down, Dir::Left, Dir::Right]
        );
        assert!(parse_command_string("xyz").is_empty());
    }

    #[test]
    fn clear_resets_everything() {
        let mut input = CommandInput::new();
        input.push_direction(Dir::Up, 0.0);
        input.set_virtual_direction(Dir::Left, true, true);
        input.clear();
        assert!(input.history.is_empty());
        assert!(input.held.is_empty());
        assert!(input.virtual_held.is_empty());
        assert!(input.last_direction_at.is_none());
    }

    #[test]
    fn command_chars() {
        assert_eq!(Dir::Up.command_char(), 'u');
        assert_eq!(Dir::Down.command_char(), 'd');
        assert_eq!(Dir::Left.command_char(), 'l');
        assert_eq!(Dir::Right.command_char(), 'r');
    }
}
