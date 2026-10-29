#[derive(Debug, Clone)]
pub struct PropInstance {
    pub name: String,
    pub active: bool,
    pub collision: bool,
    pub interactive: bool,

    pub hp: f32,
}

impl PropInstance {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            active: true,
            collision: false,
            interactive: false,
            hp: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SceneInitStats {
    pub stones: u32,
    pub boxes: u32,
    pub jugs: u32,
    pub hidden_drops: u32,
}

pub fn is_breakable_drop(name: &str) -> bool {
    let rest = match name.strip_prefix("D_") {
        Some(r) => r,
        None => return false,
    };
    let rest = match rest.strip_prefix("it").or_else(|| rest.strip_prefix("wp")) {
        Some(r) => r,
        None => return false,
    };
    rest.starts_with("St")
        || rest.starts_with("BBox")
        || rest.starts_with("BBBox")
        || rest.starts_with("Box")
        || rest.starts_with("Chair")
        || rest.starts_with("Desk")
        || rest.starts_with("Jug")
        || rest.starts_with("RJug")
}

pub fn apply_scene_on_init(props: &mut [PropInstance]) -> SceneInitStats {
    let mut stats = SceneInitStats::default();
    for inst in props {
        let n = &inst.name;
        if n.starts_with("D_St") {
            inst.active = true;
            inst.collision = true;
            inst.interactive = true;
            inst.hp = 500.0;
            stats.stones += 1;
        } else if n.starts_with("D_BBox") || n.starts_with("D_Box") || n.starts_with("D_BBBox") {
            inst.active = true;
            inst.collision = true;
            inst.interactive = true;
            stats.boxes += 1;
        } else if n.starts_with("D_Jug")
            || n.starts_with("D_RJug")
            || n.starts_with("D_Chair")
            || n.starts_with("D_Desk")
        {
            inst.active = true;
            inst.collision = true;
            inst.interactive = true;
            stats.jugs += 1;
        } else if is_breakable_drop(n) {
            inst.active = false;
            inst.collision = false;
            inst.interactive = false;
            stats.hidden_drops += 1;
        } else {
            inst.active = true;
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stone_props() {
        let mut props = vec![PropInstance::new("D_St01"), PropInstance::new("D_St02")];
        let stats = apply_scene_on_init(&mut props);
        assert_eq!(stats.stones, 2);
        for p in &props {
            assert!(p.active);
            assert!(p.collision);
            assert!(p.interactive);
            assert_eq!(p.hp, 500.0);
        }
    }

    #[test]
    fn box_and_jug_props() {
        let mut props = vec![
            PropInstance::new("D_Box01"),
            PropInstance::new("D_BBox02"),
            PropInstance::new("D_Jug01"),
            PropInstance::new("D_RJug02"),
            PropInstance::new("D_Chair03"),
            PropInstance::new("D_Desk01"),
        ];
        let stats = apply_scene_on_init(&mut props);
        assert_eq!(stats.boxes, 2);
        assert_eq!(stats.jugs, 4);
        for p in &props {
            assert!(p.active);
            assert!(p.collision);
            assert!(p.interactive);
        }
    }

    #[test]
    fn breakable_drops_hidden() {
        let mut props = vec![
            PropInstance::new("D_itSt01"),
            PropInstance::new("D_wpBox02"),
            PropInstance::new("D_itJug03"),
            PropInstance::new("D_wpDesk04"),
            PropInstance::new("D_itChair05"),
        ];
        let stats = apply_scene_on_init(&mut props);
        assert_eq!(stats.hidden_drops, 5);
        for p in &props {
            assert!(!p.active);
            assert!(!p.collision);
            assert!(!p.interactive);
        }
    }

    #[test]
    fn ordinary_props_active() {
        let mut props = vec![PropInstance::new("D_Prop01")];
        let stats = apply_scene_on_init(&mut props);
        assert!(props[0].active);
        assert!(!props[0].collision);
        assert_eq!(stats.hidden_drops, 0);
    }

    #[test]
    fn is_breakable_drop_patterns() {
        assert!(is_breakable_drop("D_itSt01"));
        assert!(is_breakable_drop("D_wpBox02"));
        assert!(is_breakable_drop("D_wpBBBox03"));
        assert!(is_breakable_drop("D_itRJug04"));
        assert!(is_breakable_drop("D_wpChair05"));
        assert!(!is_breakable_drop("D_St01"));
        assert!(!is_breakable_drop("D_Box01"));
        assert!(!is_breakable_drop("Plain"));
    }
}
