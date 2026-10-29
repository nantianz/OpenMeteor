# OpenMeteor

[简体中文](README.md) | **English**

An open-source Rust and Bevy reimplementation of **Meteor Butterfly Sword (流星蝴蝶剑)**.

OpenMeteor explores the original game's combat, movement, animation, scene logic,
and multiplayer protocols through a modular Rust workspace. It includes a native
desktop client and a dedicated server. The project is under active development;
gameplay coverage and compatibility are incomplete.

## Video introduction

[▶ Watch the project introduction on Bilibili](https://www.bilibili.com/video/BV1HceY6nEbx/)

## Screenshots

These screenshots show the game running with game resources, which must be provided separately.

| Courtyard combat | Snow scene |
| --- | --- |
| ![Courtyard combat](docs/images/courtyard-combat.png) | ![Snow scene](docs/images/snow-scene.png) |
| **Match results** | **Town scene** |
| ![Match results](docs/images/match-results.png) | ![Town scene](docs/images/town-scene.png) |

## What is included

- Modular gameplay systems for combat, physics, input, AI, and scene scripting.
- Asset readers, animation support, and resource-loading infrastructure.
- A Bevy desktop client, multiplayer networking, and a Rust server.
- Rust entry crates for Android and WebAssembly.
- Vendored rendering patches and automated tests that can run without game data.

This is a source distribution. Original game data, executables, audiovisual
assets, deployment settings, signing material, and prebuilt binaries are not
included. Building the software does not require an original game installation.
Playing it requires compatible resources that you are entitled to use.

## Build

Requirements:

- Rust stable, installed with rustup (validated with Rust 1.97.1).
- A native C/C++ linker and the system libraries required by Bevy.
- Sufficient disk space for Bevy's build dependencies.

```sh
git clone https://github.com/nantianz/OpenMeteor.git
cd OpenMeteor/meteor_rust
cargo build --workspace --locked
```

Build the desktop client or server separately:

```sh
cargo build --locked -p meteor-app --bin meteor
cargo build --locked -p meteor-server
```

For an optimized build:

```sh
cargo build --locked -p meteor-app --bin meteor --release
```

## Resources and desktop usage

The desktop client accepts a read-only original installation or compatible MPK
resource packs. Neither is distributed here. The bundled launcher uses a Bevy
fallback font and transparent artwork placeholders; complete launcher visuals
and Chinese glyph coverage require suitable replacement UI assets and fonts.

```sh
cargo run --locked -p meteor-app --bin meteor -- --help
cargo run --locked -p meteor-app --bin meteor -- --original-dir /path/to/game --check-resources
cargo run --locked -p meteor-app --bin meteor -- --original-dir /path/to/game
```

For existing resource packs, set `METEOR_PACKS_DIR` to a directory containing
`packs.json` and its pack files, or place `packs/` beside the executable.

## Multiplayer

The native client defaults to `ws://127.0.0.1:9001`. Run your own server:

```sh
cargo run --locked -p meteor-server
```

The server reads its listening port from `PORT`. Set `METEOR_WS` to use a different
WebSocket endpoint. No hosted game service or TURN credentials are bundled.

Native WebRTC uses public STUN services by default. An optional TURN relay is
configured at runtime with `METEOR_TURN_URLS`, `METEOR_TURN_USERNAME`, and
`METEOR_TURN_PASSWORD`. URLs are comma-separated. Keep credentials outside source
control. Optional build-time route candidates use `METEOR_PRIMARY_WS` and
`METEOR_SECONDARY_WS`.

## Tests

From the repository root:

```sh
cd meteor_rust
cargo test --workspace --locked
```

Tests that require original game files or separate reference captures are gated
behind `original-assets`. Enabling this feature requires providing the matching
resources and fixtures locally; they are excluded from this source distribution.

The publication was validated on macOS with Rust 1.97.1. Android, WebAssembly,
and optional Windows rendering backends have separate platform requirements and
were not validated as part of this release.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `meteor-core` | Shared data and game rules |
| `meteor-asset` | Resource formats and loading |
| `meteor-anim` | Animation data and playback |
| `meteor-combat` | Combat and status effects |
| `meteor-physics` | Movement and collision |
| `meteor-scene` | Scenes and mission scripts |
| `meteor-ai` | AI behavior |
| `meteor-input` | Input mapping |
| `meteor-network` | Protocols and multiplayer state |
| `meteor-app` | Bevy client and desktop executable |
| `meteor-server` | Dedicated server |
| `meteor-android`, `meteor-wasm` | Platform entry crates |

## Contributing

Bug reports and focused pull requests are welcome. Include reproduction steps
and relevant automated checks. Keep credentials, personal configuration, and
original game data out of contributions. Discuss changes to gameplay behavior
with reproducible evidence whenever possible.

## License

OpenMeteor project code is licensed under **GNU GPL version 3 only**. See
[LICENSE](LICENSE). Third-party dependencies retain their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

This is an independent fan project. The original game's data and trademarks
remain the property of their respective rights holders. This repository's
software license does not grant rights to those assets.
