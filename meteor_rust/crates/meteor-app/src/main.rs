#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod desktop_args;
#[cfg(target_os = "windows")]
mod desktop_console;
mod original_ui;

fn main() {
    if std::env::args_os().any(|arg| arg == "--bake-map-previews") {
        if let Err(error) = run(None) {
            eprintln!("map previews: {error}");
            std::process::exit(1);
        }
        return;
    }

    let bot_logs = std::env::args_os().any(|arg| arg == "--bot-host").then(|| {
        bot_log_directory().unwrap_or_else(|error| {
            eprintln!("meteor: {error}");
            #[cfg(target_os = "windows")]
            desktop_console::report_startup_error(&error.to_string());
            std::process::exit(1);
        })
    });
    #[cfg(target_os = "windows")]
    let _console = match bot_logs.as_deref() {
        Some(directory) => desktop_console::initialize_bot(directory),
        None => desktop_console::initialize(),
    }
    .unwrap_or_else(|error| {
        desktop_console::report_error(&error);
        std::process::exit(1);
    });
    if let Err(error) = run(bot_logs) {
        eprintln!("meteor: {error}");
        #[cfg(target_os = "windows")]
        if !std::env::args_os().any(|arg| arg == "--check-resources") {
            desktop_console::report_startup_error(&error);
        }
        std::process::exit(1);
    }
}

fn bot_log_directory() -> std::io::Result<std::path::PathBuf> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let logs = std::env::current_exe()?.with_file_name("mrc-bot-logs");
    std::fs::create_dir_all(&logs)?;
    let directory = logs.join(format!("{timestamp}-{}", std::process::id()));
    std::fs::create_dir(&directory)?;
    Ok(directory)
}
fn run(bot_logs: Option<std::path::PathBuf>) -> Result<(), String> {
    let args = desktop_args::DesktopArgs::parse(std::env::args_os().skip(1))?;
    if args.help {
        println!("Usage: meteor [--original-dir <directory>] [--check-resources]\n              [--bot-host --bot-python <python.exe> --bot-script <mrc_duel_host.py>\n               --bot-policy <policy.pt> [--bot-stage sn06] [--bot-room <name>]\n               [--bot-name <name>] [--bot-character 0] [--bot-weapon 5] [--bot-port 5230]]\n\n  --original-dir     Read an original Meteor installation (no MPK required)\n                     Missing map previews may be generated in writable stage folders\n  --check-resources  Validate resources without opening a game window\n  --bot-host         Open an original LAN room and drive the host fighter from the\n                     dagger policy sidecar (requires the three bot paths below)\n  --bot-python       Python interpreter with torch installed\n  --bot-script       tools/mrc_duel_host.py\n  --bot-policy       Trained dagger-opening checkpoint (.pt)\n  --bot-stage        Original stage for the hosted room (default sn06)\n  --bot-room         Room name shown in the original LAN browser\n  --bot-name         Host player name\n  --bot-character    Host model number 0..19 (default p0)\n  --bot-weapon       Must stay 5 (dagger); other weapons are rejected\n  --bot-port         LAN UDP port (default 5230)\n  --help, -h         Show this help");
        println!(
            "\n{}",
            if cfg!(feature = "original-directory-default") {
                "Default: read original assets and fonts/ui.ttf from the current working directory; no MPK required."
            } else {
                "Without --original-dir, use METEOR_PACKS_DIR or packs/ beside the executable."
            }
        );
        return Ok(());
    }
    let original_dir = args.resource_directory();
    if let Some(root) = &original_dir {
        let source =
            meteor_asset::original_directory::OriginalDirectory::open(root, original_ui::ASSETS)?;
        if source
            .read("fonts/ui.ttf")
            .map_err(|error| {
                format!(
                    "Required font {}/fonts/ui.ttf: {error}",
                    source.root().display()
                )
            })?
            .is_empty()
        {
            return Err("Required font fonts/ui.ttf is empty".into());
        }
        if args.check_resources {
            source.verify_all()?;
            println!("Original resources verified: {}", source.root().display());
            return Ok(());
        }
        println!("Original resource directory: {}", source.root().display());
        meteor_asset::runtime_assets::initialize_original(source)?;
    } else if args.check_resources {
        let root = meteor_asset::pack_repository::default_pack_directory()?;
        meteor_asset::pack_repository::PackRepository::open(&root)?.verify_all()?;
        println!("MPK resources verified: {}", root.display());
        return Ok(());
    }
    if args.bake_map_previews {
        return meteor_app::bake_stage_previews();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if args.bot_host {
        let config = meteor_app::plugins::stock_bot::StockBotConfig {
            python: args
                .bot_python
                .clone()
                .ok_or("--bot-host requires --bot-python <python.exe>")?,
            script: args
                .bot_script
                .clone()
                .ok_or("--bot-host requires --bot-script <mrc_duel_host.py>")?,
            policy: args
                .bot_policy
                .clone()
                .ok_or("--bot-host requires --bot-policy <policy.pt>")?,
            stage_id: args.bot_stage.clone(),
            room_name: args.bot_room.clone(),
            player_name: args.bot_name.clone(),
            character_id: args.bot_character,
            weapon: args.bot_weapon,
            port: args.bot_port,
            log_dir: bot_logs.ok_or("missing bot log directory")?,
        };
        config.validate()?;
        meteor_app::build_app()
            .add_plugins(meteor_app::plugins::stock_bot::StockBotPlugin::new(config))
            .run();
        return Ok(());
    }
    let mut app = meteor_app::build_app();
    meteor_app::install_stage_preview_worker(&mut app, original_dir);
    app.run();
    Ok(())
}
