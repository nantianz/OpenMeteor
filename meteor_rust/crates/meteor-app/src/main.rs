#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod desktop_args;
#[cfg(target_os = "windows")]
mod desktop_console;
mod original_ui;

fn main() {
    if std::env::args_os().any(|arg| arg == "--bake-map-previews") {
        if let Err(error) = run() {
            eprintln!("map previews: {error}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(target_os = "windows")]
    let _console = desktop_console::initialize().unwrap_or_else(|error| {
        desktop_console::report_error(&error);
        std::process::exit(1);
    });
    if let Err(error) = run() {
        eprintln!("meteor: {error}");
        #[cfg(target_os = "windows")]
        if !std::env::args_os().any(|arg| arg == "--check-resources") {
            desktop_console::report_startup_error(&error);
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = desktop_args::DesktopArgs::parse(std::env::args_os().skip(1))?;
    if args.help {
        println!("Usage: meteor [--original-dir <directory>] [--check-resources]\n\n  --original-dir     Read an original Meteor installation (read-only; no MPK required)\n  --check-resources  Validate resources without opening a game window\n  --help, -h         Show this help");
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
    let mut app = meteor_app::build_app();
    meteor_app::install_stage_preview_worker(&mut app, original_dir);
    app.run();
    Ok(())
}
