# Third-party notices

OpenMeteor project code is distributed under GPL-3.0-only. Third-party components
retain their own licenses and copyright notices.

| Component | Upstream | License |
| --- | --- | --- |
| Bevy render and PBR patches | https://github.com/bevyengine/bevy | MIT OR Apache-2.0; embedded BSD-licensed atmosphere code |
| wgpu-hal patch | https://github.com/gfx-rs/wgpu | MIT OR Apache-2.0 |
| bevy_dlss5 | https://github.com/AlrikOlson/bevy_dlss5 | MIT OR Apache-2.0 |
| Bevy default font | Distributed by the Bevy text dependency | SIL Open Font License 1.1 |
| Protocol compatibility fixture, @bufbuild/protobuf | https://github.com/bufbuild/protobuf-es | Apache-2.0 |

Vendored license texts are included beside their source. Other dependencies are
resolved from the public sources recorded in Cargo.lock and retain their own terms.
The original game's data, executable, audiovisual assets, names, and trademarks
are not covered by this repository's software license.

Additional shader copyright and license notices are preserved in
[bevy_pbr/NOTICE](meteor_rust/vendor/bevy_pbr/NOTICE).

Font license: [FiraMono OFL-1.1](licenses/FiraMono-OFL-1.1.txt).
Protocol fixture license: [Apache-2.0](meteor_rust/crates/meteor-network/tests/fixtures/LICENSE-APACHE).

Vendored code carries local rendering adaptations and has had source comments
removed for this distribution. The upstream license texts and separate
attribution notices are retained.
