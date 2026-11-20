use std::sync::{Arc, Mutex, Weak};

use crate::mission_runtime::MissionScriptRuntime;
use crate::pst_game::PstGame;
use crate::pst_music::PstMusicRequests;
use crate::pst_runtime::{
    parse_program, script_id, script_name, Execution, LinkedPair, LinkedScript, PstHost, PstValue,
    Runtime,
};
use crate::pst_ui::{PstUi, UiOwner};
use crate::scene_script_runtime::SceneScriptRuntime;

struct StartupHost {
    owner: UiOwner,
    ui: PstUi,
    music: PstMusicRequests,
}

const FIRST_STARTUP_ID: i64 = -10;

pub(crate) struct StartupModule(Mutex<Runtime<StartupHost>>);

impl StartupModule {
    pub(crate) fn invoke(
        &self,
        id: i64,
        name: &str,
        args: &[PstValue],
        execution: &mut Execution<'_>,
    ) -> Result<bool, String> {
        self.0
            .try_lock()
            .map_err(|_| "PST startup module is already executing".to_owned())?
            .invoke_script(id, name, args, execution)
    }

    pub(crate) fn variable(&self, id: i64, name: &str) -> Option<PstValue> {
        self.0.try_lock().ok()?.script_var(id, name)
    }
}

struct RegisteredModule {
    id: i64,
    name: String,
    module: Weak<StartupModule>,
}

#[derive(Clone, Default)]
pub(crate) struct StartupRegistry(Arc<Mutex<Vec<RegisteredModule>>>);

impl StartupRegistry {
    fn register(
        &self,
        id: i64,
        source_name: &str,
        module: &Arc<StartupModule>,
    ) -> Result<(), String> {
        let name = script_name(source_name);
        let mut entries = self
            .0
            .lock()
            .map_err(|_| "PST startup registry unavailable")?;
        entries.retain(|entry| entry.module.strong_count() != 0);
        if entries
            .iter()
            .any(|entry| entry.id == id || entry.name == name)
        {
            return Err(format!("duplicate PST startup module: {source_name}"));
        }
        entries.push(RegisteredModule {
            id,
            name,
            module: Arc::downgrade(module),
        });
        Ok(())
    }

    pub(crate) fn find(&self, selector: &PstValue) -> Option<(i64, Arc<StartupModule>)> {
        let name = match selector {
            PstValue::Str(name) => Some(script_name(name)),
            _ => None,
        };
        let id = script_id(selector);
        self.0.lock().ok()?.iter().find_map(|entry| {
            if id == Some(entry.id) || name.as_deref() == Some(entry.name.as_str()) {
                entry.module.upgrade().map(|module| (entry.id, module))
            } else {
                None
            }
        })
    }
}

