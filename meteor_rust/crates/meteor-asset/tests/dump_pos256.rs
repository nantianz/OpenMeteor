#![cfg(feature = "original-assets")]
#[test]
fn dump_pos256() {
    let data = std::fs::read("../../../origin_res/SFX/pos256.ef").expect("read pos256.ef");
    let ef = meteor_asset::pon_ef::parse_pon_ef(&data).expect("parse");
    for (i, n) in ef.nodes.iter().enumerate() {
        println!(
            "node{i}: type={} name={:?} values={:?} tail={:?}",
            n.type_id,
            n.name,
            n.values,
            match &n.tail {
                meteor_asset::pon_ef::PonEfTail::Floats(f) => format!("Floats({f:?})"),
                other => format!("{other:?}"),
            }
        );
    }
}
