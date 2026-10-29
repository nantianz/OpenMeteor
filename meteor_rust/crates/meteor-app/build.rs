use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=METEOR_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=METEOR_WS");
    let version = env::var("METEOR_BUILD_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=METEOR_BUILD_VERSION={}", version.trim());
    let default = if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        "/ws"
    } else {
        "ws://127.0.0.1:9001"
    };
    let address = env::var("METEOR_WS").unwrap_or_else(|_| default.to_owned());
    let address = address.trim();
    if address != "/ws" {
        let authority = address
            .strip_prefix("wss://")
            .or_else(|| address.strip_prefix("ws://"))
            .expect("METEOR_WS must start with ws:// or wss://");
        assert!(
            !authority.split(['/', '?', '#']).next().unwrap().is_empty()
                && !address.chars().any(char::is_whitespace)
                && !address.contains(['\\', '#']),
            "METEOR_WS must be a WebSocket URL without whitespace, escapes or fragments"
        );
    }
    println!("cargo:rustc-env=METEOR_BUILD_WS={address}");
}
