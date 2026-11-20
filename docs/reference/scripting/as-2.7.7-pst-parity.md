# AS 2.7.7 与 OpenMeteor PST 对照

依据用户提供的 AS 2.7.7《用户手册》、安装包 Api.h 和同包的 Startup、PScriptLib 与范例脚本。游戏 API 区有 148 条声明，去重后 143 个函数名。重载合并在同一行，参数差异单独标明。当前表中有 111 个已接入、15 个等价入口、16 个部分实现和 1 个待补齐。

「已接入」描述当前 PST 有相应入口。「等价入口」表示在 PST 中用另一种调用方式实现。「部分」和「待补齐」保留具体差异。表中的实现状态与设备验收状态分别记录。现有脚本接口、调用范围和示例见 [PST 扩展 API](../../guides/pst-extension-apis.md)。

## 游戏 API

| AS 声明 | 状态 | PST 对应及差异 |
| --- | --- | --- |
| `AddChatCallback` | 等价入口 | 自启动脚本注册 PST 函数名。收到 WS 聊天时提供消息文本。当前聊天投影无统一队伍标志，第二参数为 0。 |
| `AddMessageCallback` | 等价入口 | 自启动脚本注册 PST 函数名，收到其他脚本的 Print/Output。回调自身的输出不再次派发。 |
| `AddNPC` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `Call` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `CanInvokeGameAPI` | 等价入口 | 查询 PST 已登记的游戏 API 名。实际操作仍受模块、关卡和角色范围限制。 |
| `ChangeBehavior` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `CharAttack` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharAttackImm` | 部分 | 直接提交一次方向组合到角色战斗控制器。执行点是下一模拟步，保留 PST 战斗状态门。 |
| `CharBaoQi` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharCreateTempItem` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharDropTempItem` | 已接入 | 支持从角色位置抛出，以及显式坐标、水平朝向的重载。 |
| `CharDropVipItem` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharDropWeapon` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharGetBaseAttrib` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharGetBufferList` | 等价入口 | 返回资源 ID 数组。通过 ID 查询和移除增益。AS 的 BufferDetail 对象布局不参与 PST。 |
| `CharGetPosition` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharGetRotation` | 部分 | 读取角色偏航并组成向量。角色俯仰与滚转没有脚本投影。 |
| `CharGetRotationYaw` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharJump` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharJumpImm` | 部分 | 将跳跃直接提交到本地/NPC 跳跃入口，消费时间由模拟步管理。 |
| `CharLockTarget` | 部分 | 玩家走现有锁定输入，NPC 使用已选敌人。无敌人时不创建额外目标。 |
| `CharRemoveBuffer` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharSetBaseAttrib` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharSetPosition` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharSetRotation` | 部分 | 写角色偏航。俯仰和滚转尚无对应控制。 |
| `CharSetRotationYaw` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharShowPose` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `CharSwitchWeapon` | 已接入 | 支持动画与换武器许可开关，参见 PST 接口指南。 |
| `CloseFile` | 等价入口 | 提交暂存文本并释放句柄。写入失败时保留句柄供重试。 |
| `Command` | 部分 | 可用命令为 moveto、weapon、loadmych、pose、scene。其余旧暗令待逐个映射。 |
| `CreateEffect` | 部分 | 支持场景道具位置的原版特效。其他 AS 变体尚未逐个验证。 |
| `CreateSceneItem` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `DisableExecuteLine` | 已接入 | 本次进程持续禁用脚本与控制台的 PST 单行执行。 |
| `DisableWaypoints` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `Distance` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `Distanve` | 等价入口 | 使用 Distance。保留原 PST 对拼写错误的处理。 |
| `EnableKeyCommand` | 等价入口 | 控制反斜杠打开 Bevy 暗令台。 |
| `EnableKeyEsc` | 已接入 | 控制战斗中的 Esc 暂停，支持提示文本。 |
| `EnableWaypoints` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `EncryptScript` | 待补齐 | 尚无 OpenMeteor PST 源码加密发布格式。AS 密文和 Windows 工具不用于加载 PST。 |
| `EnterScene` | 部分 | 可进入 OpenMeteor 官方单人任务列表中的关卡编号。自定义任务需通过现有启动入口。 |
| `ExecuteFunction` | 已接入 | 自启动脚本可互相同步调用。单机中，场景、任务、NPC 与自启动脚本可以双向同步调用。嵌套调用共享入口的执行步数和调用深度预算。 |
| `ExecuteLine` | 已接入 | 在当前 PST 模块执行，使用脚本步数和调用深度限制。 |
| `ExitScene` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GameCallBack` | 部分 | 支持 mod 与 end。 |
| `GameOver` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetActivePose` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetAnger` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetAnyChar` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetAttack` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetChar` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetCharIdList` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetCharName` | 已接入 | 玩家返回 player，其余返回角色显示名。 |
| `GetDead` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetDefense` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetDisplayName` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetEnemy` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetGameTime` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetGameTimeFloat` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetGameVersionText` | 等价入口 | 返回 OpenMeteor 的版本。 |
| `GetGameWindowSize` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetGlobalTime` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetHP` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetKill` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetLeader` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetLevelScriptId` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetLevelScriptName` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetMapBoundingBoxMax` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetMapBoundingBoxMin` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetMapBoundingBoxSize` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetMaxHP` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetNpcScriptId` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetNpcScriptName` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetPosition` | 已接入 | 读取 SetTarget 对应槽，返回原始关卡坐标。 |
| `GetSceneItem` | 部分 | 支持数字 ID、名称与属性查询。可用属性以场景宿主为准。 |
| `GetSceneItemCount` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `GetSceneItemId` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `GetSceneItemModelName` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `GetSceneItemName` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `GetSceneItemPosition` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `GetSceneItemRotationYaw` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `GetSceneScriptId` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetSceneScriptName` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetScriptVarInt` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetScriptVarString` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `GetSelf` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetSharedVarInt` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetSharedVarString` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetSpeed` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetWeapon` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetWeapon2` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `GetWeaponType` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `GetWeaponType2` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `InputGameKey` | 部分 | 按按键名走游戏输入流。玩家有持续按键与事件，NPC 目前有攻击、跳跃、爆气和锁定。AS 整数键号转换待补。 |
| `IsAlive` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `IsAllowCharSwitchWeapon` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `IsPerforming` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `IsTwoPointsVisible` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `LoadImageFile` | 等价入口 | 返回可交给 UiAddImage 的资源路径，支持 PNG 或 JPEG。 |
| `MakeString` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `Misc` | 部分 | 已实现 gettime、transfer、randomseries、getseries。 |
| `NetEvent` | 已接入 | 现有场景事务同步。PST 的单机扩展命令没有因此获得在线权限。 |
| `OpenFile` | 等价入口 | 以 r/w/a 模式打开脚本数据，写入限存档槽。使用文本句柄与 PST 平台存档。 |
| `Output` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `Perform` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `Perofrm` | 等价入口 | 使用 Perform。保留原 PST 对拼写错误的处理。 |
| `PlayMusic` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `PlayerPerform` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `Print` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `PrintSystemText` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `Rand` | 已接入 | 一参数和双端点形式均有入口。未承诺重现 AS 随机种子序列。 |
| `ReadFile` | 等价入口 | 逐行读取十进制整数，ReadFileLine 读取文本。 |
| `Reborn` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `RebornAround` | 部分 | 当前在死亡位置复活。周围安全点选择尚未对齐。 |
| `RemoveNPC` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `Say` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `ServerKill` | 等价入口 | 单机强制死亡走角色死亡流程并产生原因 2。在线对局由原有服务器权威处理。 |
| `SetAggressPose` | 已接入 | 配置 NPC 挑衅动作的 Pose。 |
| `SetAllowCharSwitchWeapon` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `SetDisplayName` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `SetKeyCommandHint` | 已接入 | 配置启用、禁用、按下已锁定按键的提示。 |
| `SetKeyEscHint` | 已接入 | 同上，作用于战斗中的 Esc。 |
| `SetScene` | 部分 | 可提交场景参数，当前效果消费链主要覆盖原版雪景相关设置。 |
| `SetSceneItem` | 部分 | 支持现有关卡用到的属性、状态及动画。AS 继承的属性名全集未逐个验证。 |
| `SetSceneItemPosition` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `SetSceneItemRotationYaw` | 已接入 | 同名入口。见 [场景 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/scene_script_runtime.rs)。 |
| `SetSharedVarInt` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `SetSharedVarString` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `SetTarget` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `SetTeam` | 已接入 | 同名入口。见 [PST 会话](../../../meteor_rust/crates/meteor-scene/src/pst_game.rs)。 |
| `ShowChatText` | 部分 | 可隐藏聊天室聊天区文字。 |
| `ShowMouse` | 已接入 | 显示并释放光标，可传 0 恢复游戏光标管理。 |
| `StopPerform` | 已接入 | 同名入口。见 [任务 PST 宿主](../../../meteor_rust/crates/meteor-scene/src/mission_runtime.rs)。 |
| `UpdateAIAttrib` | 已接入 | 同名入口。见 [PST 运行时](../../../meteor_rust/crates/meteor-scene/src/pst_runtime.rs)。 |
| `WaveClone` | 已接入 | 复制波形资源配置和位置，持有独立播放句柄。 |
| `WaveDelete` | 已接入 | 释放句柄并停止播放。 |
| `WaveLoad` | 已接入 | 验证已安装 WAV 资源并关联句柄。 |
| `WaveNew` | 已接入 | 新建可配置 2D 或 3D 的音轨句柄。 |
| `WavePlay` | 已接入 | 播放指定音轨，可循环。 |
| `WaveSetPosition` | 已接入 | 用原始关卡坐标更新音轨位置。 |
| `WaveStop` | 已接入 | 停止音轨并保留其资源配置。 |
| `WorldRayCast` | 等价入口 | 命中返回向量，未命中返回 0。PST 不使用 AS 输出引用参数。 |
| `WriteFile` | 等价入口 | 写一行整数，WriteFileLine 写一行文本。 |

