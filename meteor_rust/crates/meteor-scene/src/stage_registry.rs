#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageInfo {
    pub id: String,
    pub number: u8,
    pub name: String,
    pub description: String,
    pub is_custom: bool,
}

pub const OFFICIAL_STAGES: &[(&str, u8, &str)] = &[
    ("sn01", 1, "钟乳洞"),
    ("sn02", 2, "秦皇陵"),
    ("sn03", 3, "一线天"),
    ("sn04", 4, "炽雪城"),
    ("sn05", 5, "皇天城"),
    ("sn06", 6, "四方阵"),
    ("sn07", 7, "死之阵"),
    ("sn08", 8, "毒牙阵"),
    ("sn09", 9, "决死阵"),
    ("sn10", 10, "炼铁狱"),
    ("sn11", 11, "五爪峰"),
    ("sn12", 12, "峰火雷"),
    ("sn13", 13, "金华城"),
    ("sn14", 14, "炎硫岛"),
    ("sn15", 15, "飞鹏堡"),
    ("sn16", 16, "五雷塔"),
    ("sn17", 17, "伏虎山"),
    ("sn18", 18, "圆满楼"),
    ("sn19", 19, "洛阳城"),
    ("sn20", 20, "卧龙岗"),
    ("sn21", 21, "圣诞夜"),
    ("sn22", 22, "威震八方"),
];

pub fn official_stages() -> Vec<StageInfo> {
    OFFICIAL_STAGES
        .iter()
        .map(|(id, number, name)| StageInfo {
            id: id.to_string(),
            number: *number,
            name: name.to_string(),
            description: String::new(),
            is_custom: false,
        })
        .collect()
}

pub fn is_official_stage_id(stage_id: &str) -> bool {
    let normalized = stage_id.trim().to_lowercase();
    OFFICIAL_STAGES.iter().any(|(id, _, _)| *id == normalized)
}

pub fn normalize_stage_id(stage_id: &str) -> String {
    let normalized = stage_id.trim().to_lowercase();

    if let Some(rest) = normalized.strip_prefix("sn") {
        if rest.len() == 2 {
            let digits = rest.as_bytes();
            if digits[0].is_ascii_digit() && digits[1].is_ascii_digit() {
                let n = rest.parse::<u8>().unwrap_or(0);
                if (1..=99).contains(&n) {
                    return normalized;
                }
            }
        }
    }
    "sn01".to_string()
}

pub fn get_stage(stage_id: &str, custom_stages: &[StageInfo]) -> StageInfo {
    let normalized = normalize_stage_id(stage_id);
    if let Some((id, number, name)) = OFFICIAL_STAGES
        .iter()
        .find(|(id, _, _)| *id == normalized.as_str())
    {
        return StageInfo {
            id: id.to_string(),
            number: *number,
            name: name.to_string(),
            description: String::new(),
            is_custom: false,
        };
    }
    if let Some(custom) = custom_stages.iter().find(|s| s.id == normalized) {
        return custom.clone();
    }

    let number = normalized
        .strip_prefix("sn")
        .and_then(|r| r.parse::<u8>().ok())
        .unwrap_or(1);
    StageInfo {
        id: normalized.clone(),
        number,
        name: normalized.to_uppercase(),
        description: String::new(),
        is_custom: true,
    }
}

#[derive(Debug, Clone, Default)]
pub struct CustomStageDescriptor {
    pub pack_id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PackDescriptor {
    #[serde(rename = "packId")]
    pub pack_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "createdAt")]
    pub created_at: String,
    #[serde(default)]
    pub meta: Option<PackMeta>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PackMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PacksManifest {
    #[serde(default, rename = "formatVersion")]
    pub format_version: u8,
    #[serde(default)]
    pub packs: Vec<PackDescriptor>,
}

pub fn custom_stages_from_manifest(text: &str) -> Vec<StageInfo> {
    let Ok(manifest) = serde_json::from_str::<PacksManifest>(text) else {
        return Vec::new();
    };
    if manifest.format_version != 1 {
        return Vec::new();
    }
    let mut stages: Vec<StageInfo> = manifest
        .packs
        .iter()
        .filter_map(|descriptor| {
            let meta = descriptor.meta.as_ref();
            custom_stage_from_descriptor(&CustomStageDescriptor {
                pack_id: descriptor.pack_id.clone(),

                name: if descriptor.name.is_empty() {
                    meta.map(|m| m.name.clone()).unwrap_or_default()
                } else {
                    descriptor.name.clone()
                },
                description: if descriptor.description.is_empty() {
                    meta.map(|m| m.description.clone()).unwrap_or_default()
                } else {
                    descriptor.description.clone()
                },
                created_at: descriptor.created_at.clone(),
            })
        })
        .collect();
    stages.sort_by_key(|stage| stage.number);
    stages
}

