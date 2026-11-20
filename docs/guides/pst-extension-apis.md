# PST 扩展 API

[AS 2.7.7 手册与 Api.h 逐项对照](../reference/scripting/as-2.7.7-pst-parity.md)

这些接口可以直接写在 OpenMeteor 的 `.pst` 回调里。接口名参考 AS 脚本引擎 2.7.7，返回值按下面的约定使用。

| API | 脚本 | 返回值 |
| --- | --- | --- |
| `GetSceneItemCount()` | 场景、任务、NPC | 道具槽数。可遍历 `0 <= id < count`。DES 原有序号保留，动态道具追加在末尾。 |
| `GetSceneItemId(name)` | 场景、任务、NPC | 道具的场景 ID，找不到返回 `-1`。名称查找忽略大小写，并沿用 PST 道具名的补零规则。 |
| `GetSceneItemName(id)` | 场景、任务、NPC | 道具名称。无效或不可用的 ID 返回空字符串。 |
| `GetWeapon2(characterId)` | 单机任务、NPC | 角色当前备用槽的 `Meteor.res` 武器道具 ID。空槽返回 `0`，找不到角色返回 `-1`。 |
| `GetWeaponType2(characterId)` | 单机任务、NPC | 备用武器类型，例如剑为 `6`，忍刀为 `11`。空槽返回 `0`，找不到角色返回 `-1`。 |
| `PlayMusic(path)` | 客户端场景、单机任务、NPC | 切换循环背景音乐。接受 `music/` 下本客户端可读的 MP3，接受请求返回 `1`，路径无效或文件缺失返回 `0`。 |
| `ExecuteFunction(target, function, ...)` | 自启动、场景、单机任务、NPC | 同步调用目标脚本中的函数。调用完成返回 `1`，目标脚本或函数不存在返回 `0`。运行错误会中止当前调用并记录诊断。 |
| `GetSceneScriptId()`、`GetLevelScriptId()`、`GetNpcScriptId(character)` | 场景、单机任务、NPC | 返回本关卡内可供 `ExecuteFunction` 使用的脚本 ID。不存在时返回 `-1`。 |
| `GetStartupScriptId(name)` | 自启动、单机的场景、任务、NPC | 通过文件名取得自启动脚本的 ID，可以传给 `ExecuteFunction`。不存在时返回 `-1`。 |
| `CreateSceneItem(name, kind, model[, actorIdOrPosition])` | 单机 | `"pickup"` 生成拾取物，`"prop"` 或空字符串生成带碰撞的摆件。最后一项是角色 ID 或三维坐标。省略时取玩家脚边。返回道具 ID，失败返回 `-1`。 |
| `CharSwitchWeapon(actorId[, force])` | 单机的场景、任务、NPC | 请求切换至备用槽，实际换武器发生在动画提交点。传入 `0` 时遵循 `SetAllowCharSwitchWeapon` 设置。 |
| `Reborn(actorId)` | 单机的场景、任务、NPC | 请求复活任务名单中已死亡的角色，接受返回 `1`，否则返回 `0`。角色恢复满血、回原出生点，角色编号和 NPC 脚本变量保留，出生保护重新计时。 |
| `UiCreatePanel(id, title)` | 单机的场景、任务、NPC | 创建隐藏面板。 |
| `UiAddText(id, text)` | 单机的场景、任务、NPC | 向面板追加一段文字。 |
| `UiAddButton(id, buttonId, label)` | 单机的场景、任务、NPC | 添加按钮，`buttonId` 在该面板内唯一。 |
| `UiAddImage(id, path)` | 单机 | 从资源目录 `Image/PstUi/` 添加 PNG。每个面板最多四张，每张最大 2048 × 2048。 |
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

`ExecuteFunction` 接受脚本 ID、文件名或 `"scene"`、`"mission"`。自启动脚本在主菜单即可互调，进入单机关卡后可以与场景、任务、NPC 脚本双向调用。比如场景脚本可以写 `ExecuteFunction("library.pst", "OnDoorOpened")`，并在调用返回后读取该函数更新的变量。关卡脚本的 ID 在本局内有效，自启动脚本的 ID 在脚本加载期间有效。各脚本独立保存全局变量，嵌套调用共享入口回调的执行预算。