## 手册章节及随包能力

| 章节或能力 | OpenMeteor 对应 | 状态或实际差异 |
| --- | --- | --- |
| 安装、受支持的游戏版本 | OpenMeteor 原版目录读取、MPK 资源加载、桌面 Android WASM | AS 引擎的 EXE 补丁、1.07/1.08/9.xx 宿主二进制在约定范围之外。 |
| 场景、任务、NPC、全局脚本安装 | `snXX/snXX.pst`、`Level/snXX_.pst`、`Level/npc*.pst`、`PScriptLib/Startup/index.txt` | 已有 PST 加载路径。 |
| 布尔、无符号、浮点、字符串 | `true/false`、`bool/uint/float/double/string`、`FloatDivide` | 声明可用。数值仍用 PST Number，整数除法语义保留。无符号溢出与 AS 类型系统不等价。 |
| `if/for/while/switch` | PST 解析器、分支回归 | 已有对应语句。 |
| 数组、字典、字符串、向量和颜色 | `Array` 系列、`Map` 系列、`String` 系列、`Vector2/3/4`、`Color` | 函数式容器。AS 类、引用、方法、字段布局未迁移。 |
| 文件、序列化和存档 | 文本句柄与命名存档槽，`SaveVector` / `LoadVector` | 可做剧情存档与表格读取。二进制 File/Serializer 的逐方法能力待补。 |
| 资源访问权限 | 已安装资源的只读视图，本地写入命名存档槽 | 写范围限制在本地存档。与 AS 按模块统计累计 1 MiB 的规则还有差异。 |
| 游戏启动、关卡和场景加载、任务结束 | `OnStart`、`Scene_OnLoad`、`OnInit`、`Level_OnStart`、`Scene_OnInit`、`OnStop` | PST 有对应事件。全局事件通过游戏线程的队列投递。 |
| 道具 Load/Init/Touch/Attack/PickUp/Idle | 具名 PST 回调、场景与任务通用事件、全局事件 | 单机链路已接入，任务从 Item_OnInit 开始接收。 |
| NPC Init/Start/Update | `OnInit(id)`、`OnStart`、`NPC_OnStart(id)`、AI `OnUpdate` | NPC 的原 PST AI 节拍保留。按模拟帧执行可用任务与 NPC 的 `OnFrame`，场景用 `Scene_OnFrame`。 |
| 死亡、复活、击杀和临时道具拾取 | `Scene_OnCharacterEvent` 与各专用回调 | 单机脚本角色已接入，在线库存和死亡保持已有授权路径。 |
| 退出游戏 | 自启动模块 `OnStop` | 显式 AppExit 时触发。 |
| 场景和任务库，箱、桌、椅、坛子、宝藏 | 原版 PST 和场景道具宿主 | 常见场景机关已存在。提供 [PST 宝藏表](../../../examples/pst/entertainment-menu/treasure-table.pst)，读取 `model,x,y,z` CSV 生成拾取物。AS 表格的全部列与共享库其余函数未逐一移植。 |
| 娱乐菜单、Mod 信息、小键盘换武器 | PST 娱乐菜单示例，可用自启动模块和 Bevy 面板 | 示例已覆盖分页、换装、瞬移、读写档和跨脚本刷 NPC。AS 随包所有页面尚未移植。 |
| 图片、贴图缓存、色调、透明、UV | `LoadImageFile`、`UiAddImage`、Bevy 图片节点、纹理坐标裁切与翻转 | 面板内的图片能力已接入，独立精灵锚点、缓存目录和任意布局待补。 |
| 声音管理、轨道、空间音、语音 | Wave 属于关卡句柄，`SoundBindActor`、`Say`、`PlayMusic` | 可控制播放、循环与角色绑定。语音整目录缓存和自动匹配文本的随包模块待补。 |
| 动作队列、延时和跳过 | `DelayCall` 与 `CancelDelayedCalls` | 等价的延时和取消入口，连续活动的 AS 队列 ID 与跳过语义待补。 |
| 新增默认按键配置 | 项目 Settings/Keymap | 可读取 `UserData/user_default.cfg` 并在重置时覆盖键位，使用 OpenMeteor 的键位文本格式。 |
| 超大地图和人数/阵营上限 | OpenMeteor 场景与 AI | 未做 AS 所述极限场景的专项验收。 |
| 切换胜利动画及媒体重定向 | 原有结算视频和播放链 | 由脚本覆盖结算媒体的接口待补。 |
| 游戏内控制台、语法检查、保存时编译 | 反斜杠暗令台 `pst ...`、`pst-check` 命令 | 可离线检查语法。AS VSCode 扩展和 ScriptCompiler 专用格式在约定范围之外。 |
| 自动热重载 | 桌面原版目录下的场景、任务、NPC 和自启动脚本监听，保留兼容全局变量 | 自启动模块成功重载时执行 OnStop/OnStart。关卡模块替换程序，保留已生成实体。 |
| ImGui Widgets、第三方脚本 DLL | Bevy 面板与 Rust 宿主 | 旧 ImGui 调用规约及第三方 DLL ABI 在约定范围之外。 |

