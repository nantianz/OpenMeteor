use crate::pst_runtime::{parse_program, PstHost, PstValue, Runtime};

#[derive(Debug, Clone, Default)]
pub struct MissionDefinition {
    pub values: std::collections::HashMap<String, PstValue>,

    pub npc_scripts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NpcRequest {
    Add { script: String, id: i64 },

    Remove { id: i64 },
}

#[derive(Debug, Clone, Default)]
pub struct NpcDefinition {
    pub script_name: String,

    pub name: String,

    pub model: i64,

    pub weapon: i64,
    pub weapon2: i64,

    pub team: i64,

    pub hp: i64,

    pub spawn: i64,

    pub view: i64,
    pub think: i64,
    pub attack1: i64,
    pub attack2: i64,
    pub attack3: i64,
    pub guard: i64,
    pub dodge: i64,
    pub jump: i64,
    pub look: i64,
    pub burst: i64,
    pub aim: i64,
    pub get_item: i64,
}

pub fn parse_npc_definition(source: &str, script_name: &str) -> NpcDefinition {
    let values = parse_globals(source);
    let number = |key: &str, fallback: i64| -> i64 {
        match values.get(key) {
            Some(PstValue::Num(n)) if n.is_finite() => *n as i64,
            _ => fallback,
        }
    };
    let name = match values.get("Name") {
        Some(PstValue::Str(s)) => s.clone(),
        Some(PstValue::Num(n)) => format!("{}", n.trunc() as i64),
        _ => script_name.to_string(),
    };
    NpcDefinition {
        script_name: script_name.to_string(),
        name,
        model: number("Model", 0),
        weapon: number("Weapon", 0),
        weapon2: number("Weapon2", 0),
        team: number("Team", 0),
        hp: number("HP", 1000),
        spawn: number("Spawn", 0),
        view: number("View", 400),
        think: number("Think", 50),
        attack1: number("Attack1", 0),
        attack2: number("Attack2", 0),
        attack3: number("Attack3", 0),
        guard: number("Guard", 0),
        dodge: number("Dodge", 0),
        jump: number("Jump", 0),
        look: number("Look", 0),
        burst: number("Burst", 0),
        aim: number("Aim", 0),
        get_item: number("GetItem", 0),
    }
}

#[derive(Default)]
pub struct MissionHost {
    vip_enabled: bool,
    vips: [Option<i64>; 2],
    flag_carrier: Option<i64>,
    ai_updates: Vec<(i64, std::collections::HashMap<String, PstValue>)>,
    waypoint_updates: Vec<(usize, bool)>,
    npc_requests: Vec<NpcRequest>,
    game_over: Option<i64>,
    output: Vec<String>,
    elapsed: f64,
    targets: std::collections::HashMap<i64, (String, f64)>,

    actor_positions: std::collections::HashMap<i64, [f64; 3]>,

    waypoint_positions: std::collections::HashMap<i64, [f64; 3]>,
    rng: u64,

    chars: std::collections::HashMap<String, i64>,

    display_names: std::collections::HashMap<i64, String>,

    fighters: std::collections::HashMap<i64, (f64, f64, f64)>,

    in_scene: std::collections::HashMap<i64, bool>,

    active: std::collections::HashMap<i64, bool>,

    enemies: std::collections::HashMap<i64, i64>,

    next_char_id: i64,

    npc_ids: std::collections::HashMap<String, i64>,

    behavior_requests: Vec<(i64, String, Vec<PstValue>)>,

    perform_requests: Vec<(i64, String, Vec<PstValue>)>,

    stop_perform_requests: Vec<i64>,

    performing: std::collections::HashMap<i64, bool>,

    player_perform_requests: Vec<(String, Vec<PstValue>)>,

    say_requests: Vec<(String, String)>,
}

impl MissionHost {
    pub fn vip(&self, team: u8) -> Option<i64> {
        team.checked_sub(1)
            .and_then(|index| self.vips.get(index as usize))
            .copied()
            .flatten()
    }

    pub fn register_vip_candidate(&mut self, id: i64, team: u8) {
        if self.vip_enabled {
            if let Some(slot) = team
                .checked_sub(1)
                .and_then(|index| self.vips.get_mut(index as usize))
            {
                slot.get_or_insert(id);
            }
        }
    }

    pub fn char_id_for_team(&self, name: &str, team: u8) -> Option<i64> {
        let target = if name.eq_ignore_ascii_case("vip") {
            self.vip(team)
        } else if name.eq_ignore_ascii_case("enemyvip") {
            self.vips
                .iter()
                .enumerate()
                .find_map(|(index, id)| (index + 1 != team as usize).then_some(*id).flatten())
        } else {
            return self.get_char_id(name, false);
        };
        target.filter(|id| self.in_scene.get(id).copied().unwrap_or(false))
    }
    pub fn defer_ai_update(
        &mut self,
        id: i64,
        values: std::collections::HashMap<String, PstValue>,
    ) {
        if self.in_scene.get(&id).copied().unwrap_or(false) {
            self.ai_updates.push((id, values));
        }
    }
    pub fn take_ai_updates(&mut self) -> Vec<(i64, std::collections::HashMap<String, PstValue>)> {
        std::mem::take(&mut self.ai_updates)
    }
    pub fn take_waypoint_updates(&mut self) -> Vec<(usize, bool)> {
        std::mem::take(&mut self.waypoint_updates)
    }

    pub fn sync_flag_carrier(&mut self, carrier: Option<i64>) {
        self.flag_carrier = carrier;
    }

    pub fn take_npc_requests(&mut self) -> Vec<NpcRequest> {
        std::mem::take(&mut self.npc_requests)
    }

    pub fn game_over(&self) -> Option<i64> {
        self.game_over
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }

    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    pub fn register_char(&mut self, name: &str, id: i64) {
        self.chars.insert(name.to_lowercase(), id);
        self.display_names.insert(id, name.to_string());
        self.in_scene.insert(id, true);
        self.active.insert(id, true);
    }

    pub fn begin_actor_position_sync(&mut self) {
        self.actor_positions.clear();
    }

    pub fn sync_actor_position(&mut self, id: i64, position: [f32; 3]) {
        self.actor_positions.insert(
            id,
            [position[0] as f64, position[1] as f64, position[2] as f64],
        );
    }

    pub fn sync_waypoint_position(&mut self, id: i64, position: [f32; 3]) {
        self.waypoint_positions.insert(
            id,
            [position[0] as f64, position[1] as f64, position[2] as f64],
        );
    }

    pub fn sync_fighter(&mut self, id: i64, hp: f64, max_hp: f64, team: f64) {
        self.fighters.insert(id, (hp, max_hp, team));
        self.in_scene.entry(id).or_insert(true);
        self.active.insert(id, hp > 0.0);
        if hp <= 0.0 {
            self.enemies.remove(&id);
        }
    }

    pub fn sync_enemy(&mut self, id: i64, enemy: Option<i64>) {
        if !self.active.get(&id).copied().unwrap_or(false) {
            self.enemies.remove(&id);
        } else if let Some(enemy) = enemy {
            self.enemies.insert(id, enemy);
        } else {
            self.enemies.remove(&id);
        }
    }

