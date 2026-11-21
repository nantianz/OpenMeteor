use flate2::{write::GzEncoder, Compression};
use meteor_asset::mrc::{codes, decode_mrc, MrcDocument, MrcQueueAEntry};
use meteor_network::{
    role_state::{decode_event2_state, RoleVec3, ROLE_WORLD_SCALE},
    stock_protocol::{
        decode_stock_roster, StockHitReactionEvent, StockRoleEnteredEvent, StockWeaponEquippedEvent,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, BufWriter, Write},
    path::Path,
};

type Counts<K> = BTreeMap<K, usize>;

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn actor(id: u32) -> Option<u32> {
    (1..=64).contains(&id).then_some(id)
}

fn checked_decode(bytes: &[u8]) -> Result<MrcDocument, String> {
    if bytes.len() < 92 || word(bytes, 0) != 0x1e0d || word(bytes, 4) != 0x28 {
        return Err("invalid MRC header".into());
    }
    if (word(bytes, 8), word(bytes, 12)) != (9, 7) {
        return Err("expected a stock 9.07 MRC".into());
    }
    let mut remainder = &bytes[92..];
    for _ in 0..word(bytes, 16) {
        if remainder.len() < 12 {
            return Err("truncated QueueA record".into());
        }
        remainder = remainder[12..]
            .get(word(remainder, 8) as usize..)
            .ok_or("truncated QueueA payload")?;
    }
    let fixed_size: u64 = [12, 8, 16, 12]
        .into_iter()
        .enumerate()
        .map(|(i, size)| u64::from(word(bytes, 20 + 4 * i)) * size)
        .sum();
    if remainder.len() as u64 != fixed_size {
        return Err("QueueB–E size does not match the file".into());
    }
    decode_mrc(bytes)
}

fn weapon_class(item: u32) -> Option<u32> {
    match item {
        1..=46 => Some((item - 1) % 9 + 1),
        47..=58 => Some(10 + (item - 47) / 4),
        59 => Some(2),
        _ => None,
    }
}