pub fn custom_stage_from_descriptor(descriptor: &CustomStageDescriptor) -> Option<StageInfo> {
    let pack_id = descriptor.pack_id.trim().to_lowercase();
    let rest = pack_id.strip_prefix("stage/")?;
    if !rest.starts_with("sn") || rest.len() != 4 {
        return None;
    }
    let id = rest.to_string();
    if is_official_stage_id(&id) {
        return None;
    }
    let number = rest.strip_prefix("sn")?.parse::<u8>().ok()?;
    if number < 23 {
        return None;
    }
    let name = if !descriptor.name.is_empty() {
        descriptor.name.clone()
    } else if !descriptor.created_at.is_empty() {
        descriptor.created_at.clone()
    } else {
        id.to_uppercase()
    };
    Some(StageInfo {
        id,
        number,
        name: name.chars().take(80).collect(),
        description: descriptor.description.chars().take(500).collect(),
        is_custom: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_custom_stages() {
        let json = r#"{
          "formatVersion": 1,
          "packs": [
            {"packId": "stage/sn25", "name": "玩家自制·试炼场", "description": "开阔场地", "createdAt": "2026-08-01"},
            {"packId": "stage/sn23", "name": "自制一", "description": ""},
            {"packId": "stage/sn01", "name": "伪造官方", "description": ""},
            {"packId": "stage/sn22", "name": "官方边界", "description": ""},
            {"packId": "other/sn30", "name": "非关卡包", "description": ""}
          ]
        }"#;
        let stages = custom_stages_from_manifest(json);
        assert_eq!(stages.len(), 2);

        assert_eq!(stages[0].id, "sn23");
        assert_eq!(stages[0].name, "自制一");
        assert!(stages[0].is_custom);
        assert_eq!(stages[1].id, "sn25");
        assert_eq!(stages[1].name, "玩家自制·试炼场");
        assert_eq!(stages[1].description, "开阔场地");

        assert!(custom_stages_from_manifest(r#"{"formatVersion":2,"packs":[]}"#).is_empty());
        assert!(custom_stages_from_manifest("not json").is_empty());

        assert!(custom_stages_from_manifest(r#"{"formatVersion":1,"packs":[]}"#).is_empty());
    }

    #[test]
    fn manifest_falls_back_to_meta_name() {
        let json =
            r#"{"formatVersion":1,"packs":[{"packId":"stage/sn30","meta":{"name":"meta名"}}]}"#;
        let stages = custom_stages_from_manifest(json);
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].name, "meta名");
    }

    #[test]
    fn official_stage_count_and_order() {
        let stages = official_stages();
        assert_eq!(stages.len(), 22);
        assert_eq!(stages[0].id, "sn01");
        assert_eq!(stages[0].name, "钟乳洞");
        assert_eq!(stages[21].id, "sn22");
        assert_eq!(stages[21].name, "威震八方");
    }

    #[test]
    fn official_id_detection() {
        assert!(is_official_stage_id("sn01"));
        assert!(is_official_stage_id("SN22"));
        assert!(!is_official_stage_id("sn23"));
        assert!(!is_official_stage_id("sn0"));
    }

    #[test]
    fn normalize_handles_invalid() {
        assert_eq!(normalize_stage_id(" sn09 "), "sn09");
        assert_eq!(normalize_stage_id("SN10"), "sn10");
        assert_eq!(normalize_stage_id("sn0"), "sn01");
        assert_eq!(normalize_stage_id("foo"), "sn01");
        assert_eq!(normalize_stage_id("sn100"), "sn01");
    }

    #[test]
    fn get_stage_resolution() {
        let s = get_stage("sn01", &[]);
        assert_eq!(s.name, "钟乳洞");
        assert!(!s.is_custom);

        let custom = vec![StageInfo {
            id: "sn23".into(),
            number: 23,
            name: "测试关".into(),
            description: String::new(),
            is_custom: true,
        }];
        let s = get_stage("sn23", &custom);
        assert_eq!(s.name, "测试关");
        assert!(s.is_custom);

        let s = get_stage("sn30", &[]);
        assert_eq!(s.number, 30);
        assert_eq!(s.name, "SN30");
        assert!(s.is_custom);
    }

    #[test]
    fn custom_descriptor_parsing() {
        let d = CustomStageDescriptor {
            pack_id: "stage/sn23".into(),
            name: "自制地图A".into(),
            description: "测试描述".into(),
            created_at: "2026-08-01".into(),
        };
        let s = custom_stage_from_descriptor(&d).unwrap();
        assert_eq!(s.id, "sn23");
        assert_eq!(s.number, 23);
        assert_eq!(s.name, "自制地图A");
        assert!(s.is_custom);

        let d = CustomStageDescriptor {
            pack_id: "stage/sn01".into(),
            ..Default::default()
        };
        assert!(custom_stage_from_descriptor(&d).is_none());

        let d = CustomStageDescriptor {
            pack_id: "stage/sn10".into(),
            ..Default::default()
        };
        assert!(custom_stage_from_descriptor(&d).is_none());

        let d = CustomStageDescriptor {
            pack_id: "foo".into(),
            ..Default::default()
        };
        assert!(custom_stage_from_descriptor(&d).is_none());
    }
}
