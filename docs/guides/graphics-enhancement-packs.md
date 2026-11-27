# 图形增强包

OpenMeteor 的图形增强包可以按关卡配置 PBR 材质、环境光、风场、接地阴影和局部雾。装饰布面可随风摆动，人物可挂载布料。增强包只影响画面，碰撞、命中判定、移动及联机协议沿用游戏原有逻辑。

## 启用示例

使用支持图形增强包的客户端。从 GitHub 下载源码 ZIP 时，把 `meteor_rust/config/` 下的 `graphics-enhancement.json` 和整个 `graphics-packs/` 目录放进游戏的 `config/`。从含图形增强包的 Windows Release 下载时，直接完整解压即可。

```text
OpenMeteor.exe
config/
  graphics-enhancement.json
  graphics-packs/
    water/
      pack.json
      water-enhancement.json
      water-deep.json
      water-shallow.json
      baked/
    starter/
      pack.json
      textures/
    meshlet-demo/
      pack.json
      models/
```

发行版的 `config/graphics-enhancement.json` 默认启用增强包。以下是仅启用 Starter 的简化格式示例；发行包实际索引还包含其他增强包：

```json
{
  "schema_version": 1,
  "enabled": true,
  "packs": ["graphics-packs/starter/pack.json"]
}
```

发行版包括四方阵山体与草地、长剑 PBR、日月光照，以及桌面支持的 Metal、Vulkan、DX12 后端上的四方阵 HDR 色调。草地接收动态投影，脚步和招式会产生不同的压弯。首次启动默认开启天空、草地、体积雾、动态阴影、环境光和水面增强；已有玩家设置继续保留。布料使用「角色衣物摆动」开关，贴图质量需高于最低档。HDR 的水面和透明特效仍需进一步校准。

将 `enabled` 改回 `false` 并重启即可停用包内配置。水面包及其烘焙资产由 `graphics-packs/water/pack.json` 管理，具体表层由 `config/graphics-packs/water/water-enhancement.json` 控制。锐化、动态阴影继续由游戏图形设置控制。

## 新增场景资源

- `graphics-packs/scene-normals/pack.json` 提供场景法线贴图。它应放在增强包索引的前部，让后续关卡材质可以覆盖相关设置。
- `graphics-packs/sn06-campfires/pack.json` 为四方阵提供营火，随图形增强包启用。
- `graphics-packs/water/pack.json` 将深浅水预设和烘焙的水体纹理一同加载。

## 配置入口

| 字段 | 用法 |
| --- | --- |
| `schema_version` | 当前版本固定为 `1` |
| `enabled` | 总开关，发行版默认 `true`。停用时跳过包和贴图的读取 |
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
| `scenes[].wind` | 关卡风。添加 `response` 后，云、清水、草地和布料使用同一风源 |
| `scenes[].clothing` | 调整布料密度、拉伸、弯折、阻尼、风阻和模拟范围。`iterations` 范围 1–8，提高迭代次数或开启 `self_collision` 会增加 CPU 开销 |
| `scenes[].contact_shadows` | 接地阴影强度、半径和距离。`strength` 范围 0–0.8，设为 0 可停用 |
| `scenes[].fog` | 每关最多 8 个球形雾区，中心使用 Bevy 世界坐标，密度范围 0–0.5。设为 `[]` 可清除之前的雾配置 |

PBR 适用于匹配到的不透明场景批次，原版水面和透明特效由各自的材质处理。`roughness` 范围 0.045–1，数值越低，反射越集中。`metallic` 范围 0–1，`normal_strength` 范围 0–4。颜色图使用 sRGB，法线和 ORM 使用线性数据。静态风动适合旗帜、草木等装饰物，风动高度使用转换后的世界 Y 坐标。示例包的环境图为低分辨率 PNG，方便参考目录结构和贴图参数。

同类场景设置以最后一个显式匹配的条目为准，匹配顺序先按 `packs` 列表、再按包内 `scenes` 列表。把通用 `*` 条目放在关卡条目前面。PNG 单张最多 16 MiB，宽高各最多 4096，客户端启动时读取包清单，进关时解码当前关卡及通用条目所需的资源，解码后的 RGBA 总上限为每关 256 MiB。暂停保留当前资源，离局释放。

修改后重启生效。若启动页提示加载失败，按错误信息检查 JSON 版本、键名和图片路径，或改回 `"enabled": false`。入口 JSON 如已损坏，可以先从 GitHub 恢复模板。

## 关卡风

Starter 的 `stage: "*"` 提供默认风，金华城（`sn13`）有自己的覆盖条目。风向是吹向的世界 X/Z。
`strength` 的范围是 0～4（世界单位/秒），`gust` 是 0～1，`frequency` 是 0～4（弧度/秒），`spatial_scale` 是 0～1。
`response.cloud`、`water`、`grass` 和 `cloth` 默认为 1，范围 0～4。旧包省略整个 `response` 时继续沿用原风动参数。

