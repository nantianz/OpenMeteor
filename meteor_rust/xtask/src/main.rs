fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: cargo run -p xtask -- <command>");
        std::process::exit(1);
    }
    match args[1].as_str() {
        "build-packs" => {
            eprintln!("build-packs: 尚未实现");
        }
        "verify-packs" => {
            eprintln!("verify-packs: 尚未实现");
        }
        other => {
            eprintln!("未知子命令: {other}");
            std::process::exit(1);
        }
    }
}
