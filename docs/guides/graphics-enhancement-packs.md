# 图形增强包

OpenMeteor 的图形增强包可以按关卡配置 PBR 材质、环境光、风场、接地阴影和局部雾。合适的装饰网格可以随风摆动，已制作衣料区域的 P0 重制模型可以使用布料模拟。增强包只影响画面，碰撞、命中判定、移动及联机协议沿用游戏原有逻辑。

## 启用示例

使用支持图形增强包的客户端。从 GitHub 下载源码 ZIP 时，把 `meteor_rust/config/` 下的 `graphics-enhancement.json` 和整个 `graphics-packs/` 目录放进游戏的 `config/`。从含图形增强包的 Windows Release 下载时，直接完整解压即可。

```text
OpenMeteor.exe
config/
  graphics-enhancement.json
  water-enhancement.json
  water-deep.json
  water-shallow.json
  graphics-packs/
    starter/
      pack.json
      textures/
    meshlet-demo/
      pack.json
      models/
```

打开 `config/graphics-enhancement.json`，把 `enabled` 改成 `true` 并重启：

```json
{
  "schema_version": 1,
  "enabled": true,
  "packs": ["graphics-packs/starter/pack.json"]
}
```

示例在 `sn06` 为原版贴图 `sn064.jpg` 对应的不透明表面应用石材 PBR。打开游戏内的「体积雾」后，可以看到该关卡的局部雾。通用条目提供风场和接地阴影。衣物效果需要打开「角色衣物摆动」，使用带布料网格的 P0 重制模型，并且贴图质量高于最低档。只使用原版资源时，可以查看场景增强，原版角色不会自动获得布料网格。

将 `enabled` 改回 `false` 并重启即可停用包内配置。水面由独立的 `config/water-enhancement.json` 控制。锐化、动态阴影继续由游戏图形设置控制。

## 配置入口

| 字段 | 用法 |
| --- | --- |
| `schema_version` | 当前版本固定为 `1` |
| `enabled` | 总开关，示例默认 `false`。停用时跳过包和贴图的读取 |
| `packs` | 按加载顺序列出 `pack.json` 的相对路径，最多 16 项 |

桌面客户端优先采用 `METEOR_GRAPHICS_CONFIG` 指定的文件。如果未指定，按以下顺序查找，采用第一个存在的文件：

1. `METEOR_USER_DATA_DIR` 指定目录下面的 `config/graphics-enhancement.json`。
2. 当前工作目录的 `config/graphics-enhancement.json`。
3. 当前工作目录的 `meteor_rust/config/graphics-enhancement.json`。
4. 可执行文件旁的 `config/graphics-enhancement.json`。

桌面未找到配置时，以及 Web 和 Android 上，使用资源包 `graphics/` 下的配置或内置的默认关闭配置。Web 和 Android 需要重新生成并分发包含这些资源的 MPK。

## 修改或制作增强包

复制 `starter/` 为新目录，给 `pack.json` 设置独立的 `id`，再把它的路径加入 `packs`。`resources` 将资源 ID 映射到相对 `pack.json` 的 PNG 路径。路径使用正斜杠，保留目录结构。

| 配置位置 | 作用 |
| --- | --- |
| `materials` | 按 ID 定义颜色图 `base_color`、切线空间法线图 `normal`、ORM 贴图 `orm`。ORM 的 R、G、B 分别是遮蔽、粗糙度和金属度 |
| `scenes[].stage` | 关卡 ID，例如 `sn06`。`*` 适用于所有关卡 |
| `scenes[].surfaces` | 用 `object` 或 `texture` 精确匹配原版物件或贴图名，至少填一项。两者同时填写时必须同时匹配。`material` 指向同包材质 ID |
| `scenes[].environment` | 调整环境光强、曝光、旋转和太阳光。提供环境图时使用 2:1 等距柱状 PNG，`specular_mips` 按宽高逐级减半排列 |
| `scenes[].wind` | 风向使用世界 X/Z，`strength` 范围 0–4，`gust` 范围 0–1。表面的 `wind` 可单独指定幅度及起止高度 |
| `scenes[].clothing` | 调整布料密度、拉伸、弯折、阻尼、风阻和模拟范围。`iterations` 范围 1–8，提高迭代次数或开启 `self_collision` 会增加 CPU 开销 |
| `scenes[].contact_shadows` | 接地阴影强度、半径和距离。`strength` 范围 0–0.8，设为 0 可停用 |
| `scenes[].fog` | 每关最多 8 个球形雾区，中心使用 Bevy 世界坐标，密度范围 0–0.5。设为 `[]` 可清除之前的雾配置 |

PBR 适用于匹配到的不透明场景批次，原版水面和透明特效由各自的材质处理。`roughness` 范围 0.045–1，数值越低，反射越集中。`metallic` 范围 0–1，`normal_strength` 范围 0–4。颜色图使用 sRGB，法线和 ORM 使用线性数据。静态风动适合旗帜、草木等装饰物，风动高度使用转换后的世界 Y 坐标。示例包的环境图为低分辨率 PNG，方便参考目录结构和贴图参数。

同类场景设置以最后一个显式匹配的条目为准，匹配顺序先按 `packs` 列表、再按包内 `scenes` 列表。把通用 `*` 条目放在关卡条目前面。PNG 单张最多 16 MiB，宽高各最多 4096，所有包解码后的 RGBA 合计最多 256 MiB。

