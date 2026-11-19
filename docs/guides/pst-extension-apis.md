# PST 扩展 API

这些接口可以直接写在 OpenMeteor 的 `.pst` 回调里。接口名参考 AS 脚本引擎 2.7.7，返回值按下面的约定使用。

| API | 脚本 | 返回值 |
| --- | --- | --- |
| `GetSceneItemCount()` | 场景、任务、NPC | 道具槽数。可遍历 `0 <= id < count`。DES 原有序号保留，动态道具追加在末尾。 |
| `GetSceneItemId(name)` | 场景、任务、NPC | 道具的场景 ID，找不到返回 `-1`。名称查找忽略大小写，并沿用 PST 道具名的补零规则。 |
| `GetSceneItemName(id)` | 场景、任务、NPC | 道具名称。无效或不可用的 ID 返回空字符串。 |
| `GetWeapon2(characterId)` | 单机任务、NPC | 角色当前备用槽的 `Meteor.res` 武器道具 ID。空槽返回 `0`，找不到角色返回 `-1`。 |
| `GetWeaponType2(characterId)` | 单机任务、NPC | 备用武器类型，例如剑为 `6`，忍刀为 `11`。空槽返回 `0`，找不到角色返回 `-1`。 |
| `PlayMusic(path)` | 客户端场景、单机任务、NPC | 切换循环背景音乐。接受 `music/` 下本客户端可读的 MP3，接受请求返回 `1`，路径无效或文件缺失返回 `0`。 |
| `ExecuteFunction(target, function, ...)` | 场景、单机任务、NPC | 同步调用目标脚本中的函数。调用完成返回 `1`，目标脚本或函数不存在返回 `0`。运行错误会中止当前调用并记录诊断。 |
| `GetSceneScriptId()`、`GetLevelScriptId()`、`GetNpcScriptId(character)` | 场景、单机任务、NPC | 返回本关卡内可供 `ExecuteFunction` 使用的脚本 ID。不存在时返回 `-1`。 |
| `CreateSceneItem(name, "pickup", model[, actorId])` | 单机的场景、任务、NPC | 在角色脚边生成拾取物。省略角色时使用玩家 `0`。返回本关内保留的道具 ID，参数无效、名称重复或超过上限时返回 `-1`。 |
| `CharSwitchWeapon(actorId)` | 单机的场景、任务、NPC | 请求存活角色切换至备用槽，返回 `1` 表示接受，`0` 表示角色不可用。备用槽有武器时走实际的换武器动画，在动画提交点切换。 |
| `Reborn(actorId)` | 单机的场景、任务、NPC | 请求复活任务名单中已死亡的角色，接受返回 `1`，否则返回 `0`。角色恢复满血、回原出生点，角色编号和 NPC 脚本变量保留，出生保护重新计时。 |
| `UiCreatePanel(id, title)` | 单机的场景、任务、NPC | 创建隐藏面板。 |
| `UiAddText(id, text)` | 单机的场景、任务、NPC | 向面板追加一段文字。 |
| `UiAddButton(id, buttonId, label)` | 单机的场景、任务、NPC | 添加按钮，`buttonId` 在该面板内唯一。 |
| `UiShowPanel(id)`、`UiClosePanel(id)` | 单机的场景、任务、NPC | 显示面板或关闭并销毁面板。 |

面板接口成功返回 `1`，参数无效、ID 重复、目标不存在或超过容量时返回 `0`。ID 区分大小写，可用字母、数字和下划线。场景和任务各自管理同名面板，同一时刻只显示最后请求显示的一个。每关最多保留 16 个面板，每个面板可放 6 段文字和 6 个按钮。

点击按钮会调用面板所属脚本的 `OnUiButton(string panel, string button)`。NPC 创建的面板归任务脚本处理。带按钮的面板暂时拦截玩家操作并释放桌面光标，脚本可在回调里调用 `UiClosePanel`。纯文字面板保持玩家操作可用。

道具被隐藏或暂时禁用时，仍可以按 ID 查询其名称。角色换武器后，备用槽跟随当前激活槽切换。同一类型的不同炼化档具有不同的武器道具 ID。

动态道具在当前回调中即可通过 ID 或名称查询，实体在后续系统阶段创建。名称和模型名使用 ASCII 字母、数字和下划线，每局最多创建 256 个。模型名使用已有资源的文件名（如剑 `W0_0`），省略扩展名。若角色或模型在生成时不可用，该 ID 变为空槽。武器和消耗品走原有拾取效果，其他可加载模型可作为剧情物品。拾取后道具隐藏，场景脚本可用 `SetSceneItem` 重新启用。