fn vec3(v: RoleVec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

fn weapon_change(local: Option<u32>, id: u32, item: u32, weapon: &mut Option<u32>) {
    if local == actor(id) && local.is_some() {
        *weapon = Some(item);
    }
}

fn event(
    entry: &MrcQueueAEntry,
    local: Option<u32>,
    weapon: &mut Option<u32>,
    observed: &mut BTreeSet<u32>,
) -> Option<(&'static str, Value)> {
    let b = &entry.blob;
    Some(match entry.id {
        2 if !b.is_empty() && b.len() % 16 == 0 => {
            let snapshots: Vec<_> = b.chunks_exact(16).filter_map(|chunk| {
                let id = actor(u32::from(half(chunk, 0)))?;
                let s = decode_event2_state(&chunk[2..]).ok()?;
                observed.insert(id);
                let w = s.state_words;
                Some(json!({"actor_id":id,"position":vec3(s.position),"forward":vec3(s.forward),
                    "velocity":vec3(s.velocity),"pose_id":w.pose_id,"pose_sequence":w.pose_sequence,
                    "grounded":w.grounded,"guard_route":w.guard_route,"playback_arg":w.playback_arg}))
            }).collect();
            if snapshots.is_empty() {
                return None;
            }
            ("snapshot", json!({"actors":snapshots}))
        }
        4 => {
            let members: Vec<_> = decode_stock_roster(b)
                .ok()?
                .into_iter()
                .filter_map(|m| {
                    let id = actor(m.actor_id)?;
                    if m.entered {
                        weapon_change(local, id, m.weapon_item_id, weapon);
                    }
                    Some(
                        json!({"actor_id":id,"hero":m.hero,"entered":m.entered,"camp":m.camp,
                    "weapon_item_id":m.weapon_item_id}),
                    )
                })
                .collect();
            ("roster", json!({"actors":members}))
        }
        9 => {
            let m = StockRoleEnteredEvent::decode(b).ok()?;
            let id = actor(m.actor_id)?;
            weapon_change(local, id, m.hero_checksum, weapon);
            (
                "role_entered",
                json!({"actor_id":id,"hero":m.hero,"camp":m.camp,"weapon_item_id":m.hero_checksum}),
            )
        }
        10 if b.len() == 16 => {
            let stage = b.get(..4)?;
            if !stage.iter().all(u8::is_ascii_alphanumeric) {
                return None;
            }
            (
                "stage",
                json!({"stage": String::from_utf8_lossy(stage).to_ascii_lowercase()}),
            )
        }
        11 | 14 => {
            let h = StockHitReactionEvent::decode(b).ok()?;
            let id = actor(h.target_actor_id)?;
            let guarded = entry.id == 14;
            let (pose, direction) = if guarded {
                (h.pose_id >> 16, Some(h.pose_id as u16))
            } else {
                (h.pose_id, None)
            };
            (
                if guarded { "guarded" } else { "hit" },
                json!({"target_actor_id":id,
                "reaction_pose_id":pose,"direction_selector":direction,
                "move_scale":h.motion_scale(),"minimum_seconds":h.minimum_seconds(),
                "weapon_strategy":h.context}),
            )
        }
        12 => {
            let e = StockWeaponEquippedEvent::decode(b).ok()?;
            let id = actor(e.actor_id)?;
            weapon_change(local, id, e.weapon_item_id, weapon);
            (
                "equip",
                json!({"actor_id":id,"weapon_item_id":e.weapon_item_id}),
            )
        }
        25 if b.len() == 6 => (
            "vitals",
            json!({"actor_id":local,"hp":half(b,0),
            "max_hp":half(b,2),"anger":half(b,4)}),
        ),

        50 | 51 if b.len() == 10 => {
            let id = actor(u32::from(half(b, 0)))?;
            (
                if entry.id == 50 {
                    "action_start"
                } else {
                    "action_chain"
                },
                json!({
                "actor_id":id,"pose_id":half(b,2),"word4":half(b,4),"sequence":half(b,6),
                "blend_seconds":f32::from(half(b,8) as i16)*0.01}),
            )
        }
        _ => return None,
    })
}

#[derive(Default)]
struct RecordingStats {
    rows: Counts<&'static str>,
    event_ids: Counts<u32>,
    omitted_selected_events: Counts<u32>,
    attacks_by_weapon: Counts<u32>,
    observed: BTreeSet<u32>,
    stage: Option<String>,
    duration: f32,
    local: Option<u32>,
}

fn export_recording(doc: &MrcDocument, mut out: impl Write) -> io::Result<RecordingStats> {
    let mut s = RecordingStats {
        local: actor(word(&doc.state, 4)),
        ..Default::default()
    };
    let mut weapon = Some(word(&doc.state, 0));

    let mut order = Vec::new();
    macro_rules! append {
        ($queue:ident, $rank:literal) => {
            order.extend(
                doc.$queue
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (e.time, $rank, i)),
            );
        };
    }
    append!(queue_b, 0);
    append!(queue_a, 1);
    append!(queue_c, 2);
    append!(queue_d, 3);
    append!(queue_e, 4);
    if order.iter().any(|(t, _, _)| !t.is_finite()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "non-finite timestamp",
        ));
    }
    order.sort_unstable_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap()
            .then_with(|| (a.1, a.2).cmp(&(b.1, b.2)))
    });
    for (t, queue, i) in order {
        s.duration = s.duration.max(t);
        let (kind, mut data) = match queue {
            0 => {
                let e = &doc.queue_b[i];
                let class = weapon.and_then(weapon_class);
                if e.code == codes::ATTACK && e.value == 1 {
                    *s.attacks_by_weapon.entry(class.unwrap_or(0)).or_default() += 1;
                }
                (
                    "input",
                    json!({"actor_id":s.local,"code":e.code,"value":e.value,
                    "weapon_item_id_before":weapon,"weapon_class_before":class}),
                )
            }
            1 => {
                let e = &doc.queue_a[i];
                *s.event_ids.entry(e.id).or_default() += 1;
                let Some((kind, mut data)) = event(e, s.local, &mut weapon, &mut s.observed) else {
                    if matches!(e.id, 2 | 4 | 9..=12 | 14 | 25 | 50 | 51) {
                        *s.omitted_selected_events.entry(e.id).or_default() += 1;
                    }
                    continue;
                };
                data["event_id"] = json!(e.id);
                if kind == "stage" && s.stage.is_none() {
                    s.stage = data["stage"].as_str().map(str::to_owned);
                }
                (kind, data)
            }
            2 => {
                let p = doc.queue_c[i].packed;
                (
                    "move_axes",
                    json!({"actor_id":s.local,"packed":p,"forward":(p&255) as i32-1,
                    "strafe":((p>>8)&255) as i32-1}),
                )
            }
            3 => {
                let e = &doc.queue_d[i];
                let position = [e.a, e.c, e.b].map(|bits| f32::from_bits(bits) * ROLE_WORLD_SCALE);
                (
                    "local_position",
                    json!({"actor_id":s.local,"position":position}),
                )
            }
            4 => {
                let e = &doc.queue_e[i];
                let [x, z] = [e.a, e.b].map(f32::from_bits);
                (
                    "local_forward",
                    json!({"actor_id":s.local,"forward_xz":[x,z]}),
                )
            }
            _ => unreachable!(),
        };
        data["kind"] = json!(kind);
        data["t"] = json!(t);
        data["source_queue"] = json!(["B", "A", "C", "D", "E"][queue]);
        data["source_index"] = json!(i);
        serde_json::to_writer(&mut out, &data)?;
        out.write_all(b"\n")?;
        *s.rows.entry(kind).or_default() += 1;
    }
    out.flush()?;
    Ok(s)
}

