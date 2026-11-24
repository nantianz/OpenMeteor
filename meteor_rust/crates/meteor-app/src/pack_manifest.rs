pub(crate) use meteor_asset::pack_manifest::*;

#[cfg(any(target_os = "android", target_arch = "wasm32"))]
pub(crate) fn installed_match_pack_roots(request: &crate::app_state::LaunchRequest) -> Vec<String> {
    let mut roots = match_pack_roots(&request.stage_id);
    if request.mode == "single" && !request.spawn_room_npcs {
        static EXTRAS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        if EXTRAS.get().is_none() {
            if let Some(index) = meteor_asset::runtime_assets::manifest()
                .and_then(|text| parse_pack_index(&text).ok())
            {
                let extra = (20..=u8::MAX)
                    .map(|id| format!("char/p{id}"))
                    .filter(|id| index.contains_key(id))
                    .collect();
                let _ = EXTRAS.set(extra);
            }
        }
        roots.extend(EXTRAS.get().into_iter().flatten().cloned());
    }
    roots
}

#[cfg(any(target_os = "android", target_arch = "wasm32", test))]
pub(crate) fn match_packs_resident(
    roots: &[String],
    mut contains: impl FnMut(&str) -> bool,
) -> bool {
    roots.iter().all(|id| contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn same_stage_requires_reload_after_returning_to_menu() {
        let mut resident: HashSet<String> = match_pack_roots("sn01").into_iter().collect();
        let roots = match_pack_roots("sn01");
        assert!(match_packs_resident(&roots, |id| resident.contains(id)));
        resident.retain(|id| !id.starts_with("stage/"));
        assert!(!match_packs_resident(&roots, |id| resident.contains(id)));
        resident.insert("stage/sn01".into());
        assert!(match_packs_resident(&roots, |id| resident.contains(id)));
        assert!(!match_packs_resident(&match_pack_roots("sn02"), |id| {
            resident.contains(id)
        }));
    }
}
