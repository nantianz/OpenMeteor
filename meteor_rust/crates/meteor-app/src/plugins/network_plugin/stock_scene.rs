use meteor_asset::{
    des::{StageDescription, StageObject},
    res::MeteorRes,
};
use meteor_network::protocol::SceneOp;
use meteor_network::stock_protocol::stock_scene_item_option_bit;
use meteor_network::stock_protocol::StockSceneObjectState;
use std::collections::HashMap;

pub(crate) struct StockSceneObjectMap {
    wire_to_script: Vec<Option<u32>>,
    #[cfg(any(test, not(target_arch = "wasm32")))]
    script_to_wire: Vec<Option<u32>>,
}

impl StockSceneObjectMap {
    pub(super) fn new(stage: &StageDescription, resource: &MeteorRes, room_flags: u32) -> Self {
        let item_bits = resource
            .records
            .iter()
            .filter_map(|record| {
                let key = record.prop_seed_value(11)?;
                Some((
                    record.name.to_ascii_lowercase(),
                    stock_scene_item_option_bit(key),
                ))
            })
            .collect::<HashMap<_, _>>();

        let static_count = stage
            .scene_objects
            .iter()
            .filter(|object| (3..=7).contains(&authored_object_type(object)))
            .count();
        let mut wire_to_script = vec![None; static_count];
        #[cfg(any(test, not(target_arch = "wasm32")))]
        let mut script_to_wire = vec![None; stage.dummies.len()];
        for (script, object) in stage.dummies.iter().enumerate() {
            let kind = authored_object_type(object);
            let admitted = if (10..=15).contains(&kind) {
                object
                    .model
                    .as_ref()
                    .and_then(|model| item_bits.get(&model.to_ascii_lowercase()))
                    .copied()
                    .flatten()
                    .is_none_or(|bit| room_flags & bit != 0)
            } else {
                object.model.is_none() && object.effect.is_some()
            };
            if admitted {
                #[cfg(any(test, not(target_arch = "wasm32")))]
                {
                    script_to_wire[script] = Some(wire_to_script.len() as u32);
                }
                wire_to_script.push(Some(script as u32));
            }
        }
        Self {
            wire_to_script,
            #[cfg(any(test, not(target_arch = "wasm32")))]
            script_to_wire,
        }
    }

    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn wire_index(&self, script: u32) -> Option<u32> {
        self.script_to_wire.get(script as usize).copied().flatten()
    }

    pub(crate) fn script_index(&self, wire: u32) -> Option<u32> {
        self.wire_to_script.get(wire as usize).copied().flatten()
    }
}

fn authored_object_type(object: &StageObject) -> u8 {
    let name = object
        .name_attr
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        "animation" => 10,
        "hell" => 4,
        "safea" => 5,
        "safeb" => 6,
        "safe" => 7,
        "door" => 11,
        "machine" => 12,
        "normal" => 0,
        "targeta" => 13,
        "targetb" => 14,
        "treasure" => 15,
        _ if name.starts_with("damage") => {
            if object.model.is_some() {
                12
            } else {
                3
            }
        }
        _ if name.starts_with("user") || name.starts_with("team") => 1,
        _ if object.model.is_some() => 15,
        _ => 0,
    }
}

pub(crate) fn snapshot_ops(object: StockSceneObjectState, reference: u32) -> Vec<SceneOp> {
    let set = |field: &str, args: Vec<serde_json::Value>| SceneOp {
        kind: "set".into(),
        reference: reference.into(),
        field: field.into(),
        args,
    };
    let mut ops: Vec<_> = [
        ("active", object.active),
        ("visible", object.visible),
        ("collision", object.collision),
        ("damage", object.damage),
        ("interactive", object.interactive),
    ]
    .into_iter()
    .map(|(attribute, value)| set("attribute", vec![attribute.into(), u32::from(value).into()]))
    .collect();
    if object.damage_value.is_finite() {
        ops.push(set(
            "attribute",
            vec!["damagevalue".into(), f64::from(object.damage_value).into()],
        ));
    }
    if let Ok(pose) = u16::try_from(object.pose) {
        ops.push(set(
            "pose",
            vec![pose.into(), u32::from(object.animation_state == 0).into()],
        ));
        if let Ok(frame) = u16::try_from(object.frame) {
            ops.push(set("snapshot_frame", vec![frame.into()]));
        }
    }
    ops
}

#[cfg(all(test, feature = "original-assets"))]
mod tests {
    use super::*;
    use meteor_asset::{des::parse_stage_description, res::parse_meteor_res};
    use std::path::Path;

    #[test]
    fn stock_indices_follow_admitted_authored_objects() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let resource = parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap()).unwrap();
        for stage_id in ["sn04", "sn11", "sn12", "sn21"] {
            let path = root.join(format!("{stage_id}/{stage_id}.des"));
            let stage = parse_stage_description(&crate::text_codec::decode_text(
                &std::fs::read(path).unwrap(),
            ));
            let all = StockSceneObjectMap::new(&stage, &resource, 0x3c);
            let no_updates = StockSceneObjectMap::new(&stage, &resource, 0x34);
            let mut shifted = false;
            let mut admitted = 0;
            for (index, dummy) in stage.dummies.iter().enumerate() {
                let index = index as u32;
                if let Some(wire) = all.wire_index(index) {
                    admitted += 1;
                    assert_eq!(all.script_index(wire), Some(index));
                    shifted |= wire != index;
                    if dummy
                        .model
                        .as_deref()
                        .is_some_and(|model| model.eq_ignore_ascii_case("item16"))
                    {
                        assert_eq!(no_updates.wire_index(index), None);
                    }
                }
            }
            assert!(
                admitted > 20 && shifted,
                "{stage_id} must use the compact wire table"
            );
        }
    }
}