fn recording_paths(source: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mrc"))
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn run(source: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let paths = recording_paths(source)?;
    if paths.is_empty() {
        return Err("no top-level .mrc recordings found".into());
    }
    if output.exists() {
        return Err("output directory already exists; choose a new one".into());
    }
    fs::create_dir_all(output.join("recordings"))?;
    let mut seen = BTreeMap::new();
    let mut recordings = Vec::new();
    let mut duplicates = Vec::new();
    let mut totals = Counts::new();
    let mut weapon_totals = Counts::new();
    let mut weapon_files = Counts::new();
    let mut stages = Counts::new();
    let mut duration = 0.0;
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy();
        let bytes = fs::read(&path)?;
        let sha = format!("{:x}", Sha256::digest(&bytes));
        if let Some(first) = seen.get(&sha) {
            duplicates.push(json!({"file":name,"duplicate_of":first,"sha256":sha}));
            continue;
        }
        let doc = checked_decode(&bytes).map_err(|err| format!("{}: {err}", path.display()))?;
        let relative = format!("recordings/{sha}.jsonl.gz");
        let file = fs::File::create(output.join(&relative))?;
        let mut gz = GzEncoder::new(file, Compression::fast());
        let s = export_recording(&doc, BufWriter::new(&mut gz))?;
        gz.finish()?;
        seen.insert(sha.clone(), name.to_string());
        duration += f64::from(s.duration);
        *stages
            .entry(s.stage.clone().unwrap_or_else(|| "unknown".into()))
            .or_default() += 1;
        for (&kind, count) in &s.rows {
            *totals.entry(kind).or_default() += count;
        }
        for (&class, count) in &s.attacks_by_weapon {
            *weapon_files.entry(class).or_default() += 1;
            *weapon_totals.entry(class).or_default() += count;
        }
        recordings.push(json!({"file":name,"sha256":sha,"timeline":relative,"duration_seconds":s.duration,
            "stage":s.stage,"recording_actor_id":s.local,"snapshot_actor_ids":s.observed,
            "rows":s.rows,"raw_event_counts":s.event_ids,"omitted_selected_event_rows":s.omitted_selected_events,
            "attack_presses_by_weapon_class":s.attacks_by_weapon}));
    }
    write_json(
        output.join("coverage.json"),
        &json!({"schema_version":1,"recordings":recordings.len(),
        "duplicates":duplicates.len(),"duration_seconds":duration,"stages":stages,"rows":totals,
        "attack_presses_by_weapon_class":weapon_totals,"recordings_by_attack_weapon_class":weapon_files}),
    )?;
    write_json(
        output.join("manifest.json"),
        &json!({"schema_version":1,"scope":"top_level_only",
        "recordings":recordings,"duplicates":duplicates}),
    )?;
    println!(
        "Exported {} unique recordings ({:.2} hours), {} exact duplicates skipped, to {}",
        recordings.len(),
        duration / 3600.0,
        duplicates.len(),
        output.display()
    );
    Ok(())
}

fn write_json(path: impl AsRef<Path>, value: &Value) -> io::Result<()> {
    let mut file = BufWriter::new(fs::File::create(path)?);
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [source, output] => run(Path::new(source), Path::new(output)),
        _ => Err("usage: mrc-duel-export <mrcs directory> <new output directory>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_asset::mrc::{encode_mrc, MrcQueueBEntry};

    #[test]
    fn export_keeps_inputs_local_and_does_not_use_tied_equipment() {
        let mut header = [0u8; 92];
        for (i, value) in [0x1e0du32, 0x28, 9, 7].into_iter().enumerate() {
            header[4 * i..4 * i + 4].copy_from_slice(&value.to_le_bytes());
        }
        header[40] = 5;
        header[44] = 2;
        let mut doc = checked_decode(&header).unwrap();
        let equip = StockWeaponEquippedEvent {
            actor_id: 2,
            weapon_item_id: 24,
        };
        doc.queue_a.push(MrcQueueAEntry {
            id: 12,
            time: 1.0,
            blob: equip.encode().to_vec(),
        });
        for time in [1.0, 2.0] {
            doc.queue_b.push(MrcQueueBEntry {
                time,
                code: codes::ATTACK,
                value: 1,
            });
        }
        let bytes = encode_mrc(&doc);
        let doc = checked_decode(&bytes).unwrap();
        assert!(checked_decode(&bytes[..bytes.len() - 1]).is_err());
        let mut out = Vec::new();
        let stats = export_recording(&doc, &mut out).unwrap();
        let out: Vec<Value> = std::str::from_utf8(&out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(out[0]["actor_id"], 2);
        assert_eq!(out[0]["weapon_class_before"], 5);
        assert_eq!(out[1]["kind"], "equip");
        assert_eq!(out[2]["weapon_class_before"], 6);
        assert_eq!(stats.attacks_by_weapon, BTreeMap::from([(5, 1), (6, 1)]));
    }
}