```json
{
  "stage": "sn13",
  "wind": {
    "direction": [1.0, 0.38],
    "strength": 0.7,
    "gust": 0.25,
    "frequency": 0.7,
    "spatial_scale": 0.08,
    "response": {"cloud": 2.0, "water": 1.0, "grass": 1.5, "cloth": 1.0}
  }
}
```

设 `strength` 为 `0` 可停风，踩草、入水涟漪和飞溅仍可触发。把 `direction` 的两个数取反并重启，可以对照云的漂移和水面的主浪方向。
圆满楼（`sn18`）示例以 `km3cloth.jpg` 选中四处布面。场景布面在 `surfaces[].wind` 写入 `amplitude`、`start_height`、`end_height`，挂布加 `"anchor_top": true` 固定上沿。高度使用世界 Y。角色衣物通过各自的 `wind_drag` 调节风压。

## 可交互草地

开启 `config/graphics-enhancement.json` 后进入一线天（`sn03`）或四方阵（`sn06`）。
Starter 匹配一线天的 `ground / sn302.jpg` 和四方阵的 `Box02 / sn06mian5.jpg` 主庭院。
草叶和地面着色共用从原地表生成的过渡区。靠近石砖、裸土和草区边缘时，草簇逐渐变矮变稀，
地面颜色也逐渐回到原贴图。
草根下铺设 CC0 实拍地表，一线天使用碎石泥地，四方阵使用露土的稀草地。
载入关卡时，把实际草根密度写入同一张边缘遮罩。密草区保留连续的低矮植被底色，稀草区透出扫描图的土色，
远处切换草叶细节等级时也能看见这些斑块。地表从同一次遮罩采样读取边缘和密度。
扫描图按世界坐标铺设，在 1K 色图内合并轻量的环境遮蔽。远处使用 mipmap，地表只增加一次纹理采样，
沿用原地面的绘制和光照。素材来自 Poly Haven 的 [Stony Dirt Path](https://polyhaven.com/a/stony_dirt_path)
和 [Sparse Grass](https://polyhaven.com/a/sparse_grass)。
细长草叶、细芒穗、低矮阔叶和枯黄叶尖成片混合。沿地面跑过，可看到草向外压弯再回弹。
天空替换生效后，草叶使用同一套太阳与天空补光，随昼夜变色。
设置中“战斗与场景特效”下的“可交互草地”可在局内开关。

| `scenes[].grass` 字段 | 用途 |
| --- | --- |
| `enabled`、`surfaces` | 关卡开关和精确的 `{"object":"ground","texture":"sn302.jpg"}` 选择器。结合原版地表坡度与 COB 碰撞面筛选 |
| `bounds`、`min_up` | 可选的 `[[minX,minZ],[maxX,maxZ]]` 世界坐标范围及最小向上法线分量。大场景可用范围限制草地 |
| `density`、`tile_size`、`clump_radius` | 每平方世界单位目标草簇数、草块边长、单簇散开半径。每块最多 192 簇，每关最多 512 块 |
| `species`、`patch_size` | 长草 `grass`、芒穗 `plumes` 和阔叶 `broadleaf` 的相对权重，以及成片变化的尺寸 |
| `patch_variation` | 0–1，控制草簇密度、高度和共享风下弯曲幅度的变化。默认 0 |
| `plume_color` | 可选的线性 RGB，启用后各距离档使用同一套带穗形的草茎 |
| `ground_cover` | 新地表与原地面纹理的最大混合比例。关闭「可交互草地」会一并关闭地表混合 |
| `height`、`width`、`root_color`、`tip_color` | 草叶高度和宽度的最小／最大值、根部与叶尖的线性 RGB |
| `near_distance`、`mid_distance`、`max_distance` | 近、中和最远可见距离。最远范围可设到 240 世界单位，四方阵示例为 180 |
| `max_visible_tiles` | 可见草块硬上限，范围 4–512。四方阵容纳整个比武台的草块，由视锥和三角形预算裁剪。Android 和 Web 额外限制为 40 块 |
| `max_triangles` | 可见草地的每帧提交预算。一线天为 104,000，四方阵加密至 168,000。移动端和 Web 额外限制为 28,000 |
| `wind_strength`、`interaction_radius`、`recovery_seconds` | 草叶的局部弯曲系数、落脚影响范围、回弹时间。共享风启用时，弯曲量还取决于关卡风和 `response.grass` |

关卡可用 `scenes[].grass_ground` 指定草下的地表图。`image` 引用同一包 `resources` 中的 PNG，
`world_size` 是每次平铺覆盖的世界单位，范围 0.5–24。后加载的包可单独替换该地表图。
没有声明地表图的关卡继续使用程序化地表配色。

草根随关卡加载一次，草块共享网格和材质。近、中、远三档每块最多 192、144、96 簇。
最远一档每块最多 96 片单三角形草叶。远近共用草根、叶片朝向、着色和风动，叶宽随距离平滑过渡。
桌面从相机位置分配草块预算，再按视锥裁剪。原地转动镜头只改变哪些草块进入视野。
交互图是相机附近的 64×64 纹理，最多 30 Hz，跟踪最近 8 个落地角色。武器、身体和特效的攻击盒
接近地面时扫倒草叶，重砸向外压草，投射物沿经过场景裁剪的飞行段留痕。每次刷新最多处理 12 段招式碰触，
和脚步共用一张交互图。相邻碰触复用地面采样，停用草地时暂停收集招式几何。渲染使用不透明草叶几何，
关闭单片投影；原场景碰撞、移动和联机数据不受影响。参考 [Grassworks 性能说明](https://grassworks.techredux.co/docs/performance)
和 [Grassworks 交互说明](https://grassworks.techredux.co/docs/interaction)。草簇分布和风动参考
[Sucker Punch 的程序化草地演讲](https://gdcvault.com/play/1027214/)。

验收时沿一线天的泥地走、跑、跳，再静止看离脚处的草回弹。用同一机位和分辨率切换开关对比慢帧；
低配置可降低 `max_visible_tiles` 和三档距离。

## 炽雪城互动雪地

在 `config/graphics-enhancement.json` 中开启增强包并确认 `packs` 包含
`graphics-packs/sn04-interactive-snow/pack.json`，重启后进入炽雪城（`sn04`）。
现有自定义索引可以单独加入这条路径。移除这条路径，或把包内 `snow.enabled` 设为
`false` 并重启，即可关闭雪地互动。

主地面 `di` 的两种积雪材质（`sn0410.jpg` 和 `sn0411.jpg`）上方覆盖一层有厚度的雪。
雪面在石道边缘逐渐变薄，并以斑驳的薄霜露出下方底材。走跑把雪压低并在鞋印边缘推起雪，跳落压出更宽的坑并扬起雪粉，
翻滚沿整段移动路径压出连续沟槽。角色在空中或从更高一层经过时，地面的雪保持原状。
积雪逐渐回填，单机暂停时冻结，新开一局重新开始。各端在本地生成雪面，接地沿用原版碰撞。

自制包可用 `scenes[].snow.surfaces` 指定同一近水平地面的精确对象和贴图对。
`thickness` 控制积雪厚度（0.12–0.58 世界单位，炽雪城为 0.38），
`imprint_strength` 控制压实深度和堆雪强度（0.05–1），`recovery_seconds` 控制回填时间（3–120 秒）。

雪面复用远近两套常驻网格，顶点着色器读取世界坐标下的压实和堆雪高度。未受扰动和较远的区域
使用粗网格，相邻细网格沿公共边缝合到粗网格。桌面最多 8 块细网格，Android 和 WASM 最多 4 块，
且每块精度更低。每次最多处理 32 个接触，保留 64 块稀疏交互数据和 96 个雪粉粒子。
高度修改按 8 KiB 的小块上传，每次最多上传 12 块，交互每秒最多更新 30 次。
雪面静止时复用常驻数据，回填开始后才分批刷新受影响的区域。

验收时沿中央石道走到两侧干净的白雪，低头看脚下。分别走、跑、原地跳落和翻滚，
再返回石道。检查雪层边缘、压坑的深度、两侧堆起的雪、翻滚沟槽的连续性和回填速度。

桌面可用原版资源复测 1080p 同机位的整帧耗时，测试输出 p50、p95 和受扰动块数。
同时设置 `METEOR_SN04_CAPTURE_DIR` 可导出原地面、未踩积雪、脚印、落地和翻滚五张 PNG。

```sh
METEOR_SN04_SNOW_BENCH=1 cargo test --manifest-path meteor_rust/Cargo.toml -p meteor-app --lib sn04_snow -- --nocapture
```

## 体积云和天空盒

将 `config/graphics-enhancement.json` 的 `enabled` 设为 `true`，重启后进入金华城
（`sn13`）。Starter 的 `scenes[].sky` 已配置该关卡；设置中“战斗与场景特效”下的“天空增强”
控制下一局是否应用。体积云在桌面的 Metal、DX12 和 Vulkan 路径可用。

| 字段 | 用途 |
| --- | --- |
| `enabled` | 启用关卡天空增强。关卡无配置时沿用原天空 |
| `replace_skybox` | `true` 时隐藏 `skybox` 中选定的材质批次；`false` 时保留关卡天空盒，包括挡住增强背景的原天空面 |
| `skybox` | 一组精确的 `{"object":"Sphere01","texture":"sky1.bmp"}` 标识。大小写不敏感，支持在同一对象上分别指定多张贴图。替换要求明确的关卡 ID 和非空列表 |
| `horizon`、`zenith`、`cloud_color` | 地平线、天顶和云的线性 RGB |
| `base_height`、`thickness` | 云底高度、云层厚度，使用游戏世界单位 |
| `density`、`coverage`、`scale` | 消光密度、覆盖率、噪声空间频率 |
| `cloud_definition` | 近云边缘密度过渡和侵蚀强度。范围 0～1，默认 0 |
| `weather_scale`、`weather_offset` | 风驱动的区域天气场比例与世界坐标偏移。决定云团聚集的位置、蓝天间隙和云塔大小 |
| `wind`、`steps` | 兼容用的独立 X/Z 风速及每条视线的采样上限，默认 24，范围 8～48。关卡配置 `wind.response` 后，云使用关卡风。短视线和远处薄云会降低实际步数 |
| `cloud_update_hz` | 云缓存更新频率上限，默认 15，范围 1～30。实际频率还受每帧更新行数限制 |
| `sun` | 可选的 `direction`、线性 RGB `color` 和 `intensity`，只用于当前增强天空。省略时取该关卡的 `environment` 太阳 |
| `time` | 可选的关卡时钟。支持自动循环、系统时间和指定时间 |
| `fixed_time` | 兼容旧包的固定时刻写法。仅在省略 `time` 时使用 |
| `ambient_strength`、`ground_bounce` | 天空漫射光强度（0～2）和地表反弹比例（0～1） |

开启替换时，所有标识都匹配后才切换。某项缺失或对象重名时继续绘制原天空，日志会指出对应标识。
金华城示例包括 `Sphere01` 上的 `sky1.bmp`、`sky2.bmp`、`sky3.bmp`，
以及 `Sphere02` 和 `Sphere03` 上的 `am2sky1.jpg`。
区域天气场先确定云团分布、云底和云顶。三维噪声塑造相连的云瓣，并侵蚀云团的边缘。
云内沿太阳方向分段估算遮蔽，叠加多次散射近似。远云随距离染入大气色。
光线步进按分段实际长度积分。长步进在段内分两点采样密度，共用一次光照计算。
全景缓存写入时，根据透射率和相对邻点的亮度变化调整去噪强度。云体细节来自世界空间的密度和自阴影。
风动使用共享的帧时间缓冲，关卡运行时天空材质保持原有绑定。
启用云层的关卡共享 3072×1536 双缓冲全景缓存。每帧最多更新 64 行，云所在的区间算完才切换。
镜头在云层外时跳过背向云层的半球。
全景缓存把三分之二的行数留给地平线上方。每帧沿当前相机方向重投影，按风速补偿位移，最多用 55 毫秒混合新旧结果。
天空底色和太阳仍按画面分辨率计算。缓存首次生成期间使用直接采样。
四方阵和金华城预设使用不同的区域天气比例和偏移。相邻云团共享较平稳的云底和变化的云顶高度。
山谷的太阳方向、强度和色温由 `sky.sun` 指定，天空和地面反弹光的强度可单独调节。
区域天气场与三维密度分层参考 [Guerrilla 的 Nubis 分享](https://www.guerrilla-games.com/read/nubis-authoring-real-time-volumetric-cloudscapes-with-the-decima-engine)。
参考 [HPVolumeCloud](https://github.com/AshenOneArt/HPVolumeCloud) 的散射模型说明，使用内置 WGSL 实现。
云形参考 [Godot 体积云示例](https://github.com/godotengine/godot-demo-projects/blob/master/3d/sky_shaders/sky_volumetric_clouds.gdshader) 的多尺度塑形思路。

成功替换天空盒后，场景和角色会根据天空渐变、云量及云层厚度获得半球漫射光，
同时接受 `sky.sun` 的方向光。开启动态阴影时，投影方向同步。清水使用这套光源计算水体散射。
PBR 表面使用天空漫射和低频镜面环境光。天空补光是关卡级近似，云移动时地面光照保持稳定。
关闭天空增强并重新进入关卡会恢复原有光照路径。

### 水面云倒影

桌面端在启用图形增强包及游戏内「天空增强」「水面增强」后，读取关卡的 `scenes[].water_sky_reflections`。
Starter 包的金华城条目选中了 `Plane02 / sn13w01.jpg`。该表面须在 `water-enhancement.json` 中配置为清水，原天空的替换对象取自关卡生效的 `sky.skybox`。

| 参数 | 作用 |
| --- | --- |
| `surface` | 用 `object` 和 `texture` 指定水面对象名与贴图文件名 |
| `enabled` | 控制该表面的云倒影。多个包匹配时使用最后一项，可用 `false` 覆盖前面的配置 |
| `visibility_resolution`、`visibility_directions` | 遮挡图的空间网格边长与方向数。分别为 16～96 和 4～32，方向数为 4 的倍数 |
| `wave_distortion`、`refraction_damping` | 大波纹对云影轮廓的扰动，以及有云影时对折射波纹的减弱程度，范围均为 0～1 |
| `grazing_boost`、`grazing_range` | 斜视时额外反射的强度及渐入范围。范围端点为 0～1，并按递增顺序填写 |
| `horizon_fade` | 倒影在遮挡边缘的过渡范围。端点为 0～1，并按递增顺序填写 |

修改包配置后重启生效。遮挡精度或天空盒标识发生变化时，桌面端会自动更新该水面的烘焙缓存。
视觉强度和波纹设置复用已有遮挡缓存。启用原版资源离屏测试前，使用 `npm --prefix asset-tools run generate:water` 生成的对应遮挡图。

### 关卡天空时间

在 `config/graphics-packs/starter/pack.json` 找到关卡，例如金华城（`sn13`）。在已有的 `sky` 对象中添加以下任意一项，重启游戏生效。

| 模式 | 添加到 `sky` 的字段 |
| --- | --- |
| 自动循环 | `"time": {"mode": "cycle", "start_time": "07:00", "time_scale": 60}` |
| 固定为进图时的本机时间 | `"time": {"mode": "system"}` |
| 固定为指定时刻 | `"time": {"mode": "fixed", "time": "23:30"}` |

时刻使用 24 小时制 `HH:mm`。`time_scale` 是正数倍率，`60` 表示现实 1 分钟推进关卡 1 小时，完整昼夜约 24 分钟。重新进图从 `start_time` 开始，单机暂停时冻结。系统时间模式进图时读取一次本机时区的时刻。省略 `time` 和 `fixed_time` 时沿用关卡预设。

太阳、云色和天空补光按关卡时钟变化。自动循环的更新频率受 `cloud_update_hz` 限制，云层分批改用新光照。验证跨午夜时可从 `23:30` 开始，用 `60` 倍速，约 30 秒经过午夜。

## HDR 色调映射与泛光

图形增强包的 `color_pipeline` 支持以下配置。

```json
"tonemapping": "none",
"bloom": { "intensity": 0.22, "threshold": 1.15, "threshold_softness": 0.6, "low_frequency_boost": 0.5 }
```

`tonemapping` 取 `aces`（默认）、`none`、`reinhard` 或 `tony_mc_mapface`。`none` 只打开 HDR 缓冲，
不改变原版的观感，适合只想要泛光的包。`bloom.intensity` 为 0 到 1，0 表示关闭；`threshold`
是线性亮度门限，只有刀光、火花这类超过门限的发光内容才会泛光，普通场景贴图不受影响。

## 刀光与特效增强

`combat-fx` 示例包替换武器 Drag 刀光并在命中时播放新的火花、闪光和尘土。原版 `.ef`
的时间轴、音效和 Pose 攻击判定形状不变，只替换显示层。设置里“战斗与场景特效”下的
“刀光与特效增强”可在局内开关；命中特效同时受“武器击打特效”总开关控制。
示例包附带由 `generate_textures.py` 生成的火花、闪光、扬尘和刀光贴图。

包根的 `weapon_trails` 以 `"default"` 或武器 id（`1` 飞镖 … `6` 轻剑、`8` 大刀、`12` 忍刀）为键：

| 字段 | 用途 |
| --- | --- |
| `texture` | 可选资源 id。横向为刀光年龄（新→旧），纵向为刀柄→刀尖 |
| `gradient` | 三段线性 RGB：刀锋处、中段、尾端 |
| `use_pose_color` | 是否乘上 POS `Drag` 里作者给的颜色，默认开 |
| `intensity` | HDR 亮度倍数，默认 3；超过泛光门限的部分会发光 |
| `tip_overshoot` | 刀尖向外延伸的比例（刀身长度），0 到 2 |
| `length_scale` | 缩放作者的 Drag 时间窗口，0.2 到 3 |
| `tail_power`、`edge_softness`、`core` | 尾部衰减曲线、刀柄/刀尖软边宽度、刀尖亮芯强度 |
| `scroll` | 贴图沿刀光方向每秒滚动的 UV 单位 |

包根的 `effects` 以 `.ef` 文件名（不含扩展名，忽略大小写）为键，支持 `*` 和 `?` 通配，
例如 `"W??BL*"` 匹配全部武器命中特效。三个保留键在没有对应 `.ef` 时也会播放：
`@hit` 普通命中、`@guard` 格挡、`@kill` 击杀（缺省回落到 `@hit`）。它们的根朝向这一击：
局部 -Z 是攻击方向，+Z 指回攻击者，+Y 向上。

| 字段 | 用途 |
| --- | --- |
| `replace` | 隐藏原版显示节点（音效保留），只播放 `emitters` |
| `intensity` | 原版节点的亮度倍数（不替换时） |
| `soft` | 原版节点与场景相交处做深度淡出。贴地的地波类特效不要开，会整体变淡 |
| `emitters` | 新增发射器列表，最多 16 个 |

发射器 `type` 有三种。`burst` 一次性喷出 `count` 个相机朝向的精灵：`direction`/`spread` 为
根局部锥体，`speed`、`life`、`size` 为 `[最小, 最大]`，`stretch` 沿速度拉长，`gravity`、`drag`
控制下落和阻尼，`color`→`end_color` 随寿命渐变，`blend` 取 `add` 或 `alpha`，`soft` 开启深度淡出。
`flipbook` 在根位置播放 `texture` 的 `grid` 列×行序列帧，时长 = 帧数 ÷ `fps`，用 `size[0]`。
`light` 是短暂点光：`intensity` 单位流明（15 万左右能照亮周围约两米），`radius` 为范围，`life[0]`
为时长。原版场景材质只读方向光，因此点光通过帧 uniform 注入，最多同时 8 个。

软粒子需要深度预渲染，只在桌面端启用；Android 和 Web 忽略 `soft`。发射器总数上限 96，
点光 8 个，超出的直接丢弃。

## 人物布料

发行包随 Starter 附带 P0 的绛红织锦护衣。启用增强包与「角色衣物摆动」后，选择 P0 即可看到。
两只袖口各跟随对应前臂，四片下摆和两条长腰带只在腰部固定，末端能在跑动时拖尾、急停后回摆。
原版 P0 和重制 P0 共用这些骨骼挂点。
样衣使用 `models/p0-split-coat-sim.obj`、`models/p0-split-coat-render.obj` 和
`textures/p0-split-coat-brocade.png`。原来的 `red_cape` OBJ 和材质仍可由自定义配置引用。
关卡可用 `"character_cloth": [{"character":"p0","enabled":false}]` 隐藏外衣。

样衣通过 `physics.motion_limit` 控制自由段的最大摆幅，通过 `physics.damping` 控制回摆衰减速度，
并用 `physics.wind_drag` 控制它对关卡风场的响应。服装网格的长度和固定点分布也会影响摆动。

Starter 同时给 P1 配了斗笠、前短后长的半透明白纱和整圈粉裙。两件布料网格在周向焊接，
裙腰按站姿校正，裙子从腰到下摆连续变形。帽纱使用头部挂点，粉裙使用腰部挂点，两者均启用低模布面碰撞。
粉裙另启用朝外恢复，减轻屈腿时腿穿过裙面的现象。碰撞以人体胶囊为代理，极端动作仍可能出现局部交叠。
关卡可用 `{"character":"p1","slot":"veil_hat","enabled":false}`
隐藏帽子，粉裙的 `slot` 是 `pink_skirt`。

| 挂载字段 | 用途 |
| --- | --- |
| `character`、`enabled` | 精确的角色资源 ID（例如 `p0`）和开关 |
| `slot` | 同一角色可用不同名称挂多件衣物。同名条目按包和场景顺序覆盖，空名禁用条目作用于该角色的全部衣物 |
| `bones` | 省略 `anchor` 时使用。依次指定颈部、骨盆、左肩、右肩 |
| `anchor` | 单个挂点，使用 `node` 和可选的局部 `offset`。节点类型为 `model`（显示模型根）、`actor`（角色根）、`bone`、`dummy`、`named`（模型下的命名节点）。后三种另填 `name` |
| `rotation_degrees` | 附加的 XYZ 欧拉角，用于对齐 OBJ 与挂点轴向 |
| `colliders` | 1–16 个人体胶囊，每项填 `a`、`b` 和 `radius`。端点采用与 `anchor` 相同的写法。单挂点模式下必填 |
| `mesh` | `id` 指向模拟 OBJ，`render_id` 可选择独立的显示 OBJ，`pin_vertices` 和 `pin_groups` 配置固定点。省略 `mesh` 时使用参数化网格 |
| `grid` | 参数化网格的横向和纵向顶点数，范围 3–12 和 3–16，默认 7×9 |
| `width`、`length`、`back_offset` | 肩部宽度、长度和背向偏移，使用世界单位 |
| `collision_margin` | 人体胶囊外的额外留空，默认 0.016 |
| `keep_visible_on_contact` | 为 `true` 时继续解算低模与胶囊的碰撞，并始终绘制完整衣物。快速动作可以出现短暂穿插。样衣启用此项 |
| `material` | 同包的 PBR 材质 ID，可调颜色、金属度、粗糙度及可选贴图 |
| `opacity` | 0–1，默认 1。小于 1 时使用透明混合，与贴图的 alpha 相乘 |
| `rigid_mesh` | 可选，填 `id` 和 `material`。引用包中的 OBJ，并与主 `anchor` 共用坐标、偏移及旋转，适合帽体 |
| `physics` | 可选的独立布料配置，可调整密度、阻尼、风阻、弯折和模拟范围。省略时继承关卡布料配置 |

自制斗篷放进增强包，例如包根填写 `"cloth_meshes": {"cape": "models/cape.obj"}`，在角色条目填写
`"mesh": {"id": "cape", "pin_vertices": [1, 2, 3]}`。固定点按 OBJ 的 `v` 行从 1 编号，每个独立布片至少要有一个固定点。
导出时三角化，以米为单位，局部原点设在挂点，+Y 朝上，裙边沿 -Y 下垂。模拟 OBJ 最多 384 个位置和 768 个三角形。
OBJ 的材质库不读取。颜色、粗糙度和贴图在增强包中配置，有贴图的模型需要 UV。
导出后可通过 `rotation_degrees` 对正轴向。带缩放的角色节点只提供位移和旋转，`offset`、网格和胶囊半径均使用世界单位。

下面的角色条目使用无骨骼模型中名为 `cape_socket` 的节点。

```json
{
  "character": "p0",
  "material": "red_cape",
  "mesh": {"id": "red_cape", "pin_vertices": [1, 2, 3, 4, 5, 6, 7]},
  "anchor": {"node": {"kind": "named", "name": "cape_socket"}, "offset": [0, 0, 0.35]},
  "back_offset": 0,
  "colliders": [{
    "a": {"node": {"kind": "actor"}, "offset": [0, 0.5, 0]},
    "b": {"node": {"kind": "actor"}, "offset": [0, 1.45, 0]},
    "radius": 0.24
  }]
}
```

命名节点必须在当前显示模型下唯一。节点稍后加载时会重新查找，挂点或自定义胶囊端点找不到时隐藏布料。

#### 外套和其他多挂点服装

在包清单的 `cloth_meshes` 中登记 `"coat_sim": "models/coat-sim.obj"` 和
`"coat_render": "models/coat-render.obj"`。把角色条目中的 `mesh` 替换为对应的配置。

```json
{
  "id": "coat_sim",
  "render_id": "coat_render",
  "pin_vertices": [1, 2],
  "pin_groups": [
    {
      "anchor": {"node": {"kind": "named", "name": "left_cuff"}},
      "vertices": [17, 18],
      "rest_origin": [-0.48, -0.28, 0.04]
    },
    {
      "anchor": {"node": {"kind": "named", "name": "right_cuff"}},
      "vertices": [27, 28],
      "rest_origin": [0.48, -0.28, 0.04]
    }
  ]
}
```

`pin_vertices` 继续跟随主挂点，`pin_groups` 中每组可以选择任意一种节点类型。顶点编号来自模拟 OBJ。
`rest_origin` 是该挂点在模拟 OBJ 静止姿势中的坐标。若静止姿势下挂点的轴向与 OBJ 不同，可填
`rest_rotation_degrees`，按 XYZ 欧拉角指定。焊接到同一个位置的固定点必须归属同一挂点。
同一布片可有多个挂点，每个独立布片至少固定一个顶点。

两个 OBJ 使用同一坐标系和静止姿势。显示 OBJ 可增加褶皱和厚度，最多 4,096 个位置、8,192 个三角形，
单个 OBJ 的大小上限为 1 MiB。显示顶点与模拟表面的距离须在 0.35 米内，贴图 UV 放在显示 OBJ 上。
两套网格在加载时绑定。运行时只求解模拟网格，每个模拟三角形的局部坐标系每次更新只计算一次。
多挂点或双网格的布料在慢帧最多补算 4 个子步，暂停且挂点与碰撞体未动时复用上一帧网格。
多人同时穿戴时，可通过关卡布料的 `max_characters` 和 `max_distance` 限制同时更新的近处衣物。
同一角色的多个 `slot` 各占一个模拟名额。

固定点跟随挂点，人体胶囊参与低模求解。样衣在快速动作中保持整件绘制，胶囊近似可能留下局部穿插。
未启用 `keep_visible_on_contact` 时，额外检查整片布料与胶囊的间距。出现重叠先尝试恢复静止形状，
仍重叠的帧隐藏布料。
布料自碰撞基于粒子接触，关卡墙面、地形和跨服装层的连续碰撞均不参与求解。

设置「角色衣物摆动」继续控制此功能；最低画质、死亡、超距、传送或超过 0.2 秒的卡顿
会清除旧惯性并恢复动画面。暂停／打击停顿不推进粒子。关闭增强包后，游戏自带布料
仍受该设置控制，包内风场和材质参数不再生效；要完全关闭布料请关闭该设置。

P1 的 `physics.surface_collision` 用低模三角形补充人体胶囊接触。`physics.surface_recovery` 设为 0–0.5 世界单位，可沿朝外的法线恢复已跨到布面另一侧的接触。

在四方阵选 P1，开启图形增强和「角色衣物摆动」。站立时检查裙腰和白纱，跑动急停后查看下摆回落，再用闪避检查大腿和裙面的交叠。

## 清水预设

水面通过 `config/graphics-packs/water/water-enhancement.json` 的 `surfaces` 选择。每条规则用 `stage`、`object`、`texture` 精确匹配清水表面。省略 `profile` 时使用 `deep`。旧配置的 `legacy` 也按 `deep` 处理。金华城默认使用 `classic`，按河床深度呈现青绿色水面，岸墙倒影会随风浪和脚边波纹扭动。三种预设均支持触水波纹和飞溅。深水与浅水的折射遇到人物或武器前景时回退到原位置采样，清水表面不参与镜头遮挡消隐。配置编辑后重启。

| `profile` | 配置文件 | 随包示例 |
| --- | --- | --- |
| `classic` | `water-enhancement.json` 中该表层的 `extinction` | `sn13 / Plane02 / sn13w01.jpg` |
| `deep` | `config/graphics-packs/water/water-deep.json` | `sn06 / Cylinder01 / sn06w.jpg`、`sn09 / Plane37 / sn914.jpg` |
| `shallow` | `config/graphics-packs/water/water-shallow.json` | 将所选清水表层的 `profile` 改成 `shallow` |

两份配置的 `optics` 独立控制 `absorption`、`scattering`、`anisotropy`、`ior`、`roughness` 和 `max_path`。`refraction_strength` 设为 0 可关闭屏幕空间折射。浅水另有 `simulation`，用于调整重力、阻尼、模拟水深范围和网格大小。角色触水时，扰动沿河床范围传播。

折射颜色会按水下深度过滤，包括 MSAA 像素边缘。命中水面前景时，改用当前水面像素下方的透射颜色。

两份水体预设与 `water-enhancement.json` 放在同一目录。缺少的预设使用内置默认值。清水光学参考 [HPWater](https://github.com/AshenOneArt/HPWater/wiki/BSDF%E2%80%90Model)，浅水模拟参考 [Unity-SWE](https://github.com/AshenOneArt/Unity-SWE)。

## Meshlet 模型替换

Windows DX12、桌面 Vulkan 和 macOS Metal 可以使用 v2 清单为静态不透明物件指定外部模型。要试用随包附带的青铜摆件，把 `graphics-packs/meshlet-demo/pack.json` 加入 `packs`，确认 `enabled` 为 `true`，重启进入 `sn07` 查看 `Cy01`。样例匹配指定版本的原版关卡文件，成功时日志会显示 `graphics meshlets sn07 / Cy01` 和 74 个簇。

移除该包并重启可以恢复原外观。模型缺失、源文件版本不同、目标冲突或渲染后端不支持时，也会保留原版物件。碰撞和联机数据始终来自原版。局域网可用一端启用、一端关闭，核对同一位置的通行和攻击结果。

`meshlet_models` 指定相对清单的 `.mlmesh` 路径及材质槽映射，`scenes[].meshlet_replacements` 用 `object` 和 `model` 绑定静态对象，可用 `placement` 调整世界偏移、绕 Y 轴的角度与统一缩放。v1 材质包仍能使用。Meshlet 文件由桌面客户端在进入对应关卡时读取，Web、Android 和 OpenGL 路径显示原版物件。

## 四方阵苔岩增强

桌面通过 `METEOR_GRAPHICS_CONFIG` 选用 `config/graphics-enhancement-sn06.json`。该预设包含四方阵的草地、天空、风和雾配置，以及运行时 Meshlet 山岩和贴图。

在 `scenes[].meshlet_props` 中填入 `model` 和 `placements` 可以添置静态装饰。摆位的 `offset` 以关卡中心为原点，按世界坐标书写。同组装饰复用网格和材质，每关最多提交 32 个装饰实例，与替换模型共用渲染预算。可通过 `source_sha256` 绑定特定版本的关卡 GMB。

`config/graphics-packs/sn06-moss-valley/pack.json` 将瀑布层、山脊层和天际线分别绑定到 `Line01`、`Line02` 和 `Line03`，并在山脚摆放苔岩。近层加强直射光并减薄雾，中层保留部分雾，外层使用偏冷的材质和较强的雾衰减。可通过 `direct_light`、`ambient_light` 和 `fog_strength` 调整材质，取值均为 0–4，默认 1。原版碰撞和瀑布对象沿用原有数据。

增强清水按前景深度判断折射采样，在人物边缘回退到透明混合。落水冲量按水线交点定位，岸边可传到相邻湿网格。共享水花网格和材质在入场时准备。

## 一线天峡谷增强

用 `METEOR_GRAPHICS_CONFIG` 选用 `config/graphics-enhancement-sn03.json`。
也可把 `graphics-packs/sn03-mist-ravine/pack.json` 加到已有入口的 `packs` 列表。
两侧使用 **Nordic Forest Cliff Large** 扫描的起伏与苔岩贴图，沿原有岩面生成连续显示网格。
高处陡壁加强岩层凸棱，近路和缓坡贴合原地形。谷底显示层加入碎石、风化岩和按峡壁遮挡烘焙的明暗，
边缘用薄层碎岩接到石壁。后端回退时沿用原网格及可用的增强材质。
中景替换两处无碰撞的外侧岩山。南端峡口后面新增两组独立 Meshlet 模型。**Volcanic Dolerite Cliff**
（`qkofq`）作为两侧的断层岩壁，**Icelandic Weathered Lava Spire**（`siuw3`）的天然石柱是峡口的主景。
**Huge Nordic Coastal Cliff**（`ullqbb1ga`）用于副峰和后方远山。峡口的原版装饰岩还混入
**Japanese Mossy Boulder** 的轮廓。
暖色侧光照向石柱和断壁的可见面，薄雾集中在谷口低处和远山之间。原有远山贴片和高处雾片随「天空增强」切换。
路面使用原版 `ground` 的三个材质区摆放短草，保留石路和碎石露出的间隙。谷底碰撞、栈道和机关沿用原版数据。
开启「天空增强」「体积雾」和「可交互草地」后，从谷底走到峡口，再沿原有路线登上高处，
核对路面草丛、近壁轮廓和石柱与远山的层次。
