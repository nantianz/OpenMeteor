use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("usage: {} <input nvngx_dlssnr.dll> [output.dll]", args[0]);
        eprintln!("  Default output: <input dir>/nvngx_dlssnr.patched.dll");
        return ExitCode::from(2);
    }
    let input = PathBuf::from(&args[1]);
    let output = if args.len() >= 3 {
        PathBuf::from(&args[2])
    } else {
        input.with_file_name("nvngx_dlssnr.patched.dll")
    };

    match bevy_dlss5::patch::patch_file(&input, &output) {
        Ok(report) => {
            for (name, off) in &report.offsets {
                println!("patched '{name}' at 0x{off:X}");
            }
            println!("wrote {}", output.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