    pub fn remove_fighter(&mut self, id: i64) {
        self.in_scene.insert(id, false);
        self.active.insert(id, false);
        self.enemies.remove(&id);
        self.performing.remove(&id);
    }

    pub fn sync_performing(&mut self, id: i64, active: bool) {
        self.performing.insert(id, active);
    }

    pub fn take_behavior_requests(&mut self) -> Vec<(i64, String, Vec<PstValue>)> {
        std::mem::take(&mut self.behavior_requests)
    }

    pub fn take_perform_requests(&mut self) -> Vec<(i64, String, Vec<PstValue>)> {
        std::mem::take(&mut self.perform_requests)
    }

    pub fn take_stop_perform_requests(&mut self) -> Vec<i64> {
        std::mem::take(&mut self.stop_perform_requests)
    }

    pub fn take_player_perform_requests(&mut self) -> Vec<(String, Vec<PstValue>)> {
        std::mem::take(&mut self.player_perform_requests)
    }

    pub fn take_say_requests(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.say_requests)
    }

    pub fn char_id(&self, name: &str) -> Option<i64> {
        self.get_char_id(name, false)
    }

    fn get_char_id(&self, name: &str, include_dead: bool) -> Option<i64> {
        if name.eq_ignore_ascii_case("vip") || name.eq_ignore_ascii_case("enemyvip") {
            return self.char_id_for_team(name, 1);
        }
        if name.eq_ignore_ascii_case("flag") {
            return self.flag_carrier.filter(|id| {
                self.in_scene.get(id).copied().unwrap_or(false)
                    && self.active.get(id).copied().unwrap_or(false)
            });
        }
        if name.eq_ignore_ascii_case("player") {
            return Some(0);
        }
        let id = self.chars.get(&name.to_lowercase()).copied()?;
        if !self.in_scene.get(&id).copied().unwrap_or(false) {
            return None;
        }
        if !include_dead && !self.active.get(&id).copied().unwrap_or(false) {
            return None;
        }
        Some(id)
    }

    fn roster_fighter(&self, id: i64) -> Option<&(f64, f64, f64)> {
        if id == 0 || self.in_scene.get(&id).copied().unwrap_or(false) {
            self.fighters.get(&id)
        } else {
            None
        }
    }

    fn active_npc_fighter(&self, id: i64) -> Option<&(f64, f64, f64)> {
        if id != 0
            && self.in_scene.get(&id).copied().unwrap_or(false)
            && self.active.get(&id).copied().unwrap_or(false)
        {
            self.fighters.get(&id)
        } else {
            None
        }
    }

    pub fn target(&self, slot: i64) -> Option<(&str, f64)> {
        self.targets
            .get(&slot)
            .map(|(kind, value)| (kind.as_str(), *value))
    }

    fn target_position(&self, slot: i64) -> Option<[f64; 3]> {
        let (kind, value) = self.targets.get(&slot)?;
        match kind.as_str() {
            "char" => {
                let id = *value as i64;
                if id != 0 && !self.in_scene.get(&id).copied().unwrap_or(false) {
                    return None;
                }
                self.actor_positions.get(&id).copied()
            }
            "waypoint" => self.waypoint_positions.get(&(*value as i64)).copied(),
            _ => None,
        }
    }