## 版本记录中的专门事项

| 手册版本 | OpenMeteor 对应与验证边界 |
| --- | --- |
| 2.7.7 | 默认键位文件缺失时用内置键位。已有设置重置路径仍会保存配置。 |
| 2.7.6 | 支持图片 UV 和颜色、默认键位文件、跨脚本共享变量与文本文件句柄。 |
| 2.7.5 | 威震八方仍使用原有场景事件与传送。此轮未录制实机小房间死亡重演。 |
| 2.7.4 | Bevy 负责 PNG/JPEG 解码。手册未附触发当年 PNG 问题的样本。 |
| 2.7.3 | Visual Studio 2022 的 AS DLL 工具集在约定范围之外。 |
| 2.7.2 | NPC 移除、角色名和两点可见性有对应入口。旧宿主崩溃样本不可用于证明 Rust 生命周期。 |
| 2.7.0 | 临时道具、四类战斗事件、NPC 变量覆写和方向招式已列入上表。 |
| 2.6.2 | 增益读取来自当前状态组件。与 AS 的缓冲对象结构差异见 `CharGetBufferList`。 |
| 2.6.1 | 角色名覆盖保存在单机关卡，进入新关卡时清理。此次未做菜单往返的实机验收。 |
| 2.6.0 | 角色列表、Pose、阵营、名字、存活和基础属性已有对应入口。 |
| 2.5.0 | 爆气交给现有战斗状态机。未用乾坤刀单独重演该版本的崩溃条件。 |
| 2.4.0 | 击杀、死亡、复活和图片色调已有入口。脚本替换胜利动画还缺媒体重定向。 |
| 2.3.0 | 可遍历增益 ID 并按 ID 移除。 |
| 2.2.0 | 单人切关和退出到菜单已有入口。 |
| 2.1.0 | 一参数与双端点的随机数入口保留各自契约。 |
| 2.0.0 | 场景道具位置、角色偏航、地图范围和可读宝藏表已接入。超大地图边界待专项实测。 |
| 1.9.0 | 属性、爆气、跳跃和版本查询已接入。控制台用 OpenMeteor 的反斜杠端口。 |
| 1.8.0 | 提供按下沿和持续按住、角色位置和偏航、换武器与动作。完整三轴姿态仍有差异。 |
| 1.7.0 | 全局时间、实时按键、窗口尺寸、图片句柄、热重载已有对应入口。加密与批量图片缓存待补。 |
| 1.6.0 | 窗口尺寸、图片裁切/翻转与定时函数已有入口。独立精灵与活动队列状态仍有差异。 |
| 1.5.0 | 脚本变量、任务时钟、共享变量、聊天显示和空间音效见接口表。目录扫描及自动剧情语音绑定待补。 |
| 1.4.0 | 按键提示、换武器许可、丢物、存档与 NPC 招式已有入口。 |
| 1.3.1 | 初次加载的任务语法错误进入关卡错误页。热重载阶段保留原程序。 |
| 1.3.0 | 控制台、背景音乐、按键锁、侧屏文字和挑衅动作已有入口。系统输入法切换未经实现。 |
| 1.2.0 | 更新 AI 属性有既有回归。图片在 Bevy 面板内排列，AS 示例的任意精灵层叠待补。 |
| 1.1.0 | 目标坐标、副武器、角色查询、结算、命令和字符事件见接口表。手册列出的旧宿主版本不用于运行 PST。 |
