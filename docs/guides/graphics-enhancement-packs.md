# 图形增强包

OpenMeteor 的图形增强包可以按关卡配置 PBR 材质、环境光、风场、接地阴影和局部雾。装饰布面可随风摆动，人物可挂载布料。增强包只影响画面，碰撞、命中判定、移动及联机协议沿用游戏原有逻辑。

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

示例在 `sn06` 为原版贴图 `sn064.jpg` 对应的不透明表面应用石材 PBR。打开「体积雾」可看到关卡的局部雾。通用条目提供风场、接地阴影及 P0、P1 的布料示例。布料使用「角色衣物摆动」开关，贴图质量需高于最低档。

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
| `scenes[].wind` | 关卡风。添加 `response` 后，云、清水、草地和布料使用同一风源 |
| `scenes[].clothing` | 调整布料密度、拉伸、弯折、阻尼、风阻和模拟范围。`iterations` 范围 1–8，提高迭代次数或开启 `self_collision` 会增加 CPU 开销 |
| `scenes[].contact_shadows` | 接地阴影强度、半径和距离。`strength` 范围 0–0.8，设为 0 可停用 |
| `scenes[].fog` | 每关最多 8 个球形雾区，中心使用 Bevy 世界坐标，密度范围 0–0.5。设为 `[]` 可清除之前的雾配置 |

PBR 适用于匹配到的不透明场景批次，原版水面和透明特效由各自的材质处理。`roughness` 范围 0.045–1，数值越低，反射越集中。`metallic` 范围 0–1，`normal_strength` 范围 0–4。颜色图使用 sRGB，法线和 ORM 使用线性数据。静态风动适合旗帜、草木等装饰物，风动高度使用转换后的世界 Y 坐标。示例包的环境图为低分辨率 PNG，方便参考目录结构和贴图参数。

同类场景设置以最后一个显式匹配的条目为准，匹配顺序先按 `packs` 列表、再按包内 `scenes` 列表。把通用 `*` 条目放在关卡条目前面。PNG 单张最多 16 MiB，宽高各最多 4096，所有包解码后的 RGBA 合计最多 256 MiB。

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

开启图形增强包后可在一线天（`sn03`）和四方阵（`sn06`）测试。前者匹配 `ground / sn302.jpg`，后者覆盖 `Box02 / sn06mian5.jpg` 主庭院。细长草叶、芒穗、阔叶及枯黄叶尖成片混合。跑过时草会弯倒，离开后回弹。天空替换生效后，草叶随天空补光和太阳变亮或变暗。 近地的挥砍、剑气和飞行道具也可压弯草叶，动作经过后草会回弹。局内可切换「可交互草地」，在同一机位和分辨率对比帧时间。

在 `config/graphics-packs/starter/pack.json` 的 `scenes[].grass` 调整以下参数。修改后重启。

| 字段 | 用法 |
| --- | --- |
| `enabled`、`surfaces`、`bounds` | 关卡开关、精确的对象名与贴图名，以及可选的 `[[minX,minZ],[maxX,maxZ]]` 世界坐标范围 |
| `density`、`tile_size`、`min_up` | 每平方世界单位的目标草簇数、草块边长和最小向上法线分量 |
| `height`、`width`、`root_color`、`tip_color` | 叶片高宽范围及根部、叶尖的线性 RGB |
| `near_distance`、`mid_distance`、`max_distance` | 近、中和最远可见范围，最远可设到 240 世界单位，四方阵示例为 180 |
| `max_visible_tiles`、`max_triangles` | 草块和三角形硬上限。一线天为 96 块、104,000 个三角形，四方阵高密度预设为 512 块、168,000 个三角形 |
| `species`、`patch_size`、`clump_radius`、`ground_cover` | 植被类型权重、成片变化尺寸、草簇半径和地表覆盖强度 |
| `patch_variation` | 0–1，控制草簇密度、高度和共享风下弯曲幅度的变化。默认 0 |
| `plume_color` | 可选线性 RGB，启用后各距离档使用同一套带穗草茎 |
| `wind_strength`、`interaction_radius`、`recovery_seconds` | 风弯曲量、脚边影响范围和回弹时间 |

最远一档每草块最多提交 96 个单三角形草叶。远近共用草根、叶片朝向、着色和风动，叶宽随距离过渡。桌面从相机位置分配草块预算，再按视锥裁剪。踩踏只更新相机附近的 64×64 交互纹理，最多跟踪 8 个落地角色。远景覆盖补偿参考 [AMD GPUOpen 的草地渲染实现](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-procedural_grass_rendering/#level-of-detail)。

## 体积云和天空盒

开启图形增强包后进入金华城（`sn13`）。在 `scenes[].sky` 指定 `enabled` 和 `replace_skybox`。后者开启时，`skybox` 里每组 `{"object":"Sphere01","texture":"sky1.bmp"}` 都要匹配，才会隐藏指定的原版材质批次。关闭替换会保留原天空盒，原天空面可能遮挡后方的体积云。游戏内的「天空增强」设置在下一局生效。

`horizon`、`zenith` 和 `cloud_color` 是线性 RGB。可用 `base_height`、`thickness`、`coverage`、`density` 和 `scale` 调整云层。`steps` 是每条视线的采样上限，默认 24，范围 8～48。`sky.wind` 用于未设置共享风的旧包。
`cloud_update_hz` 是全景缓存更新频率上限，默认 15，范围 1～30。每帧只刷新一部分缓存，按当前相机方向取样。
成功替换天空盒后，场景和角色使用新天空的补光，共用 `sun` 的颜色和方向。`ambient_strength` 和 `ground_bounce` 用于调整天空漫射光和地表反弹。
Windows 上使用 DX12 或 Vulkan。参考 [HPVolumeCloud](https://github.com/AshenOneArt/HPVolumeCloud) 的散射模型说明。

在关卡的 `sky` 对象中添加一种时间模式，重启生效。旧的 `fixed_time` 字段也可继续用，配新模式时移除旧字段。

| 模式 | 配置 |
| --- | --- |
| 自动循环 | `"time": {"mode":"cycle","start_time":"07:00","time_scale":60}` |
| 进图时的本机时间 | `"time": {"mode":"system"}` |
| 指定时刻 | `"time": {"mode":"fixed","time":"23:30"}` |

`60` 倍表示现实 1 分钟推进关卡 1 小时。单机暂停冻结，重新进图从指定起点开始。要快速看过午夜，从 `23:30` 开始，约 30 秒后到午夜。

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

## 深水和浅水

水面通过 `config/water-enhancement.json` 的 `surfaces` 选择。每条规则用 `stage`、`object`、`texture` 精确匹配清水表面。省略 `profile` 时使用 `deep`。旧配置的 `legacy` 也按 `deep` 处理。金华城默认使用 `deep`，两种预设均保留触水波纹和飞溅。 水面折射遇到人物或武器前景时回退到原位置采样，清水表面不参与镜头遮挡消隐。配置编辑后重启。

| `profile` | 配置文件 | 随包示例 |
| --- | --- | --- |
| `deep` | `config/water-deep.json` | `sn06 / Cylinder01 / sn06w.jpg`、`sn09 / Plane37 / sn914.jpg`、`sn13 / Plane02 / sn13w01.jpg` |
| `shallow` | `config/water-shallow.json` | 将所选清水表层的 `profile` 改成 `shallow` |

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
