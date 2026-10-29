# OpenMeteor

**简体中文** | [English](README.en.md)

基于 **Rust 和 Bevy** 的《流星蝴蝶剑》开源重实现。

OpenMeteor 通过模块化的 Rust 工程探索原版游戏的战斗、移动、动画、场景逻辑和多人联机协议，包含原生桌面客户端与独立服务器。项目仍在开发中，玩法还原和兼容性尚未完整覆盖。

## 视频介绍

[▶ 在 Bilibili 观看项目介绍视频](https://www.bilibili.com/video/BV1HceY6nEbx/)

## 游戏截图

截图展示搭配游戏资源运行时的画面；游戏资源需自行准备。

| 庭院战斗 | 雪地场景 |
| --- | --- |
| ![庭院战斗](docs/images/courtyard-combat.png) | ![雪地场景](docs/images/snow-scene.png) |
| **对局战绩** | **城镇场景** |
| ![对局战绩](docs/images/match-results.png) | ![城镇场景](docs/images/town-scene.png) |

## 项目内容

- 战斗、物理、输入、AI 和场景脚本等模块化玩法系统。
- 资源格式解析、动画支持与资源加载基础设施。
- Bevy 桌面客户端、多人联机功能与 Rust 服务器。
- Android 和 WebAssembly 的 Rust 入口 crate。
- 随源码维护的渲染补丁，以及无需游戏数据即可运行的自动化测试。

本仓库发布源代码，不包含原版游戏数据、可执行文件、音视频素材、部署设置、签名材料或预编译程序。编译软件不需要安装原版游戏；运行游戏需要自行准备有权使用的兼容资源。

## 编译

环境要求：

- 通过 rustup 安装的 Rust stable，已使用 Rust 1.97.1 验证。
- 本机 C/C++ 链接器，以及 Bevy 所需的系统库。
- 足够存放 Bevy 构建依赖和产物的磁盘空间。

```sh
git clone https://github.com/nantianz/OpenMeteor.git
cd OpenMeteor/meteor_rust
cargo build --workspace --locked
```

也可以分别编译桌面客户端和服务器：

```sh
cargo build --locked -p meteor-app --bin meteor
cargo build --locked -p meteor-server
```

构建经过优化的发布版本：

```sh
cargo build --locked -p meteor-app --bin meteor --release
```

## 资源与桌面运行

桌面客户端支持只读加载原版游戏安装目录，或加载兼容的 MPK 资源包；这两类资源均未随仓库分发。启动界面使用 Bevy 后备字体和透明图片占位内容，完整的界面效果与中文字符显示需要适用的 UI 素材和字体。

```sh
cargo run --locked -p meteor-app --bin meteor -- --help
cargo run --locked -p meteor-app --bin meteor -- --original-dir /path/to/game --check-resources
cargo run --locked -p meteor-app --bin meteor -- --original-dir /path/to/game
```

使用已有资源包时，将 `METEOR_PACKS_DIR` 设置为包含 `packs.json` 及其资源包文件的目录，或将 `packs/` 目录放在可执行文件旁。

## 多人联机

原生客户端默认连接 `ws://127.0.0.1:9001`。启动自己的服务器：

```sh
cargo run --locked -p meteor-server
```

服务器从 `PORT` 环境变量读取监听端口。设置 `METEOR_WS` 可以使用其他 WebSocket 地址。仓库不提供托管游戏服务，也不包含 TURN 凭据。

原生 WebRTC 默认使用公共 STUN 服务。需要 TURN 中继时，可在运行时设置 `METEOR_TURN_URLS`、`METEOR_TURN_USERNAME` 和 `METEOR_TURN_PASSWORD`，多个 URL 使用逗号分隔。请将凭据保留在版本控制之外。可选的构建期路由候选地址通过 `METEOR_PRIMARY_WS` 和 `METEOR_SECONDARY_WS` 设置。

## 测试

在仓库根目录运行：

```sh
cd meteor_rust
cargo test --workspace --locked
```

依赖原版游戏文件或独立参考采集数据的测试由 `original-assets` feature 控制。启用该功能前，需要在本地准备匹配的资源与测试数据；这些文件未随源码分发。

本次发布已在 macOS 上使用 Rust 1.97.1 验证。Android、WebAssembly 和可选的 Windows 渲染后端需要各自的平台环境，本次发布未验证这些平台。

## 工程结构

| Crate | 职责 |
| --- | --- |
| `meteor-core` | 共享数据与游戏规则 |
| `meteor-asset` | 资源格式与加载 |
| `meteor-anim` | 动画数据与播放 |
| `meteor-combat` | 战斗与状态效果 |
| `meteor-physics` | 移动与碰撞 |
| `meteor-scene` | 场景与任务脚本 |
| `meteor-ai` | AI 行为 |
| `meteor-input` | 输入映射 |
| `meteor-network` | 协议与多人联机状态 |
| `meteor-app` | Bevy 客户端与桌面程序 |
| `meteor-server` | 独立服务器 |
| `meteor-android`, `meteor-wasm` | 平台入口 crate |

## 参与贡献

欢迎提交问题报告和范围明确的 Pull Request。请附上复现步骤及相关自动化检查结果，避免将凭据、个人配置或原版游戏数据纳入提交。涉及玩法行为的修改，请尽可能提供可复核的依据。

## 许可证

OpenMeteor 项目代码采用 **GNU GPL 第 3 版（仅此版本）**，详见 [LICENSE](LICENSE)。第三方依赖保留各自的许可证，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

本项目是独立的爱好者项目。原版游戏的数据与商标归各自权利人所有，本仓库的软件许可证不授予这些素材的使用权。