- `Item_OnPickUp(int item, int actor)` 在场景道具领取完成后触发。先运行原有的 `<道具名>_OnPickUp`，再运行场景和任务的通用回调。物品 ID 与 `GetSceneItemId` 一致。
- `OnCharacterKill(int killer, int victim)` 在离线战斗确认致命攻击后触发。场景回调先于任务回调。角色 ID 使用任务角色编号，玩家为 `0`。环境或自伤死亡沿用原有死亡事件。
- `OnCharacterDeath(int actor, int reason)` 在死亡状态提交后触发。离线原因码 `1` 是被其他角色击杀，`3` 是环境或自身效果致死。复活提交后触发 `OnCharacterReborn(int actor)`。
- `Item_OnLoad/Init/Idle(int item)`、`Item_OnTouch(int item, int actor)`、`Item_OnAttack(int item, int actor, int damage)` 是通用道具回调。按名称定义的 `<道具名>_On…` 仍在通用回调之前执行。任务脚本从 `Item_OnInit` 开始接收道具事件。
- 任务脚本依次收到 `OnInit` 与 `OnStart`，离局时收到 `OnStop`。场景脚本离局时收到 `Scene_OnClose` 与 `Scene_OnStop`。
- 在场景 PST 中调用 `SetSceneItem("D_DemoKey", "attribute", "script_pickup", 1)`，可以把可见、可交互的 DES 实例变成玩家触碰即拾取的剧情物品。领取后隐藏，脚本可显式重新启用。

## 小任务示例

[钥匙、门和守卫](../../examples/pst/key-door-guard/) 包含原创场景 PST、任务 PST 和守卫 PST。准备脚本复制一份原版资源，在副本的第一关放置钥匙和门。原版资源目录保持原样。

```sh
python3 examples/pst/key-door-guard/prepare.py origin_res artifacts/pst-key-door-demo-v3
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

需要可用的 GPU。面板和开门截图写入 `artifacts/pst-key-door-panel/`，复活字幕和胜利截图写入 `artifacts/pst-key-door-ui/`。截图尺寸为 1280×720。

## 场景、道具与角色控制

三维坐标使用关卡原始坐标，Z 轴朝上。传入 `Vector3(x, y, z)`，用 `value[0]`、`value[1]`、`value[2]` 读取分量。水平朝向用度数。查询后修改道具坐标的脚本可以在同一回调里读到新值，渲染和碰撞随后更新。

| 能力 | 接口与用法 |
| --- | --- |
| 场景道具资料 | `GetSceneItemModelName`、`GetSceneItemPosition`、`GetSceneItemRotationYaw` 接道具 ID，也接名称。单机可用 `SetSceneItemPosition(id, pos)` 和 `SetSceneItemRotationYaw(id, degrees)` 移动道具。 |
| 临时道具 | `CharCreateTempItem(actor, type, resId, pos)`、`CharDropTempItem(actor, type, resId[, pos, yaw])` 使用 `Meteor.res` 的类别与 ID。投掷物落地后可拾取，领取时触发 `OnTempItemPickUp(type, resId, actor)`。 |
| 空间查询 | `GetPosition(targetSlot)` 配合 `SetTarget`，`GetMapBoundingBoxMin/Max/Size()` 返回关卡包围盒，`IsTwoPointsVisible(a, b)` 和 `WorldRayCast(a, b)` 查询关卡碰撞。后者命中时返回坐标，未命中时返回 `0`。 |
| 角色资料 | `GetCharIdList()` 返回角色 ID 数组，可用 `ArraySize` 遍历。`IsAlive`、`GetActivePose`、`GetKill`、`GetDead`、`GetDisplayName`、`GetActorTeam` 和 `GetWeapon/GetWeaponType` 都接角色 ID。 |
| 角色位置和装备 | `CharGetPosition(id)`、`CharSetPosition(id, pos)`、`CharGetRotationYaw(id)`、`CharSetRotationYaw(id, degrees)`。`CharEquipWeapon(id, resWeaponId)` 直接装备指定武器，`CharSetModel(id, modelId)` 选择已安装的角色模型，`CharDropWeapon(id)` 丢弃主武器并接替为备用武器，`CharDropVipItem(id)` 让当前携带者放下镖物。 |
| 属性与行为 | `SetTeam`、`SetDisplayName`、`CharGetBaseAttrib`、`CharSetBaseAttrib`、`CharGetBufferList`、`CharRemoveBuffer`。属性编号 `0–4` 依次为最大血量、当前血量、攻击、防御、速度。增益列表元素为资源 ID。 |
| 招式与复活 | `CharAttack(id, sequence)` 的方向用 `f/b/l/r`，单 A 用 `n`，跳跃用 `j`，逗号或句点等待 0.2 秒。`CharJump`、`CharBaoQi` 与 `CharShowPose` 可单独触发。`RebornAt(id, pos)` 在指定位置复活，之后的正常出生点仍取原配置。 |
| NPC 初始化 | `AddNPC("npc01_01", "Name=练习守卫;HP=600;")` 在该 NPC 的 `OnInit` 返回后覆盖已有变量，然后按新值创建实体。 |

## 自启动、跨关状态与音频

在资源目录 `PScriptLib/Startup/index.txt` 中逐行列出同目录下的 PST 文件名。可写以 `#` 开头的注释。模块从主菜单开始运行，跨单机关卡保留全局变量。可定义 `OnStart`、`OnUpdate`、`OnStop`，以及单机关卡的 `Level_OnStart`、`Level_OnStop`、`NPC_OnStart(id)` 和场景、道具、角色事件。自启动脚本建的面板把按钮事件交回创建该面板的模块。关卡动作与临时道具由本机单机模拟提交。

