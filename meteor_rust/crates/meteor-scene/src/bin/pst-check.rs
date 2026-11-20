use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut paths = std::env::args_os().skip(1).peekable();
    if paths.peek().is_none() {
        eprintln!("usage: pst-check FILE.pst [FILE.pst ...]");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for path in paths {
        let path = Path::new(&path);
        let result = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                let text = meteor_scene::text::decode_cp936(&bytes);
                meteor_scene::pst_runtime::parse_program(&text).map(|_| ())
            });
        match result {
            Ok(()) => println!("{} OK", path.display()),
            Err(error) => {
                eprintln!("{}: {error}", path.display());
                failed = true;
            }
        }
    }
    ExitCode::from(u8::from(failed))
}