修改后重启生效。若启动页提示加载失败，按错误信息检查 JSON 版本、键名和图片路径，或改回 `"enabled": false`。入口 JSON 如已损坏，可以先从 GitHub 恢复模板。

## 可交互草地

开启图形增强包后进入一线天（`sn03`）。示例在 `ground / sn302.jpg` 的泥地铺草。沿底层泥地跑过，脚边的草会弯倒，走开后回弹。局内通过设置里的「可交互草地」切换。测帧耗时，用相同分辨率、画质和机位分别记录开启和关闭后的帧时间。

在 `config/graphics-packs/starter/pack.json` 的 `scenes[].grass` 调整以下参数。修改后重启。

| 字段 | 用法 |
| --- | --- |
| `enabled`、`surfaces`、`bounds` | 关卡开关、精确的对象名与贴图名，以及可选的 `[[minX,minZ],[maxX,maxZ]]` 世界坐标范围 |
| `density`、`tile_size`、`min_up` | 每平方世界单位的目标叶片数、草块边长和最小向上法线分量 |
| `height`、`width`、`root_color`、`tip_color` | 叶片高宽范围及根部、叶尖的线性 RGB |
| `near_distance`、`mid_distance`、`max_distance` | 三档距离阈值。越远越稀疏，超过最远距离裁剪 |
| `max_visible_tiles` | 每帧可见草块上限。本例为 96。调低可减少渲染开销 |
| `wind_strength`、`interaction_radius`、`recovery_seconds` | 风弯曲量、脚边影响范围和回弹时间 |

三档每块最多绘制 192、72、24 片草。踩踏只更新相机附近的 64×64 交互纹理，最多跟踪 8 个落地角色。参考 [Grassworks](https://grassworks.techredux.co/docs/performance) 的分块和 LOD 方法。

## 体积云和天空盒

开启图形增强包后进入金华城（`sn13`）。在 `scenes[].sky` 指定 `enabled` 和 `replace_skybox`。后者开启时，`skybox` 里每组 `{"object":"Sphere01","texture":"sky1.bmp"}` 都要匹配，才会隐藏指定的原版材质批次。关闭替换会保留原天空盒，原天空面可能遮挡后方的体积云。游戏内的「天空增强」设置在下一局生效。

`horizon`、`zenith` 和 `cloud_color` 是线性 RGB。可用 `base_height`、`thickness`、`coverage`、`density`、`scale`、`wind` 调整云层。`steps` 是采样步数，默认 24，范围 8–48。Windows 上使用 DX12 或 Vulkan。参考 [HPVolumeCloud](https://github.com/AshenOneArt/HPVolumeCloud) 的散射模型说明。

## 深水和浅水

水面通过 `config/water-enhancement.json` 的 `surfaces` 选择。每条规则用 `stage`、`object`、`texture` 精确匹配清水表面。省略 `profile` 时使用 `deep`。旧配置的 `legacy` 也按 `deep` 处理。金华城使用 `shallow`。两种预设均保留触水波纹和飞溅。配置编辑后重启。

| `profile` | 配置文件 | 随包示例 |
| --- | --- | --- |
| `deep` | `config/water-deep.json` | `sn06 / Cylinder01 / sn06w.jpg`、`sn09 / Plane37 / sn914.jpg` |
| `shallow` | `config/water-shallow.json` | 金华城 `sn13 / Plane02 / sn13w01.jpg` |

两份配置的 `optics` 独立控制 `absorption`、`scattering`、`anisotropy`、`ior`、`roughness` 和 `max_path`。`refraction_strength` 设为 0 可关闭屏幕空间折射。浅水另有 `simulation`，用于调整重力、阻尼、模拟水深范围和网格大小。角色触水时，扰动沿河床范围传播。

两份水体预设与 `water-enhancement.json` 放在同一目录。缺少的预设使用内置默认值。清水光学参考 [HPWater](https://github.com/AshenOneArt/HPWater/wiki/BSDF%E2%80%90Model)，浅水模拟参考 [Unity-SWE](https://github.com/AshenOneArt/Unity-SWE)。

## Meshlet 模型替换

Windows DX12、桌面 Vulkan 和 macOS Metal 可以使用 v2 清单为静态不透明物件指定外部模型。要试用随包附带的青铜摆件，把 `graphics-packs/meshlet-demo/pack.json` 加入 `packs`，确认 `enabled` 为 `true`，重启进入 `sn07` 查看 `Cy01`。样例匹配指定版本的原版关卡文件，成功时日志会显示 `graphics meshlets sn07 / Cy01` 和 74 个簇。

移除该包并重启可以恢复原外观。模型缺失、源文件版本不同、目标冲突或渲染后端不支持时，也会保留原版物件。碰撞和联机数据始终来自原版。局域网可用一端启用、一端关闭，核对同一位置的通行和攻击结果。

`meshlet_models` 指定相对清单的 `.mlmesh` 路径及材质槽映射，`scenes[].meshlet_replacements` 用 `object` 和 `model` 绑定静态对象，可用 `placement` 调整世界偏移、绕 Y 轴的角度与统一缩放。v1 材质包仍能使用。Meshlet 文件由桌面客户端在进入对应关卡时读取，Web、Android 和 OpenGL 路径显示原版物件。