    fn next_rand(&mut self) -> f64 {
        if self.rng == 0 {
            self.rng = 0x9e37_79b9_7f4a_7c15;
        }
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let value = x.wrapping_mul(0x2545f4914f6cdd1d);
        (value >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl PstHost for MissionHost {
    fn builtin_for_actor(&mut self, actor: Option<i64>, name: &str, args: &[PstValue]) -> PstValue {
        if matches!(name, "GetChar" | "GetAnyChar") {
            if let Some(PstValue::Str(target)) = args.first() {
                if target.eq_ignore_ascii_case("vip") || target.eq_ignore_ascii_case("enemyvip") {
                    let team = actor
                        .and_then(|id| self.fighters.get(&id))
                        .map_or(1, |fighter| fighter.2 as u8);
                    return PstValue::Num(self.char_id_for_team(target, team).unwrap_or(-1) as f64);
                }
            }
        }
        self.builtin(name, args)
    }
    fn initialize_script_actor(
        &mut self,
        id: i64,
        globals: &std::collections::HashMap<String, PstValue>,
    ) {
        let name = globals
            .get("Name")
            .and_then(|value| {
                if let PstValue::Str(s) = value {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");
        self.register_char(name, id);
        let hp = globals
            .get("HP")
            .map(PstValue::as_f64)
            .unwrap_or(1000.0)
            .max(1.0);
        let team = globals.get("Team").map(PstValue::as_f64).unwrap_or(2.0);
        self.sync_fighter(id, hp, hp, team);
        self.register_vip_candidate(id, team as u8);
    }

    fn script_actor_active(&self, id: i64) -> bool {
        self.in_scene.get(&id).copied().unwrap_or(false)
    }

    fn update_ai_attributes(
        &mut self,
        id: i64,
        globals: &std::collections::HashMap<String, PstValue>,
    ) -> bool {
        self.ai_updates.push((id, globals.clone()));
        true
    }

    fn builtin(&mut self, name: &str, args: &[PstValue]) -> PstValue {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(PstValue::Num(0.0));
        match name {
            "GameOver" => {
                self.game_over = Some(arg(0).as_f64() as i64);
                PstValue::Num(1.0)
            }

            "AddNPC" => {
                let script = match &arg(0) {
                    PstValue::Str(s) => s.clone(),
                    PstValue::Num(n) => format!("{}", n.trunc() as i64),
                    _ => String::new(),
                };
                if !script.is_empty() {
                    let id = if let Some(id) = self.npc_ids.get(&script).copied() {
                        self.in_scene.insert(id, true);
                        self.active.insert(id, true);
                        if let Some((hp, max_hp, _)) = self.fighters.get_mut(&id) {
                            *hp = *max_hp;
                        }
                        id
                    } else {
                        self.next_char_id += 1;
                        self.npc_ids.insert(script.clone(), self.next_char_id);
                        self.next_char_id
                    };
                    self.npc_requests.push(NpcRequest::Add { script, id });
                    return PstValue::Num(id as f64);
                }
                PstValue::Num(-1.0)
            }
            "RemoveNPC" => {
                let id = arg(0).as_f64() as i64;
                if id != 0 && self.in_scene.get(&id).copied().unwrap_or(false) {
                    self.remove_fighter(id);
                    self.npc_requests.push(NpcRequest::Remove { id });
                    PstValue::Num(1.0)
                } else {
                    PstValue::Num(0.0)
                }
            }

            "GetGameTime" => PstValue::Num(self.elapsed.floor()),

            "GetChar" | "GetAnyChar" => {
                let include_dead = name == "GetAnyChar";
                let actor_name = match &arg(0) {
                    PstValue::Str(s) => s.clone(),
                    PstValue::Num(n) => format!("{}", n.trunc() as i64),
                    _ => String::new(),
                };
                PstValue::Num(self.get_char_id(&actor_name, include_dead).unwrap_or(-1) as f64)
            }
            "GetSelf" => arg(0),
            "GetHP" => {
                let id = arg(0).as_f64() as i64;
                PstValue::Num(self.roster_fighter(id).map(|f| f.0).unwrap_or(-1.0))
            }
            "GetMaxHP" => {
                let id = arg(0).as_f64() as i64;
                PstValue::Num(self.roster_fighter(id).map(|f| f.1).unwrap_or(-1.0))
            }
            "GetTeam" => {
                let id = arg(0).as_f64() as i64;
                PstValue::Num(self.roster_fighter(id).map(|f| f.2).unwrap_or(-1.0))
            }
            "GetEnemy" => {
                let id = arg(0).as_f64() as i64;
                let enemy = self
                    .active_npc_fighter(id)
                    .and_then(|_| self.enemies.get(&id).copied())
                    .unwrap_or(-1);
                PstValue::Num(enemy as f64)
            }

            "GetLeader" => {
                let id = arg(0).as_f64() as i64;
                let leader = self.active_npc_fighter(id).map_or(-1, |source| {
                    let own_team = source.2;
                    self.fighters
                        .iter()
                        .filter(|(other, (hp, _, team))| {
                            **other != 0
                                && *hp > 0.0
                                && *team == own_team
                                && self.in_scene.get(other).copied().unwrap_or(false)
                                && self.active.get(other).copied().unwrap_or(false)
                        })
                        .min_by(|a, b| b.1 .1.total_cmp(&a.1 .1).then_with(|| a.0.cmp(b.0)))
                        .map(|(other, _)| *other)
                        .unwrap_or(-1)
                });
                PstValue::Num(leader as f64)
            }
            "SetTarget" => {
                let kind = match &arg(1) {
                    PstValue::Str(s) => s.to_ascii_lowercase(),
                    _ => String::new(),
                };
                if !kind.is_empty() {
                    self.targets
                        .insert(arg(0).as_f64() as i64, (kind, arg(2).as_f64()));
                }
                PstValue::Num(1.0)
            }
            "IsPerforming" => {
                let id = arg(0).as_f64() as i64;
                let performing = (id == 0 || self.active_npc_fighter(id).is_some())
                    && self.performing.get(&id).copied().unwrap_or(false);
                PstValue::Num(performing as i32 as f64)
            }
            "Rand" => {
                let n = arg(0).as_f64().max(1.0);
                PstValue::Num((self.next_rand() * n).floor())
            }

            "ChangeBehavior" => {
                let id = arg(0).as_f64() as i64;
                let behavior = match &arg(1) {
                    PstValue::Str(s) => s.clone(),
                    _ => String::new(),
                };
                let args = args.iter().skip(2).cloned().collect::<Vec<PstValue>>();
                self.behavior_requests.push((id, behavior, args));
                PstValue::Num(1.0)
            }

            "Perform" | "Perofrm" => {
                let id = arg(0).as_f64() as i64;
                let action = match &arg(1) {
                    PstValue::Str(s) => s.clone(),
                    _ => String::new(),
                };
                self.performing.insert(id, true);
                self.perform_requests
                    .push((id, action, args.iter().skip(2).cloned().collect()));
                PstValue::Num(1.0)
            }
            "StopPerform" => {
                let id = arg(0).as_f64() as i64;
                self.perform_requests.retain(|(target, _, _)| *target != id);
                if id == 0 {
                    self.player_perform_requests.clear();
                }
                self.performing.insert(id, false);
                self.stop_perform_requests.push(id);
                PstValue::Num(1.0)
            }
            "PlayerPerform" => {
                let action_index = matches!(args.first(), Some(PstValue::Num(_))) as usize;
                let action = match args.get(action_index) {
                    Some(PstValue::Str(action)) => action.clone(),
                    _ => String::new(),
                };
                if !action.is_empty() {
                    self.performing.insert(0, true);
                    self.player_perform_requests.push((
                        action,
                        args.iter().skip(action_index + 1).cloned().collect(),
                    ));
                }
                PstValue::Num(1.0)
            }
            "Distance" | "Distanve" => {
                let a = self.target_position(arg(0).as_f64() as i64);
                let b = self.target_position(arg(1).as_f64() as i64);
                let distance = a.zip(b).map(|(a, b)| {
                    let dx = a[0] - b[0];
                    let dy = a[1] - b[1];
                    let dz = a[2] - b[2];
                    (dx * dx + dy * dy + dz * dz).sqrt()
                        / meteor_core::constants::MODEL_SCALE as f64
                });
                PstValue::Num(distance.unwrap_or(999_999.0))
            }
            "EnableWaypoints" | "DisableWaypoints" => {
                let enabled = name == "EnableWaypoints";
                for arg in args {
                    let id = arg.as_f64();
                    if id.is_finite() && id >= 0.0 && id.fract() == 0.0 {
                        self.waypoint_updates.push((id as usize, enabled));
                    }
                }
                PstValue::Num(1.0)
            }

            "Print" | "Output" => {
                let text = args
                    .iter()
                    .map(|v| match v {
                        PstValue::Str(s) => s.clone(),
                        PstValue::Num(n) => format!("{n}"),
                        PstValue::Array(_) => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.output.push(text);
                PstValue::Num(1.0)
            }
            "Say" => {
                let id = arg(0).as_f64() as i64;
                if self.in_scene.get(&id).copied().unwrap_or(false) {
                    if let Some(name) = self.display_names.get(&id).cloned() {
                        let text = args
                            .iter()
                            .skip(1)
                            .map(|value| match value {
                                PstValue::Str(text) => text.clone(),
                                PstValue::Num(value) => format!("{value}"),
                                PstValue::Array(_) => String::new(),
                            })
                            .collect::<String>();
                        self.say_requests.push((name, text));
                    }
                }
                PstValue::Num(1.0)
            }
            _ => PstValue::Num(0.0),
        }
    }
}

pub struct MissionScriptRuntime {
    runtime: Runtime<MissionHost>,
    values: std::collections::HashMap<String, PstValue>,
    elapsed: f64,
    update_accumulator: f64,
}

impl MissionScriptRuntime {
    pub fn new(source: &str, values: std::collections::HashMap<String, PstValue>) -> Self {
        let program = parse_program(source).expect("mission pst parses");

        let mut host = MissionHost::default();
        host.vip_enabled = values
            .get("Rule")
            .is_some_and(|value| value.as_f64() == 4.0);
        host.register_vip_candidate(0, 1);
        for script in add_npc_targets(source) {
            if !host.npc_ids.contains_key(&script) {
                host.next_char_id += 1;
                host.npc_ids.insert(script, host.next_char_id);
            }
        }
        Self {
            runtime: Runtime::new(program, host, 50_000),
            values,
            elapsed: 0.0,
            update_accumulator: 0.0,
        }
    }

    pub fn register_npc_script(&mut self, name: &str, source: &str) -> Result<(), String> {
        let host = self.runtime.host_mut();
        let id = if let Some(id) = host.npc_ids.get(name) {
            *id
        } else {
            host.next_char_id += 1;
            host.npc_ids.insert(name.to_owned(), host.next_char_id);
            host.next_char_id
        };
        self.runtime.register_script(id, source)
    }

    pub fn activate_npc_script(&mut self, id: i64, source: &str) -> Result<(), String> {
        if !self.runtime.script_initialized(id) {
            self.runtime.register_script(id, source)?;
            self.runtime.activate_script(id)?;
        }
        Ok(())
    }

    pub fn call_npc_script(
        &mut self,
        id: i64,
        name: &str,
        args: &[PstValue],
    ) -> Result<bool, String> {
        self.runtime.call_script(id, name, args)
    }

    pub fn values(&self) -> &std::collections::HashMap<String, PstValue> {
        &self.values
    }

    pub fn host(&self) -> &MissionHost {
        self.runtime.host()
    }

    pub fn host_mut(&mut self) -> &mut MissionHost {
        self.runtime.host_mut()
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.runtime.has_function("OnStart") {
            self.runtime.call("OnStart", &[])?;
        }
        Ok(())
    }

    pub fn update(&mut self, dt: f64) -> Result<(), String> {
        self.elapsed += dt;
        self.update_accumulator += dt;

        self.runtime.host_mut().elapsed = self.elapsed;

        if self.update_accumulator < 0.1 - 1e-9 {
            return Ok(());
        }
        self.update_accumulator %= 0.1;
        if self.runtime.has_function("OnUpdate") {
            self.runtime.call("OnUpdate", &[])?;
        }
        for id in self.runtime.script_ids() {
            if self
                .runtime
                .host()
                .active
                .get(&id)
                .copied()
                .unwrap_or(false)
            {
                self.runtime.call_script(id, "OnUpdate", &[])?;
            }
        }
        Ok(())
    }

    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }
}

pub fn parse_mission_definition(source: &str) -> MissionDefinition {
    let values = parse_globals(source);
    let npc_scripts = add_npc_targets(source);
    MissionDefinition {
        values,
        npc_scripts,
    }
}

fn parse_globals(source: &str) -> std::collections::HashMap<String, PstValue> {
    let clean = strip_comments(source);
    let mut out = std::collections::HashMap::new();
    let mut depth = 0usize;
    let mut statement_start = 0usize;
    let bytes = clean.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => {
                let segment = &clean[statement_start..i];
                statement_start = i + 1;
                if let Some(eq) = segment.find('=') {
                    let decl = segment[..eq].trim();
                    let mut parts = decl.split_whitespace();
                    let _type = parts.next();
                    if let Some(name) = parts.next() {
                        let name = name.split('[').next().unwrap_or(name);
                        if name
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        {
                            let value = decode_literal(segment[eq + 1..].trim());
                            out.insert(name.to_string(), value);
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn add_npc_targets(source: &str) -> Vec<String> {
    let clean = strip_comments(source);
    let Some(body) = function_body(&clean, "OnStart") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, args) in find_calls(&body) {
        if !name.eq_ignore_ascii_case("AddNPC") {
            continue;
        }
        if let PstValue::Str(script) = decode_literal(args.trim()) {
            if !script.is_empty() {
                out.push(script);
            }
        }
    }
    out
}

fn function_body(source: &str, name: &str) -> Option<String> {
    let mut search = 0usize;
    while let Some(rel) = source[search..].find(name) {
        let start = search + rel;
        search = start + name.len();

        if start > 0 {
            let prev = source.as_bytes()[start - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                continue;
            }
        }
        let Some(open_rel) = source[search..].find('(') else {
            continue;
        };
        let Some(close_rel) = find_matching_paren(&source[search + open_rel + 1..]) else {
            continue;
        };
        let body_start = search + open_rel + 1 + close_rel + 1;
        let Some(body_rel) = source[body_start..].find('{') else {
            continue;
        };
        let body_start = body_start + body_rel + 1;
        let bytes = source.as_bytes();
        let mut depth = 1usize;
        let mut quote = false;
        let mut i = body_start;
        while i < bytes.len() {
            if bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                quote = !quote;
            } else if !quote {
                if bytes[i] == b'{' {
                    depth += 1;
                } else if bytes[i] == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(source[body_start..i].to_string());
                    }
                }
            }
            i += 1;
        }
        return None;
    }
    None
}

fn find_matching_paren(after_open: &str) -> Option<usize> {
    let bytes = after_open.as_bytes();
    let mut quote = false;
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            quote = !quote;
        } else if !quote && c == b')' {
            return Some(i);
        }
    }
    None
}

fn find_calls(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = body[start..i].to_string();
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                if let Some(close) = find_matching_paren(&body[j + 1..]) {
                    out.push((name, body[j + 1..j + 1 + close].to_string()));
                    i = j + 1 + close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '/' {
            if let Some(&(j, next)) = chars.peek() {
                if next == '*' {
                    chars.next();
                    let rest = &source[j + 1..];
                    if let Some(end) = rest.find("*/") {
                        for _ in 0..end + 1 {
                            chars.next();
                        }
                    } else {
                        break;
                    }
                    continue;
                }
                if next == '/' {
                    chars.next();
                    for (_, c) in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                    out.push('\n');
                    continue;
                }
            }
        }
        out.push(ch);
        let _ = i;
    }
    out
}

fn decode_literal(text: &str) -> PstValue {
    let value = text.trim();
    if value.starts_with('"') && value.ends_with('"') {
        return PstValue::Str(
            value[1..value.len() - 1]
                .replace("\\\"", "\"")
                .replace("\\\\", "\\"),
        );
    }
    if let Ok(n) = value.parse::<f64>() {
        return PstValue::Num(n);
    }
    PstValue::Str(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_vip_follows_spawn_order_and_query_actor_team() {
        let source =
            "int Rule=4; OnStart(){ AddNPC(\"friend\"); AddNPC(\"enemy\"); AddNPC(\"later\"); }";
        let mut runtime =
            MissionScriptRuntime::new(source, parse_mission_definition(source).values);
        runtime.host_mut().sync_fighter(0, 100.0, 100.0, 0.0);
        runtime
            .register_npc_script("friend", "string Name=\"Friend\"; int Team=1;")
            .unwrap();
        runtime.register_npc_script("enemy", "string Name=\"Enemy\"; int Team=2; OnStart(){ Print(GetChar(\"vip\"), GetChar(\"enemyvip\")); }").unwrap();
        runtime
            .register_npc_script("later", "string Name=\"Later\"; int Team=2;")
            .unwrap();
        runtime.start().unwrap();
        assert_eq!(runtime.host().vip(1), Some(0));
        assert_eq!(runtime.host().vip(2), Some(2));
        assert_eq!(runtime.host().char_id_for_team("vip", 1), Some(0));
        assert_eq!(runtime.host().char_id_for_team("vip", 2), Some(2));
        assert_eq!(runtime.host().char_id_for_team("enemyvip", 2), Some(0));
        assert_eq!(runtime.host().output().last().unwrap(), "2 0");
        runtime.host_mut().remove_fighter(2);
        assert_eq!(runtime.host().char_id_for_team("vip", 2), None);
        runtime.host_mut().register_vip_candidate(3, 2);
        assert_eq!(runtime.host().vip(2), Some(2));
    }

    fn sample_mission() -> &'static str {
        r#"// 任务定义
int RoundTime = 120;
string MissionName = "练习";
OnStart()
{
    AddNPC("npc01_01");
    AddNPC("npc01_02");
    RemoveNPC(3);
    GameOver(-1);
}
OnUpdate()
{
    int t = GetGameTime();
    if (t > 100)
    {
        GameOver(1);
    }
}
"#
    }

    #[test]
    fn parse_mission_definition_extracts_globals_and_npc_scripts() {
        let def = parse_mission_definition(sample_mission());
        assert_eq!(def.values.get("RoundTime"), Some(&PstValue::Num(120.0)));
        assert_eq!(
            def.values.get("MissionName"),
            Some(&PstValue::Str("练习".into()))
        );

        assert_eq!(def.npc_scripts, vec!["npc01_01", "npc01_02"]);
    }

    #[test]
    fn mission_runtime_collects_npc_requests_and_game_over() {
        let def = parse_mission_definition(sample_mission());
        let mut runtime = MissionScriptRuntime::new(sample_mission(), def.values.clone());
        runtime.start().expect("OnStart runs");

        let requests = runtime.host_mut().take_npc_requests();
        assert_eq!(
            requests,
            vec![
                NpcRequest::Add {
                    script: "npc01_01".into(),
                    id: 1
                },
                NpcRequest::Add {
                    script: "npc01_02".into(),
                    id: 2
                },
            ]
        );

        assert_eq!(runtime.host().game_over(), Some(-1));
    }

    #[test]
    fn mission_on_update_ticks_on_0_1s_gate() {
        let source = r#"int RoundTime = 120;
OnStart()
{
    AddNPC("npc01_01");
}
OnUpdate()
{
    Print("tick");
}"#;
        let mut runtime = MissionScriptRuntime::new(source, Default::default());
        runtime.start().expect("OnStart runs");
        runtime.host_mut().take_npc_requests();
        assert!(runtime.host().output().is_empty());

        runtime.update(0.05).expect("tick");
        runtime.update(0.04).expect("tick");
        assert!(
            runtime.host().output().is_empty(),
            "sub-gate ticks skip OnUpdate"
        );

        runtime.update(0.01).expect("tick");
        assert_eq!(runtime.host().output(), &["tick"]);
        assert!(runtime.elapsed() > 0.0);
    }

    #[test]
    fn get_game_time_uses_live_runtime_clock_and_unblocks_delayed_branches() {
        let source = r#"int timer = 0;
int fired = 0;
OnStart()
{
    timer = GetGameTime() + 2;
}
OnUpdate()
{
    if (fired == 0 && GetGameTime() > timer)
    {
        AddNPC("late_guard");
        fired = 1;
    }
}"#;
        let mut runtime = MissionScriptRuntime::new(source, Default::default());
        runtime.start().expect("OnStart runs");
        assert!(runtime.host_mut().take_npc_requests().is_empty());

        runtime.update(2.9).expect("pre-threshold tick");
        assert_eq!(runtime.elapsed(), 2.9);
        assert_eq!(runtime.host().elapsed(), 2.9);
        assert!(runtime.host_mut().take_npc_requests().is_empty());

        runtime.update(0.2).expect("post-threshold tick");
        assert_eq!(runtime.host().elapsed().floor(), 3.0);
        assert_eq!(
            runtime.host_mut().take_npc_requests(),
            vec![NpcRequest::Add {
                script: "late_guard".into(),
                id: 1,
            }]
        );
    }

    #[test]
    fn rand_leaves_zero_seed_absorbing_state_and_stays_in_range() {
        let mut host = MissionHost::default();
        let values = (0..8)
            .map(|_| host.builtin("Rand", &[PstValue::Num(100.0)]).as_f64())
            .collect::<Vec<_>>();
        assert!(values.iter().all(|value| (0.0..100.0).contains(value)));
        assert!(
            values.iter().any(|value| *value != 0.0),
            "Rand must not remain trapped at zero"
        );
        assert!(values.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn mission_game_over_positive_is_victory_signal() {
        let source = r#"OnStart() { GameOver(1); }"#;
        let mut runtime = MissionScriptRuntime::new(source, Default::default());
        runtime.start().expect("OnStart runs");
        assert_eq!(runtime.host().game_over(), Some(1));
    }

    #[test]
    fn mission_globals_skip_function_local_declarations() {
        let source = r#"int RoundTime = 90;
OnStart()
{
    int local = 5;
    AddNPC("npc01_01");
}"#;
        let def = parse_mission_definition(source);
        assert_eq!(def.values.len(), 1);
        assert_eq!(def.values.get("RoundTime"), Some(&PstValue::Num(90.0)));
    }

    #[test]
    fn mission_get_char_player_returns_zero() {
        let source = r#"OnStart()
{
    int p = GetChar("player");
    int u = GetChar("npc01_01");
    if (p == 0 && u == -1)
    {
        Print("ok");
    }
}"#;
        let mut runtime = MissionScriptRuntime::new(source, Default::default());
        runtime.start().expect("OnStart runs");
        assert_eq!(runtime.host().output(), &["ok"]);
    }
}

#[test]
fn npc_definition_parses_attributes() {
    let source = r#"string Name = "铁枪哨兵甲";
int Model = 8;
int Weapon = 7;
int Team = 2;
int HP = 1000;
int Spawn = 53;
OnStart()
{
    ChangeBehavior(GetSelf(0), "wait");
}"#;
    let def = parse_npc_definition(source, "npc01_01");
    assert_eq!(def.script_name, "npc01_01");
    assert_eq!(def.name, "铁枪哨兵甲");
    assert_eq!(def.model, 8);
    assert_eq!(def.weapon, 7);
    assert_eq!(def.team, 2);
    assert_eq!(def.hp, 1000);
    assert_eq!(def.spawn, 53);

    assert_eq!(def.weapon2, 0);
    assert_eq!(def.attack1, 0);
}

#[test]
fn get_char_and_fighter_queries_use_registered_tables() {
    let source = r#"OnStart()
{
    int c = GetChar("铁枪哨兵甲");
    int p = GetChar("player");
    int hp = GetHP(c);
    int team = GetTeam(c);
    if (c == 1 && p == 0 && hp == 500 && team == 2)
    {
        Print("ok");
    }
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());

    runtime.host_mut().register_char("铁枪哨兵甲", 1);
    runtime.host_mut().sync_fighter(1, 500.0, 1000.0, 2.0);
    runtime.start().expect("OnStart runs");
    assert_eq!(runtime.host().output(), &["ok"]);

    let missing = r#"OnStart()
{
    int c = GetChar("不存在的人");
    if (c == -1) { Print("miss"); }
}"#;
    let mut runtime2 = MissionScriptRuntime::new(missing, Default::default());
    runtime2.start().expect("OnStart runs");
    assert_eq!(runtime2.host().output(), &["miss"]);
}

#[test]
fn change_behavior_collects_requests() {
    let source = r#"OnStart()
{
    int c = GetChar("守卫");
    ChangeBehavior(c, "wait");
    ChangeBehavior(c, "follow", 5);
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().register_char("守卫", 1);
    runtime.start().expect("OnStart runs");
    let requests = runtime.host_mut().take_behavior_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], (1, "wait".into(), vec![]));
    assert_eq!(requests[1], (1, "follow".into(), vec![PstValue::Num(5.0)]));

    assert!(runtime.host_mut().take_behavior_requests().is_empty());
}

#[test]
fn perform_collects_action_requests() {
    let source = r#"OnStart()
{
    int c = GetChar("守卫");
    StopPerform(c);
    Perform(c, "say", "站住！");
    Perform(c, "attack");
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().register_char("守卫", 1);
    runtime.start().expect("OnStart runs");
    let requests = runtime.host_mut().take_perform_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, 1);
    assert_eq!(requests[0].1, "say");

    assert_eq!(requests[0].2, vec![PstValue::Str("站住！".into())]);
    assert_eq!(requests[1], (1, "attack".into(), vec![]));
    assert_eq!(runtime.host_mut().take_stop_perform_requests(), vec![1]);
}

#[test]
fn is_performing_reads_live_host_projection() {
    let source = r#"OnStart()
{
    if (IsPerforming(7)) { Print("busy"); }
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().register_char("守卫", 7);
    runtime.host_mut().sync_fighter(7, 100.0, 100.0, 2.0);
    runtime.host_mut().sync_performing(7, true);
    runtime.start().expect("OnStart runs");
    assert_eq!(runtime.host().output(), &["busy"]);
}

#[test]
fn player_perform_uses_original_action_first_signature_and_keeps_legacy_prefix() {
    let source = r#"OnStart()
{
    PlayerPerform("block", 0);
    PlayerPerform("say", "我来了");
    PlayerPerform(9, "pause", 3);
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.start().expect("OnStart runs");
    assert_eq!(
        runtime.host_mut().take_player_perform_requests(),
        vec![
            ("block".into(), vec![PstValue::Num(0.0)]),
            ("say".into(), vec![PstValue::Str("我来了".into())]),
            ("pause".into(), vec![PstValue::Num(3.0)]),
        ]
    );
}

#[test]
fn get_enemy_and_leader_query_fighter_table() {
    let source = r#"OnStart()
{
    int e = GetEnemy(1);
    int l = GetLeader(1);
    if (e == 2 && l == 3) { Print("ok"); }
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());

    runtime.host_mut().sync_fighter(1, 500.0, 1000.0, 2.0);
    runtime.host_mut().sync_fighter(2, 300.0, 800.0, 1.0);
    runtime.host_mut().sync_fighter(3, 900.0, 1500.0, 2.0);
    runtime.host_mut().sync_enemy(1, Some(2));
    runtime.start().expect("OnStart runs");
    assert_eq!(runtime.host().output(), &["ok"]);

    let empty = r#"OnStart()
{
    int e = GetEnemy(1);
    int l = GetLeader(9);
    if (e == -1 && l == -1) { Print("none"); }
}"#;
    let mut runtime2 = MissionScriptRuntime::new(empty, Default::default());
    runtime2.host_mut().sync_fighter(1, 100.0, 100.0, 2.0);
    runtime2.start().expect("OnStart runs");
    assert_eq!(runtime2.host().output(), &["none"]);
}

#[test]
fn get_char_distinguishes_dead_from_removed_roster() {
    let source = r#"OnStart()
{
    int alive = GetChar("守卫");
    int any = GetAnyChar("守卫");
    Print(alive);
    Print(any);
}"#;
    let mut alive = MissionScriptRuntime::new(source, Default::default());
    alive.host_mut().register_char("守卫", 7);
    alive.host_mut().sync_fighter(7, 100.0, 100.0, 2.0);
    alive.start().expect("alive query");
    assert_eq!(alive.host().output(), &["7", "7"]);

    let mut dead = MissionScriptRuntime::new(source, Default::default());
    dead.host_mut().register_char("守卫", 7);
    dead.host_mut().sync_fighter(7, 0.0, 100.0, 2.0);
    dead.start().expect("dead query");
    assert_eq!(dead.host().output(), &["-1", "7"]);

    let mut removed = MissionScriptRuntime::new(source, Default::default());
    removed.host_mut().register_char("守卫", 7);
    removed.host_mut().sync_fighter(7, 100.0, 100.0, 2.0);
    removed.host_mut().remove_fighter(7);
    removed.start().expect("removed query");
    assert_eq!(removed.host().output(), &["-1", "-1"]);
}

#[test]
fn removed_actor_data_is_immediately_hidden_and_duplicate_remove_is_rejected() {
    let mut host = MissionHost::default();
    host.register_char("守卫", 7);
    host.sync_fighter(7, 500.0, 1000.0, 2.0);
    host.sync_enemy(7, Some(0));
    host.sync_performing(7, true);

    assert_eq!(
        host.builtin("RemoveNPC", &[PstValue::Num(7.0)]),
        PstValue::Num(1.0)
    );
    for builtin in ["GetHP", "GetMaxHP", "GetTeam", "GetEnemy", "GetLeader"] {
        assert_eq!(
            host.builtin(builtin, &[PstValue::Num(7.0)]),
            PstValue::Num(-1.0),
            "{builtin} must not expose removed actor state"
        );
    }
    assert_eq!(
        host.builtin("IsPerforming", &[PstValue::Num(7.0)]),
        PstValue::Num(0.0)
    );
    assert_eq!(
        host.builtin("RemoveNPC", &[PstValue::Num(7.0)]),
        PstValue::Num(0.0)
    );
    assert_eq!(host.take_npc_requests(), vec![NpcRequest::Remove { id: 7 }]);
}

#[test]
fn get_enemy_uses_live_ai_projection_not_team_guess() {
    let source = r#"OnStart()
{
    Print(GetEnemy(1));
    Print(GetEnemy(2));
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().sync_fighter(1, 500.0, 1000.0, 2.0);
    runtime.host_mut().sync_fighter(2, 500.0, 1000.0, 2.0);
    runtime.host_mut().sync_fighter(3, 500.0, 1000.0, 1.0);
    runtime.host_mut().sync_enemy(1, Some(3));
    runtime.start().expect("enemy query");
    assert_eq!(runtime.host().output(), &["3", "-1"]);
}

#[test]
fn get_leader_includes_source_and_excludes_dead_roster_members() {
    let source = r#"OnStart()
{
    Print(GetLeader(1));
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().sync_fighter(1, 500.0, 1000.0, 2.0);
    runtime.host_mut().sync_fighter(2, 0.0, 2000.0, 2.0);
    runtime.start().expect("leader query");
    assert_eq!(runtime.host().output(), &["1"]);
}

#[test]
fn distance_and_typo_alias_resolve_live_char_and_waypoint_positions() {
    let source = r#"OnStart()
{
    SetTarget(0, "CHAR", 0);
    SetTarget(1, "char", 7);
    SetTarget(2, "waypoint", 3);
    SetTarget(3, "char", 99);
    Print(Distance(0, 1));
    Print(Distanve(1, 2));
    Print(Distance(0, 3));
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().register_char("守卫", 7);
    runtime.host_mut().sync_actor_position(0, [0.0, 0.0, 0.0]);
    runtime.host_mut().sync_actor_position(7, [1.2, 0.0, 0.0]);
    runtime
        .host_mut()
        .sync_waypoint_position(3, [1.2, 0.0, 1.2]);
    runtime.start().expect("distance script");
    let output = runtime.host().output();
    assert!((output[0].parse::<f64>().unwrap() - 10.0).abs() < 1e-5);
    assert!((output[1].parse::<f64>().unwrap() - 10.0).abs() < 1e-5);
    assert_eq!(output[2], "999999");
}

#[test]
fn say_accepts_dead_roster_member_and_ignores_removed_actor() {
    let source = r#"OnStart()
{
    Say(GetAnyChar("队长"), "遗", "言", 2);
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.host_mut().register_char("队长", 7);
    runtime.host_mut().sync_fighter(7, 0.0, 1000.0, 2.0);
    runtime.start().expect("dead captain can say line");
    assert_eq!(
        runtime.host_mut().take_say_requests(),
        vec![("队长".into(), "遗言2".into())]
    );

    let mut removed = MissionScriptRuntime::new(source, Default::default());
    removed.host_mut().register_char("队长", 7);
    removed.host_mut().remove_fighter(7);
    removed.start().expect("removed captain script still runs");
    assert!(removed.host_mut().take_say_requests().is_empty());
}

#[test]
fn add_npc_preassigns_source_order_and_reuses_id_after_remove() {
    let source = r#"OnStart()
{
    AddNPC("npc_a");
}
SpawnLater()
{
    AddNPC("npc_b");
}"#;
    let mut runtime = MissionScriptRuntime::new(source, Default::default());
    runtime.start().expect("initial activation");
    assert_eq!(
        runtime.host_mut().take_npc_requests(),
        vec![NpcRequest::Add {
            script: "npc_a".into(),
            id: 1,
        }]
    );

    let host = runtime.host_mut();
    host.register_char("甲", 1);
    host.sync_fighter(1, 0.0, 800.0, 2.0);
    host.remove_fighter(1);
    assert_eq!(
        host.builtin("AddNPC", &[PstValue::Str("npc_a".into())]),
        PstValue::Num(1.0)
    );
    assert_eq!(host.char_id("甲"), Some(1));
    assert_eq!(
        host.builtin("GetHP", &[PstValue::Num(1.0)]),
        PstValue::Num(800.0)
    );
    assert_eq!(
        host.take_npc_requests(),
        vec![NpcRequest::Add {
            script: "npc_a".into(),
            id: 1,
        }]
    );

    assert_eq!(
        host.builtin("AddNPC", &[PstValue::Str("npc_b".into())]),
        PstValue::Num(2.0)
    );
}

#[cfg(feature = "original-assets")]
#[test]
fn origin_res_sn01_mission_parses_and_runs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");

    let npc_text = std::fs::read(root.join("Level/npc01_01.pst"))
        .map(|b| crate::text::decode_cp936(&b))
        .expect("npc01_01.pst");
    let npc_def = parse_npc_definition(&npc_text, "npc01_01");
    assert_eq!(npc_def.name, "铁枪哨兵﹒甲");
    assert_eq!(npc_def.model, 8);
    assert_eq!(npc_def.weapon, 7);
    assert_eq!(npc_def.team, 2);
    assert_eq!(npc_def.hp, 1000);
    assert_eq!(npc_def.spawn, 53);

    let path = root.join("Level/sn01_.pst");
    if !path.exists() {
        eprintln!("origin_res missing, skipping");
        return;
    }
    let bytes = std::fs::read(path).expect("sn01_.pst");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let definition = parse_mission_definition(&text);

    assert_eq!(
        definition.values.get("RoundTime"),
        Some(&PstValue::Num(10.0)),
        "sn01_ keeps RoundTime"
    );
    assert_eq!(
        definition.values.get("PlayerSpawn"),
        Some(&PstValue::Num(9.0))
    );

    assert_eq!(
        definition.npc_scripts,
        vec!["npc01_01", "npc01_02", "npc01_03"]
    );

    let mut runtime = MissionScriptRuntime::new(&text, definition.values);
    runtime.start().expect("OnStart runs");
    let requests = runtime.host_mut().take_npc_requests();
    assert_eq!(
        requests,
        vec![
            NpcRequest::Add {
                script: "npc01_01".into(),
                id: 1
            },
            NpcRequest::Add {
                script: "npc01_02".into(),
                id: 2
            },
            NpcRequest::Add {
                script: "npc01_03".into(),
                id: 3
            },
        ]
    );
    assert_eq!(runtime.host().game_over(), None);

    runtime.update(0.1).expect("tick");
    assert_eq!(runtime.host().game_over(), None);
}

#[cfg(feature = "original-assets")]
#[test]
fn origin_res_sn09_death_loses_and_surviving_dialogue_wins() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let source = crate::text::decode_cp936(&std::fs::read(root.join("sn09_.pst")).unwrap());
    for dead in [None, Some(0), Some(1)] {
        let definition = parse_mission_definition(&source);
        let mut runtime = MissionScriptRuntime::new(&source, definition.values);
        runtime.start().unwrap();
        runtime.host_mut().register_char("冷燕", 1);
        runtime.host_mut().register_char("屠城", 2);
        for id in 0..=2 {
            runtime
                .host_mut()
                .sync_fighter(id, 3000.0, 3000.0, if id == 0 { 1.0 } else { 2.0 });
        }
        runtime.update(0.1).unwrap();
        if let Some(id) = dead {
            runtime.host_mut().sync_fighter(id, 0.0, 3000.0, 2.0);
            runtime.update(0.1).unwrap();
            assert_eq!(runtime.host().game_over(), None);
            runtime.update(3.1).unwrap();
            assert_eq!(runtime.host().game_over(), Some(-1), "dead actor {id}");
        } else {
            for _ in 0..1970 {
                runtime.update(0.1).unwrap();
            }
            assert_eq!(runtime.host().game_over(), None);
            for _ in 0..30 {
                runtime.update(0.1).unwrap();
            }
            assert_eq!(runtime.host().game_over(), Some(1));
        }
    }
}

#[cfg(feature = "original-assets")]
#[test]
fn origin_res_all_official_missions_parse_start_and_tick() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let mut stages = Vec::new();
    for number in 1..=99 {
        let stage = format!("sn{number:02}");
        let path = root.join(format!("{stage}_.pst"));
        if !path.is_file() {
            continue;
        }
        stages.push(stage.clone());
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{stage}: {error}"));
        let source = crate::text::decode_cp936(&bytes);
        let definition = parse_mission_definition(&source);
        let mut runtime = MissionScriptRuntime::new(&source, definition.values);
        runtime
            .start()
            .unwrap_or_else(|error| panic!("{stage} OnStart: {error}"));
        runtime
            .update(0.1)
            .unwrap_or_else(|error| panic!("{stage} OnUpdate: {error}"));
    }
    assert_eq!(stages.len(), 20, "official mission fixture count changed");
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod pst_completion_tests {
    use super::*;

    fn runtime(source: &str) -> MissionScriptRuntime {
        MissionScriptRuntime::new(source, parse_mission_definition(source).values)
    }

    #[test]
    fn pst_stop_perform_discards_only_commands_before_the_stop() {
        let mut rt = runtime(
            r#"OnStart(){ int id=AddNPC("a");
            Perform(id,"say","old"); Print(IsPerforming(id)); StopPerform(id);
            Print(IsPerforming(id)); Perform(id,"say","new"); Print(IsPerforming(id)); }"#,
        );
        rt.register_npc_script("a", "string Name=\"A\";").unwrap();
        rt.start().unwrap();
        assert_eq!(rt.host().output(), &["1", "0", "1"]);
        let commands = rt.host_mut().take_perform_requests();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].2, vec![PstValue::Str("new".into())]);
    }

    #[test]
    fn pst_call_uses_independent_npc_globals_and_refreshes_live_ai() {
        let source = r#"
            OnStart() {
                int a = AddNPC("a"); int b = AddNPC("b");
                Print(GetChar("Alpha"), GetHP(a));
                Print(Call(a, "Attack", 0), Call(b, "Report"));
                Print(Call(a, "Missing"));
            }
        "#;
        let mut rt = runtime(source);
        rt.register_npc_script(
            "a",
            r#"
            string Name="Alpha"; int HP=4000; int Team=2;
            int Attack1=40; int Attack2=30; int Attack3=10; int self;
            OnInit(int id) { self=GetSelf(id); }
            OnStart() { ChangeBehavior(self,"follow","player"); Perform(self,"say","ready"); }
            Attack(int enable) { Attack1=enable; UpdateAIAttrib(self); return 99; }
        "#,
        )
        .unwrap();
        rt.register_npc_script(
            "b",
            r#"
            string Name="Beta"; int Attack1=77; int self;
            OnInit(int id) { self=GetSelf(id); }
            Report() { UpdateAIAttrib(self); return 42; }
        "#,
        )
        .unwrap();
        rt.start().unwrap();
        assert_eq!(rt.host().output(), &["1 4000", "1 1", "0"]);
        assert_eq!(rt.host_mut().take_behavior_requests()[0].0, 1);
        assert_eq!(rt.host_mut().take_perform_requests()[0].0, 1);
        let updates = rt.host_mut().take_ai_updates();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].0, 1);
        assert_eq!(updates[0].1["Attack1"], PstValue::Num(0.0));
        assert_eq!(updates[1].0, 2);
        assert_eq!(updates[1].1["Attack1"], PstValue::Num(77.0));
    }

    #[test]
    fn pst_npc_reactivation_resets_globals_and_runs_complete_startup() {
        let mut rt = runtime(
            r#"OnStart(){ AddNPC("a"); }
            OnUpdate(){ RemoveNPC(1); AddNPC("a"); }"#,
        );
        rt.register_npc_script(
            "a",
            r#"int n=0; string Name="A";
            OnInit(int id){ n=id; }
            OnStart(){ n=n+1; Print(n); }
            Mutate(){ n=99; }
        "#,
        )
        .unwrap();
        rt.start().unwrap();
        rt.call_npc_script(1, "Mutate", &[]).unwrap();
        rt.update(0.1).unwrap();
        assert_eq!(rt.host().output(), &["2", "2"]);
        rt.host_mut().builtin("RemoveNPC", &[PstValue::Num(1.0)]);
        assert!(!rt.call_npc_script(1, "Mutate", &[]).unwrap());
    }

    #[test]
    fn pst_nested_call_restores_scopes_and_errors_do_not_poison_runtime() {
        let mut rt = runtime(
            r#"OnStart(){ AddNPC("a"); AddNPC("b"); }
            OnUpdate(){ Print(Call(1,"A")); }"#,
        );
        rt.register_npc_script(
            "a",
            r#"int n=1;
            A(){ Call(2,"B"); Print(n); }
            Set(){ n=7; }
            Recurse(){ Call(2,"Recurse"); }
        "#,
        )
        .unwrap();
        rt.register_npc_script(
            "b",
            r#"int n=9;
            B(){ Call(1,"Set"); Print(n); }
            Recurse(){ Call(1,"Recurse"); }
        "#,
        )
        .unwrap();
        rt.start().unwrap();
        rt.update(0.1).unwrap();
        assert_eq!(rt.host().output(), &["9", "7", "1"]);
        let error = rt.call_npc_script(1, "Recurse", &[]).unwrap_err();
        assert!(error.contains("depth limit"), "{error}");
        assert!(rt.call_npc_script(1, "Set", &[]).unwrap());
        rt.update(0.1).unwrap();
        assert_eq!(&rt.host().output()[3..], &["9", "7", "1"]);
    }

    #[test]
    fn pst_original_npc05_attack_switch_reads_modified_attributes() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
        let mut rt = runtime(r#"OnStart(){ AddNPC("npc05_03"); }"#);
        let source = crate::text::decode_cp936(&std::fs::read(root.join("npc05_03.pst")).unwrap());
        rt.register_npc_script("npc05_03", &source).unwrap();
        rt.start().unwrap();
        assert!(rt
            .call_npc_script(1, "Attack", &[PstValue::Num(0.0)])
            .unwrap());
        let disabled = rt.host_mut().take_ai_updates().pop().unwrap();
        assert_eq!(disabled.0, 1);
        for name in ["Attack1", "Attack2", "Attack3"] {
            assert_eq!(disabled.1[name], PstValue::Num(0.0));
        }
        rt.call_npc_script(1, "Attack", &[PstValue::Num(1.0)])
            .unwrap();
        let enabled = rt.host_mut().take_ai_updates().pop().unwrap();
        assert_eq!(enabled.1["Attack1"], PstValue::Num(40.0));
        assert_eq!(enabled.1["Attack2"], PstValue::Num(30.0));
        assert_eq!(enabled.1["Attack3"], PstValue::Num(10.0));
    }

    #[test]
    fn pst_waypoint_toggles_keep_authored_order_and_ignore_invalid_ids() {
        let mut rt = runtime("OnStart(){ DisableWaypoints(2,4,-1); EnableWaypoints(2); }");
        rt.start().unwrap();
        assert_eq!(
            rt.host_mut().take_waypoint_updates(),
            vec![(2, false), (4, false), (2, true)]
        );
    }
}