pub(crate) fn is_startup_id(id: i64) -> bool {
    id <= FIRST_STARTUP_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pst_game::ScriptEvent;

    #[test]
    fn scene_can_call_multiple_startup_modules_and_reenter_the_original_call_stack() {
        let game = PstGame::default();
        let ui = PstUi::default();
        let music = PstMusicRequests::new(|_| None);
        let mut startup = StartupScripts::default();
        startup
            .add(
                "library.pst",
                r#"
            int score = 0;
            Forward() { score = 1; ExecuteFunction("utility.pst", "Cross"); }
            Add(int amount) { score += amount; }
        "#,
                game.clone(),
                ui.clone(),
                music.clone(),
            )
            .unwrap();
        startup
            .add(
                "utility.pst",
                r#"
            Cross() { ExecuteFunction(GetLevelScriptId(), "UpdateScore"); }
            CheckMenu() {
                ExecuteFunction(GetStartupScriptId("library"), "Add", 2);
                SetSharedVarInt("menuScore", GetScriptVarInt("library", "score"));
            }
        "#,
                game.clone(),
                ui,
                music,
            )
            .unwrap();
        startup
            .dispatch(
                Some(1),
                &[ScriptEvent {
                    name: "CheckMenu".into(),
                    args: vec![],
                }],
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            game.builtin("GetSharedVarInt", &[PstValue::from("menuScore")]),
            Some(PstValue::Num(2.0))
        );
        game.begin_stage(true);
        let mut scene = SceneScriptRuntime::load(
            r#"
            int phase = 3;
            Begin() {
                if (!ExecuteFunction("library.pst", "Forward")) { return -1; }
                return phase * 100 + GetScriptVarInt("library", "score");
            }
            Stamp() { phase += 5; }
        "#,
            vec![],
            2000,
        )
        .unwrap();
        scene.set_game(game.clone());
        let mut mission = MissionScriptRuntime::new(
            r#"
            UpdateScore() {
                ExecuteFunction(GetStartupScriptId("library"), "Add", GetScriptVarInt("scene", "phase"));
                ExecuteFunction(GetSceneScriptId(), "Stamp");
            }
        "#,
            Default::default(),
        );
        mission.set_game(game.clone());
        assert_eq!(
            scene.call_linked("Begin", &[], Some(&mut mission)).unwrap(),
            PstValue::Num(804.0)
        );
        game.begin_stage(false);
        assert_eq!(
            scene.call_linked("Begin", &[], Some(&mut mission)).unwrap(),
            PstValue::Num(-1.0)
        );
        game.begin_stage(true);
        drop(startup);
        assert_eq!(
            scene.call_linked("Begin", &[], Some(&mut mission)).unwrap(),
            PstValue::Num(-1.0)
        );
    }

    #[test]
    fn startup_calls_share_the_original_step_budget() {
        let game = PstGame::default();
        game.begin_stage(true);
        let mut startup = StartupScripts::default();
        startup
            .add(
                "library.pst",
                "Spin() { int i = 0; while (i < 2000) { i += 1; } }",
                game.clone(),
                PstUi::default(),
                PstMusicRequests::new(|_| None),
            )
            .unwrap();
        let mut scene = SceneScriptRuntime::load(
            r#"Begin() { ExecuteFunction("library", "Spin"); }"#,
            vec![],
            100,
        )
        .unwrap();
        scene.set_game(game);
        let error = scene.call("Begin", &[]).unwrap_err();
        assert!(error.contains("step limit 100"), "{error}");
    }

    #[test]
    fn invalid_reload_keeps_hooks_and_successful_reload_keeps_globals() {
        let game = PstGame::default();
        let mut startup = StartupScripts::default();
        startup
            .add(
                "state.pst",
                r#"
            int visits = 0;
            OnStart() { visits += 1; }
            OnStop() { visits += 3; }
            Record() { SetSharedVarInt("visits", visits); }
        "#,
                game.clone(),
                PstUi::default(),
                PstMusicRequests::new(|_| None),
            )
            .unwrap();
        let dispatch = |scripts: &mut StartupScripts, name: &str| {
            scripts
                .dispatch(
                    None,
                    &[ScriptEvent {
                        name: name.into(),
                        args: vec![],
                    }],
                    None,
                    None,
                )
                .unwrap();
        };
        dispatch(&mut startup, "OnStart");
        assert!(startup.reload(0, "OnStart( broken", None, None).is_err());
        dispatch(&mut startup, "Record");
        let result = || game.builtin("GetSharedVarInt", &[PstValue::from("visits")]);
        assert_eq!(result(), Some(PstValue::Num(1.0)));
        assert!(startup
            .reload(
                0,
                r#"
            int visits = 200;
            OnStart() { visits += 10; }
            Record() { SetSharedVarInt("visits", visits); }
        "#,
                None,
                None
            )
            .unwrap());
        dispatch(&mut startup, "Record");
        assert_eq!(result(), Some(PstValue::Num(14.0)));
    }

    #[test]
    fn registered_chat_and_message_handlers_use_pst_function_names() {
        let game = PstGame::default();
        game.begin_stage(true);
        let mut startup = StartupScripts::default();
        startup
            .add(
                "events.pst",
                r#"
            OnStart() {
                AddChatCallback("Chat");
                AddMessageCallback("Message");
            }
            Chat(string text, int team) { SetSharedVarString("chat", text); }
            Message(string text) { SetSharedVarString("message", text); Print("no echo"); }
            Produce() { Print("debug", 7); }
        "#,
                game.clone(),
                PstUi::default(),
                PstMusicRequests::new(|_| None),
            )
            .unwrap();
        startup
            .dispatch(
                None,
                &[
                    ScriptEvent {
                        name: "OnStart".into(),
                        args: vec![],
                    },
                    ScriptEvent {
                        name: "OnChat".into(),
                        args: vec![PstValue::from("hi"), PstValue::Num(0.0)],
                    },
                    ScriptEvent {
                        name: "Produce".into(),
                        args: vec![],
                    },
                ],
                None,
                None,
            )
            .unwrap();
        startup
            .dispatch(None, &game.take_events(), None, None)
            .unwrap();
        assert!(game.take_events().is_empty());
        assert_eq!(
            game.builtin("GetSharedVarString", &[PstValue::from("chat")]),
            Some(PstValue::from("hi"))
        );
        assert_eq!(
            game.builtin("GetSharedVarString", &[PstValue::from("message")]),
            Some(PstValue::from("debug 7"))
        );
    }
}

impl PstHost for StartupHost {
    fn builtin(&mut self, name: &str, args: &[PstValue]) -> PstValue {
        if let Some(value) = self.ui.builtin(self.owner, name, args) {
            return value;
        }
        if name == "PlayMusic" {
            return PstValue::Num(f64::from(u8::from(self.music.play(args))));
        }
        if matches!(name, "Print" | "Output") {
            let words = args
                .iter()
                .map(crate::pst_runtime::value_text)
                .collect::<Vec<_>>();
            log::info!(target: "meteor::pst", "{}", words.join(" "));
            return PstValue::Num(1.0);
        }
        PstValue::Num(0.0)
    }
}

