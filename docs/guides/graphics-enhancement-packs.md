# 图形增强包

OpenMeteor 的图形增强包可以按关卡配置 PBR 材质、环境光、风场、接地阴影和局部雾。合适的装饰网格可以随风摆动，已制作衣料区域的 P0 重制模型可以使用布料模拟。增强包只影响画面，碰撞、命中判定、移动及联机协议沿用游戏原有逻辑。

## 启用示例

使用支持图形增强包的客户端。从 GitHub 下载源码 ZIP 时，把 `meteor_rust/config/` 下的 `graphics-enhancement.json` 和整个 `graphics-packs/` 目录放进游戏的 `config/`。从含图形增强包的 Windows Release 下载时，直接完整解压即可。

```text
OpenMeteor.exe
config/
  graphics-enhancement.json
  graphics-packs/
    starter/
      pack.json
      textures/
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
