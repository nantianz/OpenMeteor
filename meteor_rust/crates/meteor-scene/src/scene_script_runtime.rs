pub use crate::pst_runtime::PstValue;
use crate::pst_runtime::{parse_program, PstHost, Runtime};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub enum SceneNetOp {
    Set {
        ref_index: usize,
        field: String,
        args: Vec<PstValue>,
    },
    Effect {
        ref_index: usize,
        effect: String,
        extra: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneTransferRequest {
    pub character_id: i64,
    pub destination: String,
}

#[derive(Debug, Clone, Default)]
pub struct SceneItemInstance {
    pub name: String,

    pub index: usize,

    pub props: HashMap<String, PstValue>,
}

impl SceneItemInstance {
    pub fn get(&self, key: &str) -> PstValue {
        self.props
            .get(&key.to_ascii_lowercase())
            .cloned()
            .unwrap_or(PstValue::Num(0.0))
    }

    pub fn set(&mut self, key: &str, value: PstValue) {
        if key.eq_ignore_ascii_case("active") {
            self.props.insert("_active_override".into(), value.clone());
        }
        self.props.insert(key.to_ascii_lowercase(), value);
    }

    pub fn collision_enabled(&self) -> bool {
        self.get("collision").as_f64() != 0.0
            && self
                .props
                .get("_active_override")
                .is_none_or(|v| v.as_f64() != 0.0)
    }
}

#[derive(Default)]
pub struct SceneHost {
    instances: Vec<SceneItemInstance>,
    by_name: HashMap<String, usize>,
    effects: Vec<SceneNetOp>,
    output: Vec<String>,
    net_event: bool,
    net_ops: Option<Vec<SceneNetOp>>,
    committed_net_batches: VecDeque<Vec<SceneNetOp>>,

    character_teams: HashMap<i64, i64>,

    game_mode: i64,
    game_time_ms: f64,
    series: HashMap<i64, Vec<i64>>,
    transfer_requests: Vec<SceneTransferRequest>,

    scene_requests: Vec<(String, Vec<f64>)>,

    game_end: Option<i64>,

    rng: u64,
}

impl SceneHost {
    pub fn new(instances: Vec<SceneItemInstance>) -> Self {
        let mut by_name = HashMap::new();
        for (index, instance) in instances.iter().enumerate() {
            by_name.insert(instance.name.to_lowercase(), index);
        }
        Self {
            instances,
            by_name,
            character_teams: HashMap::from([(0, 0)]),
            game_mode: 1,
            rng: 0x9e3779b97f4a7c15,
            ..Default::default()
        }
    }

    pub fn instances(&self) -> &[SceneItemInstance] {
        &self.instances
    }

    pub fn instances_mut(&mut self) -> &mut [SceneItemInstance] {
        &mut self.instances
    }

    pub fn instance(&self, index: usize) -> Option<&SceneItemInstance> {
        self.instances.get(index)
    }

    pub fn instance_mut(&mut self, index: usize) -> Option<&mut SceneItemInstance> {
        self.instances.get_mut(index)
    }

    pub fn take_effects(&mut self) -> Vec<SceneNetOp> {
        std::mem::take(&mut self.effects)
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }

    pub fn scene_requests(&self) -> &[(String, Vec<f64>)] {
        &self.scene_requests
    }

    pub fn take_scene_requests(&mut self) -> Vec<(String, Vec<f64>)> {
        std::mem::take(&mut self.scene_requests)
    }

    pub fn game_end(&self) -> Option<i64> {
        self.game_end
    }

    pub fn take_net_ops(&mut self) -> Vec<SceneNetOp> {
        self.take_net_batches().into_iter().flatten().collect()
    }

    pub fn take_net_batches(&mut self) -> Vec<Vec<SceneNetOp>> {
        self.committed_net_batches.drain(..).collect()
    }

    pub fn set_character_team(&mut self, character_id: i64, team: i64) {
        self.character_teams.insert(character_id, team);
    }

    pub fn set_game_mode(&mut self, mode: i64) {
        self.game_mode = mode;
    }

    pub fn take_transfer_requests(&mut self) -> Vec<SceneTransferRequest> {
        std::mem::take(&mut self.transfer_requests)
    }

    pub fn apply_net_op(&mut self, op: &SceneNetOp) {
        match op {
            SceneNetOp::Set {
                ref_index,
                field,
                args,
            } => {
                let reference = PstValue::Num(*ref_index as f64);
                let field = PstValue::Str(field.clone());
                self.set(&reference, &field, args);
            }
            SceneNetOp::Effect { .. } => {
                self.effects.push(op.clone());
                if self.effects.len() > 64 {
                    self.effects.remove(0);
                }
            }
        }
    }

    fn next_rand(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let value = x.wrapping_mul(0x2545f4914f6cdd1d);
        (value >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn resolve_name(&self, key: &str) -> Option<usize> {
        let lower = key.to_lowercase();
        if let Some(&index) = self.by_name.get(&lower) {
            return Some(index);
        }
        let start = lower.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1)?;
        if start >= lower.len() {
            return None;
        }
        let (stem, digits) = lower.split_at(start);
        let Ok(n) = digits.parse::<i64>() else {
            return None;
        };
        for candidate in [
            format!("{stem}{:02}", n),
            format!("{stem}{:03}", n),
            format!("{stem}{n}"),
        ] {
            if let Some(&index) = self.by_name.get(&candidate) {
                return Some(index);
            }
        }
        None
    }

    fn entry(&self, reference: &PstValue) -> Option<usize> {
        match reference {
            PstValue::Num(n) => {
                let index = n.trunc() as usize;
                self.instances.get(index).map(|_| index)
            }
            PstValue::Str(key) => self.resolve_name(key),
            _ => None,
        }
    }

    fn set(&mut self, reference: &PstValue, field: &PstValue, args: &[PstValue]) -> PstValue {
        let Some(index) = self.entry(reference) else {
            return PstValue::Num(0.0);
        };
        let key = field_to_string(field).to_lowercase();
        let value = args.first().cloned().unwrap_or(PstValue::Num(0.0));
        match key.as_str() {
            "attribute" => {
                let attribute =
                    field_to_string(args.first().unwrap_or(&PstValue::Num(0.0))).to_lowercase();
                let value = args.get(1).cloned().unwrap_or(PstValue::Num(0.0));
                self.instance_mut(index)
                    .expect("entry checked")
                    .set(&attribute, value);
            }
            "pose" => {
                let instance = self.instance_mut(index).expect("entry checked");
                let pose = args.first().map(|v| v.as_f64()).unwrap_or(0.0) as i64;
                let loop_flag = args.get(1).map(|v| v.as_f64()).unwrap_or(0.0) as i64;
                let revision = instance.get("_pose_revision").as_f64().max(0.0) + 1.0;
                let has_fmc = instance.get("_has_fmc").as_f64() != 0.0;
                instance.set("pose", PstValue::Num(pose as f64));
                instance.set("pose_time", PstValue::Num(loop_flag as f64));
                instance.set("pose_loop", PstValue::Num(loop_flag as f64));
                instance.set("anim_time", PstValue::Num(0.0));
                instance.set("state", PstValue::Num(if has_fmc { 1.0 } else { 3.0 }));

                instance.set("_pose_revision", PstValue::Num(revision));
            }
            "frame" => {
                let frame = args.first().map(|v| v.as_f64()).unwrap_or(0.0) as i64;
                self.instance_mut(index)
                    .expect("entry checked")
                    .set("frame", PstValue::Num(frame as f64));
            }
            "name" => {
                let name = field_to_string(args.first().unwrap_or(&PstValue::Num(0.0)));
                self.instance_mut(index)
                    .expect("entry checked")
                    .set("script_type", PstValue::Str(name));
            }
            _ => {
                self.instance_mut(index)
                    .expect("entry checked")
                    .set(&key, value);
            }
        }
        PstValue::Num(1.0)
    }

    fn get(&self, reference: &PstValue, field: &PstValue) -> PstValue {
        let Some(index) = self.entry(reference) else {
            return PstValue::Num(-1.0);
        };
        let key = field_to_string(field).to_lowercase();
        let instance = self.instance(index).expect("entry checked");
        match key.as_str() {
            "index" => PstValue::Num(index as f64),
            "state" => instance.get("state"),
            "pose" => instance.get("pose"),
            _ => instance.get(&key),
        }
    }
}

fn field_to_string(value: &PstValue) -> String {
    match value {
        PstValue::Str(s) => s.clone(),
        PstValue::Num(n) => {
            if n.fract().abs() < 1e-9 {
                format!("{}", n.trunc() as i64)
            } else {
                format!("{n}")
            }
        }
        PstValue::Array(_) => String::new(),
    }
}

impl PstHost for SceneHost {
    fn builtin(&mut self, name: &str, args: &[PstValue]) -> PstValue {
        match name {
            "GetSceneItem" => {
                let (reference, field) = (args.first(), args.get(1));
                match (reference, field) {
                    (Some(r), Some(f)) => self.get(r, f),
                    _ => PstValue::Num(0.0),
                }
            }
            "SetSceneItem" => {
                let (reference, field) = (args.first(), args.get(1));
                let ref_index = reference.and_then(|reference| self.entry(reference));
                let result = match (reference, field) {
                    (Some(r), Some(f)) => self.set(r, f, &args[2..]),
                    _ => PstValue::Num(0.0),
                };

                if self.net_event {
                    if let (Some(ref_index), Some(f)) = (ref_index, field) {
                        if let Some(ops) = self.net_ops.as_mut() {
                            if ops.len() < 64 {
                                ops.push(SceneNetOp::Set {
                                    ref_index,
                                    field: field_to_string(f),
                                    args: args[2..].to_vec(),
                                });
                            }
                        }
                    }
                }
                result
            }
            "NetEvent" => {
                let enabled = args.first().map(|v| v.as_f64() != 0.0).unwrap_or(false);
                if enabled == self.net_event {
                    return PstValue::Num(1.0);
                }
                self.net_event = enabled;
                if enabled {
                    self.net_ops = Some(Vec::new());
                } else if let Some(ops) = self.net_ops.take() {
                    if !ops.is_empty() {
                        self.committed_net_batches.push_back(ops);
                    }
                }
                PstValue::Num(1.0)
            }
            "CreateEffect" => {
                let reference = args.first().cloned().unwrap_or(PstValue::Num(0.0));
                let effect = field_to_string(args.get(1).unwrap_or(&PstValue::Num(0.0)));
                let extra = args.get(2).map(|v| v.as_f64()).unwrap_or(0.0);
                let op = SceneNetOp::Effect {
                    ref_index: reference.as_f64() as usize,
                    effect,
                    extra,
                };
                self.effects.push(op.clone());
                if self.effects.len() > 64 {
                    self.effects.remove(0);
                }
                if self.net_event {
                    if let Some(ops) = self.net_ops.as_mut() {
                        if ops.len() < 64 {
                            ops.push(op);
                        }
                    }
                }
                PstValue::Num(1.0)
            }
            "Output" | "Print" => {
                let line = args
                    .iter()
                    .map(field_to_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.output.push(line);
                PstValue::Num(1.0)
            }
            "Misc" => {
                let cmd =
                    field_to_string(args.first().unwrap_or(&PstValue::Num(0.0))).to_lowercase();
                match cmd.as_str() {
                    "gettime" => PstValue::Num(self.game_time_ms.floor()),
                    "transfer" => {
                        let character_id = args.get(1).map(PstValue::as_f64).unwrap_or(-1.0) as i64;
                        let destination =
                            field_to_string(args.get(2).unwrap_or(&PstValue::Str(String::new())));
                        if !destination.is_empty() && self.transfer_requests.len() < 64 {
                            self.transfer_requests.push(SceneTransferRequest {
                                character_id,
                                destination,
                            });
                        }
                        PstValue::Num(1.0)
                    }
                    "randomseries" => {
                        let slot = args.get(1).map(|v| v.as_f64() as i64).unwrap_or(0);
                        let lo = args.get(2).map(|v| v.as_f64() as i64).unwrap_or(0);
                        let hi = args.get(3).map(|v| v.as_f64() as i64).unwrap_or(0);
                        let mut values: Vec<i64> = (lo..=hi).collect();

                        for i in (1..values.len()).rev() {
                            let j = (self.next_rand() * (i as f64 + 1.0)) as usize;
                            values.swap(i, j);
                        }
                        self.series.insert(slot, values);
                        PstValue::Num(1.0)
                    }
                    "getseries" => {
                        let slot = args.get(1).map(|v| v.as_f64() as i64).unwrap_or(0);
                        let idx = args.get(2).map(|v| v.as_f64() as i64).unwrap_or(0);
                        PstValue::Num(
                            self.series
                                .get(&slot)
                                .and_then(|values| values.get(idx as usize))
                                .copied()
                                .unwrap_or(0) as f64,
                        )
                    }
                    _ => PstValue::Num(0.0),
                }
            }
            "GetTeam" => {
                let character_id = args.first().map(PstValue::as_f64).unwrap_or(-1.0) as i64;
                PstValue::Num(
                    self.character_teams
                        .get(&character_id)
                        .copied()
                        .unwrap_or(-1) as f64,
                )
            }
            "GameCallBack" => {
                let cmd =
                    field_to_string(args.first().unwrap_or(&PstValue::Num(0.0))).to_lowercase();
                match cmd.as_str() {
                    "mod" => PstValue::Num(self.game_mode as f64),
                    "end" => {
                        self.game_end = Some(args.get(1).map(|v| v.as_f64() as i64).unwrap_or(0));
                        PstValue::Num(1.0)
                    }
                    _ => PstValue::Num(0.0),
                }
            }
            "SetScene" => {
                let key =
                    field_to_string(args.first().unwrap_or(&PstValue::Num(0.0))).to_lowercase();
                let values: Vec<f64> = args[1..].iter().map(|v| v.as_f64()).collect();
                self.scene_requests.push((key, values));
                PstValue::Num(1.0)
            }
            "Rand" | "rand" => {
                let n = args.first().map(|v| v.as_f64()).unwrap_or(0.0);
                if let Some(m) = args.get(1) {
                    let lo = n as i64;
                    let hi = m.as_f64() as i64;
                    let a = lo.min(hi);
                    let b = lo.max(hi);
                    PstValue::Num((a + (self.next_rand() * (b - a + 1) as f64) as i64) as f64)
                } else {
                    PstValue::Num((self.next_rand() * (n as i64).max(1) as f64) as i64 as f64)
                }
            }
            _ => PstValue::Num(0.0),
        }
    }
}

pub struct SceneScriptRuntime {
    runtime: Runtime<SceneHost>,
    update_accumulator: f64,
}

impl SceneScriptRuntime {
    pub fn load(
        source: &str,
        instances: Vec<SceneItemInstance>,
        max_steps: usize,
    ) -> Result<Self, String> {
        let program = parse_program(source)?;
        let host = SceneHost::new(instances);
        Ok(Self {
            runtime: Runtime::new(program, host, max_steps),
            update_accumulator: 0.0,
        })
    }

    pub fn start(&mut self) -> Result<(), String> {
        for name in ["Scene_OnLoad", "Scene_OnInit"] {
            if self.runtime.has_function(name) {
                self.runtime.call(name, &[])?;
            }
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), String> {
        self.call("Scene_OnClose", &[]).map(|_| ())
    }

    pub fn call(&mut self, name: &str, args: &[PstValue]) -> Result<PstValue, String> {
        if self.runtime.has_function(name) {
            self.runtime.call(name, args)
        } else {
            Ok(PstValue::Num(0.0))
        }
    }

    pub fn has_item_callback(&self, name: &str, event_name: &str) -> bool {
        self.callback_names(name).iter().any(|candidate| {
            self.runtime
                .has_function(&format!("{candidate}_{event_name}"))
        })
    }

    pub fn call_item(
        &mut self,
        name: &str,
        event_name: &str,
        args: &[PstValue],
    ) -> Result<PstValue, String> {
        let Some(index) = self.runtime.host().resolve_name(name) else {
            return Ok(PstValue::Num(0.0));
        };
        for candidate in self.callback_names(name) {
            let callback = format!("{candidate}_{event_name}");
            if self.runtime.has_function(&callback) {
                let mut call_args = vec![PstValue::Num(index as f64)];
                call_args.extend_from_slice(args);
                return self.runtime.call(&callback, &call_args);
            }
        }
        Ok(PstValue::Num(0.0))
    }

    pub fn set_item_property(&mut self, name: &str, key: &str, value: PstValue) -> bool {
        let Some(index) = self.runtime.host().resolve_name(name) else {
            return false;
        };
        let Some(instance) = self.runtime.host_mut().instance_mut(index) else {
            return false;
        };
        instance.set(key, value);
        true
    }

    fn callback_names(&self, name: &str) -> Vec<String> {
        let mut names = vec![name.to_string()];
        let digits_start = name.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1);
        if let Some(start) = digits_start {
            if start < name.len() {
                let (stem, digits) = name.split_at(start);
                if let Ok(n) = digits.parse::<i64>() {
                    let padded = format!("{stem}{:02}", n);
                    if !padded.eq_ignore_ascii_case(name) {
                        names.push(padded);
                    }
                }
            }
        }
        names
    }

    pub fn update(&mut self, dt: f64) -> Result<(), String> {
        self.runtime.host_mut().game_time_ms += dt.max(0.0) * 1000.0;
        self.update_accumulator += dt;
        if self.update_accumulator < 0.1 {
            return Ok(());
        }
        self.update_accumulator %= 0.1;
        if self.runtime.has_function("Scene_OnIdle") {
            self.runtime.call("Scene_OnIdle", &[])?;
        }
        let instance_names: Vec<String> = self
            .runtime
            .host()
            .instances()
            .iter()
            .map(|instance| instance.name.clone())
            .collect();
        for name in instance_names {
            self.call_item(&name, "OnIdle", &[])?;
        }
        Ok(())
    }

    pub fn host(&self) -> &SceneHost {
        self.runtime.host()
    }

    pub fn host_mut(&mut self) -> &mut SceneHost {
        self.runtime.host_mut()
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    fn instance(name: &str, index: usize) -> SceneItemInstance {
        SceneItemInstance {
            name: name.to_string(),
            index,
            props: HashMap::new(),
        }
    }

    fn load(source: &str, instances: Vec<SceneItemInstance>) -> SceneScriptRuntime {
        SceneScriptRuntime::load(source, instances, 250_000).expect("load")
    }

    #[test]
    fn static_set_scene_item_applies_attributes() {
        let instances = vec![instance("D_BBox01", 0), instance("D_St01", 1)];
        let source = r#"
Scene_OnLoad()
{
    SetSceneItem(name, "attribute", "hp", 500);
    SetSceneItem("D_BBox01", "visible", 0);
    SetSceneItem(1, "name", "stone");
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        let host = runtime.host();
        assert_eq!(host.instance(0).unwrap().get("hp").as_f64(), 500.0);
        assert_eq!(host.instance(0).unwrap().get("visible").as_f64(), 0.0);
        assert_eq!(
            host.instance(1).unwrap().get("script_type"),
            PstValue::Str("stone".into())
        );
    }

    #[test]
    fn close_calls_scene_on_close_and_exposes_environment_requests() {
        let mut runtime = load(
            r#"
Scene_OnClose()
{
    SetScene("snow", 0);
    SetSceneItem(0, "active", 0);
}
"#,
            vec![instance("D_snow01", 0)],
        );

        runtime.close().expect("close");

        assert_eq!(
            runtime.host().instance(0).expect("instance").get("active"),
            PstValue::Num(0.0)
        );
        assert_eq!(
            runtime.host_mut().take_scene_requests(),
            vec![("snow".into(), vec![0.0])]
        );
    }

    #[test]
    fn get_scene_item_queries_state_and_pose() {
        let instances = vec![instance("D_Box01", 0)];
        let source = r#"
Scene_OnLoad()
{
    SetSceneItem(0, "pose", 12, 1);
}
Scene_Check()
{
    int idx = GetSceneItem(0, "index");
    int pose = GetSceneItem(0, "pose");
    int missing = GetSceneItem("NoSuch01", "index");
}
Scene_Replay()
{
    SetSceneItem(0, "pose", 12, 1);
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        runtime.call("Scene_Check", &[]).expect("call");
        let host = runtime.host();
        assert_eq!(host.instance(0).unwrap().get("pose").as_f64(), 12.0);

        assert_eq!(host.instance(0).unwrap().get("state").as_f64(), 3.0);
        assert_eq!(
            host.instance(0).unwrap().get("_pose_revision").as_f64(),
            1.0
        );
        runtime.call("Scene_Replay", &[]).expect("replay pose");
        assert_eq!(
            runtime
                .host()
                .instance(0)
                .unwrap()
                .get("_pose_revision")
                .as_f64(),
            2.0
        );
    }

    #[test]
    fn set_pose_enters_playing_immediately_for_registered_fmc() {
        let mut item = instance("D_Box01", 0);
        item.set("_has_fmc", PstValue::Num(1.0));
        let mut runtime = load(
            r#"Scene_OnLoad() { SetSceneItem(0, "pose", 1, 0); }"#,
            vec![item],
        );
        runtime.start().expect("start");
        let item = runtime.host().instance(0).expect("instance");
        assert_eq!(item.get("pose"), PstValue::Num(1.0));
        assert_eq!(item.get("state"), PstValue::Num(1.0));
    }

    #[test]
    fn make_string_padded_entry_resolves() {
        let instances = vec![instance("D_BBox01", 0)];
        let source = r#"
Scene_OnLoad()
{
    string s;
    MakeString(s, "D_BBox", 1);
    SetSceneItem(s, "active", 0);
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        assert_eq!(
            runtime.host().instance(0).unwrap().get("active").as_f64(),
            0.0
        );
    }

    #[test]
    fn item_callback_naming_convention() {
        let instances = vec![instance("D_tpAC01", 0)];
        let source = r#"
D_tpAC01_OnTouch(int idx)
{
    SetSceneItem(idx, "active", 0);
}
"#;
        let mut runtime = load(source, instances);
        assert!(runtime.has_item_callback("D_tpAC01", "OnTouch"));

        assert!(runtime.has_item_callback("D_tpAC1", "OnTouch"));
        runtime
            .call_item("D_tpAC1", "OnTouch", &[])
            .expect("call item");
        assert_eq!(
            runtime.host().instance(0).unwrap().get("active").as_f64(),
            0.0
        );
    }

    #[test]
    fn update_ticks_on_idle_interval() {
        let instances = vec![instance("D_Box01", 0)];
        let source = r#"
int idle_count = 0;
Scene_OnIdle() { idle_count = idle_count + 1; SetSceneItem(0, "ticks", idle_count); }
"#;
        let mut runtime = load(source, instances);

        runtime.update(0.1).expect("tick");
        runtime.update(0.1).expect("tick");
        runtime.update(0.05).expect("tick");
        assert_eq!(
            runtime.host().instance(0).unwrap().get("ticks").as_f64(),
            2.0
        );
    }

    #[test]
    fn net_event_collects_ops() {
        let instances = vec![instance("D_Box01", 0)];
        let source = r#"
Scene_OnLoad()
{
    NetEvent(1);
    SetSceneItem(0, "hp", 100);
    NetEvent(0);
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        let ops = runtime.host_mut().take_net_ops();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], SceneNetOp::Set { ref_index: 0, field, .. } if field == "hp"));
        assert_eq!(
            runtime.host().instance(0).unwrap().get("hp").as_f64(),
            100.0
        );
    }

    #[test]
    fn rand_and_randomseries_deterministic() {
        let instances = vec![instance("D_Box01", 0)];
        let source = r#"
Scene_OnLoad()
{
    int a = Rand(1, 6);
    int b = rand(10);
    Misc("randomseries", 1, 0, 4);
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        let _ = runtime;

        let mut runtime_a = load(source, vec![instance("D_Box01", 0)]);
        let mut runtime_b = load(source, vec![instance("D_Box01", 0)]);
        runtime_a.start().expect("start");
        runtime_b.start().expect("start");

        assert!(runtime_a.host().output().is_empty());
        assert!(runtime_b.host().output().is_empty());
    }

    #[test]
    fn game_call_back_end_records_winner() {
        let instances = vec![instance("D_Box01", 0)];
        let source = r#"
Scene_OnLoad()
{
    GameCallBack("end", 2);
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        assert_eq!(runtime.host().game_end(), Some(2));
    }

    #[test]
    fn scene_requests_collect_set_scene() {
        let instances = vec![instance("D_Box01", 0)];
        let source = r#"
Scene_OnLoad()
{
    SetScene("snowdensity", 5);
}
"#;
        let mut runtime = load(source, instances);
        runtime.start().expect("start");
        assert_eq!(runtime.host().scene_requests().len(), 1);
        assert_eq!(runtime.host().scene_requests()[0].0, "snowdensity");
        assert_eq!(runtime.host().scene_requests()[0].1, vec![5.0]);
    }

    #[test]
    fn misc_transfer_and_game_mode_are_exposed_to_scene_scripts() {
        let source = r#"
Scene_OnLoad()
{
    Misc("transfer", 0, "D_tpED01");
}
Mode()
{
    return GameCallBack("mod", 0);
}
"#;
        let mut runtime = load(source, vec![instance("D_tpED01", 0)]);
        runtime.start().expect("start");
        assert_eq!(
            runtime.host_mut().take_transfer_requests(),
            vec![SceneTransferRequest {
                character_id: 0,
                destination: "D_tpED01".into(),
            }]
        );
        assert_eq!(runtime.call("Mode", &[]).expect("mode"), PstValue::Num(1.0));
        runtime.host_mut().set_game_mode(5);
        assert_eq!(runtime.call("Mode", &[]).expect("mode"), PstValue::Num(5.0));
    }

    #[test]
    fn character_death_event_keeps_original_id_and_event_code() {
        let source = r#"
Scene_OnCharacterEvent(character, event)
{
    SetSceneItem(0, "last_event", character * 1000 + event);
}
"#;
        let mut runtime = load(source, vec![instance("D_tpEC01", 0)]);
        runtime
            .call(
                "Scene_OnCharacterEvent",
                &[PstValue::Num(7.0), PstValue::Num(202.0)],
            )
            .expect("event runs");
        assert_eq!(
            runtime
                .host()
                .instance(0)
                .expect("instance")
                .get("last_event"),
            PstValue::Num(7202.0)
        );
    }

    #[test]
    fn origin_res_sn01_pst_executes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01/sn01.pst").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }

        let mut parts = Vec::new();
        for include in ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"] {
            let path = root.join("PScriptLib").join(include);
            if let Ok(bytes) = std::fs::read(&path) {
                parts.push(crate::text::decode_cp936(&bytes));
            }
        }
        let stage_bytes = std::fs::read(root.join("sn01/sn01.pst")).expect("sn01.pst");
        parts.push(crate::text::decode_cp936(&stage_bytes));
        let source = parts.join("\n");

        let desc_text = std::fs::read_to_string(root.join("sn01/sn01.des")).expect("sn01.des");
        let desc = meteor_asset::des::parse_stage_description(&desc_text);
        let instances: Vec<SceneItemInstance> = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: HashMap::from([(
                    "active".into(),
                    PstValue::Num(dummy.visible as i32 as f64),
                )]),
            })
            .collect();
        assert!(!instances.is_empty(), "sn01 has scene items");

        let mut runtime =
            SceneScriptRuntime::load(&source, instances, 250_000).expect("parse sn01.pst");
        runtime.start().expect("Scene_OnLoad/OnInit runs");

        runtime
            .call_item(
                "D_BBox01",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(101.0)],
            )
            .expect("real D_BBox01 OnAttack runs");
        let box_instance = runtime
            .host()
            .instances()
            .iter()
            .find(|instance| instance.name.eq_ignore_ascii_case("D_BBox01"))
            .expect("sn01 box");
        assert_eq!(box_instance.get("pose"), PstValue::Num(1.0));
        assert_eq!(box_instance.get("collision"), PstValue::Num(0.0));
        assert_eq!(
            box_instance.get("active"),
            PstValue::Num(1.0),
            "source box remains visible while its break clip is pending"
        );
        let weapon_drop = runtime
            .host()
            .instances()
            .iter()
            .find(|instance| instance.name.eq_ignore_ascii_case("D_wpBBox01"))
            .expect("sn01 paired weapon drop");
        assert_eq!(weapon_drop.get("active"), PstValue::Num(1.0));
        assert_eq!(weapon_drop.get("interactive"), PstValue::Num(1.0));
        assert!(runtime.host_mut().take_effects().iter().any(
            |effect| matches!(effect, SceneNetOp::Effect { effect, .. } if effect == "BoxBRK")
        ));

        for _ in 0..3 {
            runtime.update(0.1).expect("OnIdle tick");
        }
        let box_instance = runtime
            .host()
            .instances()
            .iter()
            .find(|instance| instance.name.eq_ignore_ascii_case("D_BBox01"))
            .expect("sn01 box after OnIdle");
        assert_eq!(
            box_instance.get("active"),
            PstValue::Num(0.0),
            "finished real box break must remove the source prop"
        );

        let host = runtime.host();
        let written = host
            .instances()
            .iter()
            .filter(|instance| !instance.props.is_empty())
            .count();
        assert!(written > 0, "sn01 script writes item attributes");
    }
}

#[cfg(test)]
mod net_op_tests {
    use super::*;

    #[test]
    fn apply_net_op_writes_instance_props() {
        let host = SceneHost::new(vec![SceneItemInstance {
            name: "D_BBox01".into(),
            index: 0,
            ..Default::default()
        }]);

        let program = parse_program(
            r#"OnStart()
{
    NetEvent(1);
    SetSceneItem(0, "visible", 0);
    NetEvent(0);
}"#,
        )
        .expect("parses");
        let mut runtime = Runtime::new(program, host, 50_000);
        runtime.call("OnStart", &[]).expect("OnStart runs");
        let ops = runtime.host_mut().take_net_ops();
        assert_eq!(ops.len(), 1);
        assert!(
            matches!(&ops[0], SceneNetOp::Set { ref_index: 0, field, .. } if field == "visible")
        );

        assert_eq!(
            runtime.host().instance(0).expect("instance").get("visible"),
            PstValue::Num(0.0)
        );

        runtime.host_mut().apply_net_op(&ops[0]);
        assert_eq!(
            runtime.host().instance(0).expect("instance").get("visible"),
            PstValue::Num(0.0)
        );
    }

    #[test]
    fn named_scene_refs_emit_the_resolved_stock_object_index() {
        let host = SceneHost::new(vec![SceneItemInstance {
            name: "D_APdoor02".into(),
            index: 0,
            ..Default::default()
        }]);
        let program = parse_program(
            r#"OnStart()
{
    NetEvent(1);
    SetSceneItem("D_APdoor02", "attribute", "active", 1);
    NetEvent(0);
}"#,
        )
        .expect("parses");
        let mut runtime = Runtime::new(program, host, 50_000);
        runtime.call("OnStart", &[]).expect("OnStart runs");
        assert!(matches!(
            runtime.host_mut().take_net_ops().as_slice(),
            [SceneNetOp::Set {
                ref_index: 0,
                field,
                ..
            }] if field == "attribute"
        ));
    }

    #[test]
    fn consecutive_net_events_keep_their_atomic_batch_boundaries() {
        let host = SceneHost::new(vec![SceneItemInstance {
            name: "D_APdoor".into(),
            index: 0,
            ..Default::default()
        }]);
        let program = parse_program(
            r#"OnStart()
{
    NetEvent(1);
    SetSceneItem(0, "pose", 2, 0);
    NetEvent(0);
    NetEvent(1);
    SetSceneItem(0, "pose", 4, 0);
    NetEvent(0);
}"#,
        )
        .expect("parses");
        let mut runtime = Runtime::new(program, host, 50_000);
        runtime.call("OnStart", &[]).expect("OnStart runs");
        let batches = runtime.host_mut().take_net_batches();
        assert_eq!(batches.len(), 2);
        assert!(matches!(
            batches[0].as_slice(),
            [SceneNetOp::Set { args, .. }] if args.first() == Some(&PstValue::Num(2.0))
        ));
        assert!(matches!(
            batches[1].as_slice(),
            [SceneNetOp::Set { args, .. }] if args.first() == Some(&PstValue::Num(4.0))
        ));
    }

    #[test]
    fn create_effect_is_played_locally_and_emitted_by_net_event() {
        let host = SceneHost::new(vec![SceneItemInstance {
            name: "D_BBox01".into(),
            index: 0,
            ..Default::default()
        }]);
        let program = parse_program(
            r#"OnStart()
{
    NetEvent(1);
    CreateEffect(0, "BoxBRK", 2);
    NetEvent(0);
}"#,
        )
        .expect("parses");
        let mut runtime = SceneScriptRuntime {
            runtime: Runtime::new(program, host, 50_000),
            update_accumulator: 0.0,
        };
        runtime.call("OnStart", &[]).expect("OnStart runs");

        let net_ops = runtime.host_mut().take_net_ops();
        assert!(matches!(
            net_ops.as_slice(),
            [SceneNetOp::Effect { ref_index: 0, effect, extra }]
                if effect == "BoxBRK" && (*extra - 2.0).abs() < f64::EPSILON
        ));
        let local_effects = runtime.host_mut().take_effects();
        assert!(matches!(
            local_effects.as_slice(),
            [SceneNetOp::Effect { ref_index: 0, effect, extra }]
                if effect == "BoxBRK" && (*extra - 2.0).abs() < f64::EPSILON
        ));

        runtime.host_mut().apply_net_op(&SceneNetOp::Effect {
            ref_index: 0,
            effect: "StoneBRK".into(),
            extra: 0.0,
        });
        assert!(matches!(
            runtime.host_mut().take_effects().as_slice(),
            [SceneNetOp::Effect { ref_index: 0, effect, extra }]
                if effect == "StoneBRK" && *extra == 0.0
        ));
    }

    #[test]
    fn get_team_keeps_neutral_and_registered_teams_distinct() {
        let host = SceneHost::new(Vec::new());
        let program = parse_program(
            r#"Team(character)
{
    return GetTeam(character);
}"#,
        )
        .expect("parses");
        let mut runtime = Runtime::new(program, host, 50_000);
        assert_eq!(
            runtime.call("Team", &[PstValue::Num(0.0)]).expect("runs"),
            PstValue::Num(0.0)
        );
        runtime.host_mut().set_character_team(8, 2);
        assert_eq!(
            runtime.call("Team", &[PstValue::Num(8.0)]).expect("runs"),
            PstValue::Num(2.0)
        );
        assert_eq!(
            runtime.call("Team", &[PstValue::Num(9.0)]).expect("runs"),
            PstValue::Num(-1.0)
        );
    }

    #[test]
    fn on_attack_uses_character_team_and_damage_parameters() {
        let host = SceneHost::new(vec![SceneItemInstance {
            name: "D_APdoor".into(),
            index: 0,
            props: HashMap::from([("hp".into(), PstValue::Num(500.0))]),
        }]);
        let program = parse_program(
            r#"D_APdoor_OnAttack(id, character, damage)
{
    if (GetTeam(character) == 1) return 0;
    SetSceneItem(id, "hp", GetSceneItem(id, "hp") - damage);
}"#,
        )
        .expect("parses");
        let mut runtime = SceneScriptRuntime {
            runtime: Runtime::new(program, host, 50_000),
            update_accumulator: 0.0,
        };
        runtime
            .call_item(
                "D_APdoor",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(126.0)],
            )
            .expect("neutral player damages A door");
        assert_eq!(
            runtime.host().instance(0).expect("door").get("hp"),
            PstValue::Num(374.0)
        );

        runtime.host_mut().set_character_team(0, 1);
        runtime
            .call_item(
                "D_APdoor",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(126.0)],
            )
            .expect("friendly player is rejected");
        assert_eq!(
            runtime.host().instance(0).expect("door").get("hp"),
            PstValue::Num(374.0)
        );
    }
}