两个角色操作都由游戏系统在回调返回后执行。结算已完成的回合不能复活。若关卡要围绕复活安排多轮战斗，可以在第一次击杀回调中调用 `Reborn`，把 `GameOver` 留给最后一次击杀。

背景音乐遵循客户端的音量与启用设置。当前曲目可重复请求，播放从当前进度继续。同一帧有多个有效请求时，最后一次切换生效。脚本选择只影响执行它的客户端。

```c
OnStart()
{
    PlayMusic("music/s8.mp3");
    int player = GetChar("player");
    Print("备用武器", GetWeapon2(player), GetWeaponType2(player));

    int item = GetSceneItemId("D_BBox01");
    if (item >= 0)
    {
        Print(item, GetSceneItemName(item));
    }
}
```

## 跨脚本与事件

`ExecuteFunction` 的目标可以是脚本 ID、文件名或 `"scene"`、`"mission"`。例如
`ExecuteFunction(GetSceneScriptId(), "OpenDemoDoor")`。脚本 ID 整局有效，各脚本独立保存全局变量，嵌套调用共享入口回调的执行预算。

- `Item_OnPickUp(int item, int actor)` 在场景道具领取完成后触发。先运行原有的 `<道具名>_OnPickUp`，再运行场景和任务的通用回调。物品 ID 与 `GetSceneItemId` 一致。
- `OnCharacterKill(int killer, int victim)` 在离线战斗确认致命攻击后触发。场景回调先于任务回调。角色 ID 使用任务角色编号，玩家为 `0`。环境或自伤死亡沿用原有死亡事件。
- 在场景 PST 中调用 `SetSceneItem("D_DemoKey", "attribute", "script_pickup", 1)`，可以把可见、可交互的 DES 实例变成玩家触碰即拾取的剧情物品。领取后隐藏，脚本可显式重新启用。

## 小任务示例

[钥匙、门和守卫](../../examples/pst/key-door-guard/) 包含原创场景 PST、任务 PST 和守卫 PST。把本地原版游戏资源目录作为准备脚本的第一个参数。准备脚本复制一份原版资源，在副本的第一关放置钥匙和门。原版资源目录保持原样。

```sh
python3 examples/pst/key-door-guard/prepare.py /path/to/original-game artifacts/pst-key-door-demo-v3
cargo run --manifest-path meteor_rust/Cargo.toml -p meteor-app --bin meteor -- --original-dir artifacts/pst-key-door-demo-v3
```

进入单人战役第一关，沿出生朝向走到钥匙旁。拾取后弹出「石门」面板。点击「开门」后面板关闭，门解除碰撞，守卫出现，音乐切换到 `s8.mp3`。玩家脚边生成一把剑，拾取后从匕首自动切换到剑。第一次击败守卫时，他在原出生点满血复活，第二次击败后进入胜利结算。

### 离屏验证

用例运行钥匙守卫任务的自动化流程。面板和开门字幕分别取自按钮事件前后的真实状态，测试同时检查门的碰撞和守卫生成。另两个用例分别截取复活字幕和第二次击败守卫后的胜利结算。

```sh
METEOR_UI_CAPTURE=hud-pst-key-panel cargo test --manifest-path meteor_rust/Cargo.toml -p meteor-app --features original-assets --lib cinematic_frontend_gpu_capture -- --ignored --nocapture
METEOR_UI_CAPTURE=hud-pst-door-open cargo test --manifest-path meteor_rust/Cargo.toml -p meteor-app --features original-assets --lib cinematic_frontend_gpu_capture -- --ignored --nocapture
METEOR_UI_CAPTURE=hud-pst-key-door cargo test --manifest-path meteor_rust/Cargo.toml -p meteor-app --features original-assets --lib cinematic_frontend_gpu_capture -- --ignored --nocapture
METEOR_UI_CAPTURE=result-pst-key-door cargo test --manifest-path meteor_rust/Cargo.toml -p meteor-app --features original-assets --lib cinematic_frontend_gpu_capture -- --ignored --nocapture
```

启用 `original-assets` 前需准备原版资源和参考采集数据。离屏测试需要可用的 GPU、放在仓库根目录 `origin_res/` 的原版游戏资源，以及放在 `meteor_rust/crates/meteor-app/assets/fonts/ui.ttf` 的中文字体（可使用 Windows 发布包附带的字体）。面板和开门截图写入 `artifacts/pst-key-door-panel/`，复活字幕和胜利截图写入 `artifacts/pst-key-door-ui/`。截图尺寸为 1280×720。