- `GetSceneScriptName()`、`GetLevelScriptName()`、`GetNpcScriptName(id)` 返回脚本名。`GetScriptVarInt/String(target, name)` 读已链接脚本的变量。
- `SetSharedVarInt/String(key, value)` 和 `GetSharedVarInt/String(key)` 在同一次运行中的脚本之间共享数据。`SaveString/LoadString` 和 `SaveVector/LoadVector` 读写有名称的本地存档槽。键由英文字母、数字、下划线组成，桌面和 Android 保存在 `UserData/Saves`，WASM 使用浏览器本地存储。
- `DelayCall(seconds, "Function", ...args)` 安排当前脚本稍后调用自身函数。多次调用可编排连续的剧情步骤，`CancelDelayedCalls([function])` 取消尚未到期的调用。`EnterScene(number)` 进入指定官方单人任务，`ExitScene()` 返回菜单。
- `IsKeyPressed("J")` 检查本帧按下，`IsKeyDown("J")` 检查持续按住。名称也可用 `NUMPAD1`、`ESCAPE` 和 `MOUSELEFT`。
- `SoundPlay(track, path[, loop])` 播放游戏资产中的 WAV。用 `SoundSetPosition(track, pos)` 设置位置，`SoundBindActor(track, id)` 跟随角色，`SoundUnbind(track)` 保留当前位置，`SoundStop(track)` 停止。可搭配 `Say` 播放剧情语音。

## 娱乐菜单示例

从工作区根目录运行下面的命令，进入单人战役「钟乳洞」。按 **J** 打开菜单，保存当前位置，走几步再读档。第二页可换剑和召唤练习守卫，第三页可抛出灵芝并播放跟随玩家的提示音。数字键盘 **1** 直接换成匕首。

```sh
python3 examples/pst/entertainment-menu/prepare.py origin_res artifacts/pst-entertainment-menu
cargo run --manifest-path meteor_rust/Cargo.toml -p meteor-app --bin meteor -- --original-dir artifacts/pst-entertainment-menu
```

## AS 手册功能的 PST 用法

- 单机关卡按 **反斜杠** 打开暗令台。输入 `pst GetHP(0)` 查看玩家血量，或输入 `pst Print("HP", GetHP(0))` 查看脚本日志。`DisableExecuteLine()` 会锁定本次进程的 PST 执行入口。
- 在桌面使用 `--original-dir` 启动时，保存已有场景、任务、NPC 或自启动脚本会触发重载。语法错误时继续使用上一次可运行的程序。已有实体和兼容的脚本变量保留。自启动模块按顺序执行旧 `OnStop` 和新 `OnStart`。
- `OpenFile("UserData/Saves/example.sav", "w")` 返回整数句柄。用 `WriteFile` 写整数，或用 `WriteFileLine` 写文本，再调用 `CloseFile` 提交。`"r"` 读取，`"a"` 追加。
- 用 `WaveNew(1)` 建立空间音轨，调用 `WaveLoad`、`WaveSetPosition` 和 `WavePlay`。用 `WaveClone` 复制配置，`WaveStop` 停止，`WaveDelete` 释放。
- `UiAddImage(panel, path, Vector4(1,0,0,1), Color(1,1,1,0.7))` 水平翻转图片并设为七成不透明度。UV 参数顺序为 `u0, v0, u1, v1`。
- 关卡可包含 [宝藏表脚本](../../examples/pst/entertainment-menu/treasure-table.pst)，调用 `SpawnTreasureTable("PScriptLib/Data/treasure.csv")`。每行填写模型名和三个原始关卡坐标，例如 `item02,30,20,10`。以 `#` 起始的行可作为注释。

在工作区根目录用以下命令检查脚本语法。

```sh
cargo run --manifest-path meteor_rust/Cargo.toml -p meteor-scene --bin pst-check -- examples/pst/entertainment-menu/menu.pst
```
