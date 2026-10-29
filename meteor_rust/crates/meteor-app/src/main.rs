#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod desktop_args;
#[cfg(target_os = "windows")]
mod desktop_console;
mod original_ui;

fn main() {
    #[cfg(target_os = "windows")]
    let _console = desktop_console::initialize().unwrap_or_else(|error| {
        desktop_console::report_error(&error);
        std::process::exit(1);
    });
    if let Err(error) = run() {
        eprintln!("meteor: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = desktop_args::DesktopArgs::parse(std::env::args_os().skip(1))?;
    if args.help {
        println!("Usage: meteor [--original-dir <directory>] [--check-resources]\n\n  --original-dir     Read an original Meteor installation (read-only; no MPK required)\n  --check-resources  Validate resources without opening a game window\n  --help, -h         Show this help\n\nWithout --original-dir, use METEOR_PACKS_DIR or packs/ beside the executable.");
        return Ok(());
    }
    if let Some(root) = args.original_dir {
        let source =
            meteor_asset::original_directory::OriginalDirectory::open(&root, original_ui::ASSETS)?;
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
    meteor_app::build_app().run();
    Ok(())
}
