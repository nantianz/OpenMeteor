pub(crate) use meteor_asset::pack_manifest::*;

#[cfg(any(target_os = "android", target_arch = "wasm32", test))]
pub(crate) fn match_packs_resident(stage_id: &str, mut contains: impl FnMut(&str) -> bool) -> bool {
    match_pack_roots(stage_id).iter().all(|id| contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn same_stage_requires_reload_after_returning_to_menu() {
        let mut resident: HashSet<String> = match_pack_roots("sn01").into_iter().collect();
        assert!(match_packs_resident("sn01", |id| resident.contains(id)));
        resident.retain(|id| !id.starts_with("stage/"));
        assert!(!match_packs_resident("sn01", |id| resident.contains(id)));
        resident.insert("stage/sn01".into());
        assert!(match_packs_resident("sn01", |id| resident.contains(id)));
        assert!(!match_packs_resident("sn02", |id| resident.contains(id)));
    }
}