#[derive(Default)]
pub struct StartupScripts {
    modules: Vec<Arc<StartupModule>>,
    sources: Vec<(String, String)>,
}

impl StartupScripts {
    pub fn add(
        &mut self,
        name: &str,
        source: &str,
        game: PstGame,
        ui: PstUi,
        music: PstMusicRequests,
    ) -> Result<(), String> {
        let index = self.modules.len();
        let host = StartupHost {
            owner: UiOwner::Startup(index),
            ui,
            music,
        };
        let mut runtime = Runtime::new(parse_program(source)?, host, 50_000);
        let id = FIRST_STARTUP_ID - index as i64;
        runtime.set_root_script(id, name);
        let registry = game.startup_scripts.clone();
        runtime.set_game(game);
        let module = Arc::new(StartupModule(Mutex::new(runtime)));
        registry.register(id, name, &module)?;
        self.modules.push(module);
        self.sources.push((name.to_owned(), source.to_owned()));
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn event_names(&self) -> std::collections::HashSet<String> {
        let mut events = std::collections::HashSet::new();
        for module in &self.modules {
            let runtime = module.0.lock().expect("PST startup module");
            events.extend(runtime.function_names().cloned());
            for name in ["OnChat", "OnScriptMessage"] {
                if !runtime.event_callbacks(name).is_empty() {
                    events.insert(name.into());
                }
            }
        }
        events
    }

    pub fn update_channels(&mut self, ui: PstUi, music: PstMusicRequests) {
        for module in &self.modules {
            let mut runtime = module.0.lock().expect("PST startup module");
            runtime.host_mut().ui = ui.clone();
            runtime.host_mut().music = music.clone();
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|(name, _)| name.as_str())
    }

    pub fn reload(
        &mut self,
        index: usize,
        source: &str,
        mut scene: Option<&mut SceneScriptRuntime>,
        mut mission: Option<&mut MissionScriptRuntime>,
    ) -> Result<bool, String> {
        let Some((_, last_seen)) = self.sources.get_mut(index) else {
            return Ok(false);
        };
        if *last_seen == source {
            return Ok(false);
        }
        source.clone_into(last_seen);
        let program = parse_program(source)?;
        let event = |name: &str| crate::pst_game::ScriptEvent {
            name: name.into(),
            args: vec![],
        };
        let stop = self.dispatch(
            Some(index),
            &[event("OnStop")],
            scene.as_deref_mut(),
            mission.as_deref_mut(),
        );
        self.modules[index]
            .0
            .lock()
            .expect("PST startup module")
            .replace_program(program);
        let start = self.dispatch(Some(index), &[event("OnStart")], scene, mission);
        stop.and(start).map(|_| true)
    }

    pub fn dispatch(
        &mut self,
        target: Option<usize>,
        events: &[crate::pst_game::ScriptEvent],
        mut scene: Option<&mut SceneScriptRuntime>,
        mut mission: Option<&mut MissionScriptRuntime>,
    ) -> Result<(), String> {
        let scene = scene.as_mut().map(|scene| scene.linked_script());
        let mission = mission.as_mut().map(|mission| mission.linked_script());
        match (scene, mission) {
            (Some(scene), Some(mission)) => {
                self.dispatch_with_peer(target, events, Some(&mut LinkedPair(scene, mission)))
            }
            (scene, mission) => self.dispatch_with_peer(target, events, scene.or(mission)),
        }
    }

    fn dispatch_with_peer(
        &mut self,
        target: Option<usize>,
        events: &[crate::pst_game::ScriptEvent],
        mut peer: Option<&mut dyn LinkedScript>,
    ) -> Result<(), String> {
        let mut outcome = Ok(());
        for (index, module) in self.modules.iter().enumerate() {
            if target.is_some_and(|target| target != index) {
                continue;
            }
            let mut runtime = module.0.lock().expect("PST startup module");
            let mut run = |peer: Option<&mut dyn LinkedScript>| {
                let mut peer = peer;
                let mut result = Ok(());
                for event in events {
                    let suppress = (event.name == "OnScriptMessage")
                        .then(|| runtime.game().cloned())
                        .flatten();
                    if let Some(game) = &suppress {
                        game.suppress_script_messages(true);
                    }
                    for callback in runtime.event_callbacks(&event.name) {
                        let linked = peer
                            .as_mut()
                            .map(|peer| &mut **peer as &mut dyn LinkedScript);
                        let current = runtime
                            .call_with_peer(&callback, &event.args, linked)
                            .map(|_| ());
                        result = result.and(current);
                    }
                    if let Some(game) = &suppress {
                        game.suppress_script_messages(false);
                    }
                }
                result
            };
            let linked = peer
                .as_mut()
                .map(|peer| &mut **peer as &mut dyn LinkedScript);
            outcome = outcome.and(run(linked));
            if target.is_none() {
                let linked = peer
                    .as_mut()
                    .map(|peer| &mut **peer as &mut dyn LinkedScript);
                outcome = outcome.and(runtime.run_due(linked));
            }
        }
        outcome
    }
}
