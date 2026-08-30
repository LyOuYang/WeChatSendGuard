# 微信输入法（IME 组合）探测测试结果

> 文档用途：记录 2026 年在微信 + 微信输入法/微软拼音环境下对「输入法组合中按下 Enter」
> 能否被外部程序可靠识别这一问题的全部实测结论，供后期正式适配（恢复输入法组合
> Enter 放行）时直接引用。本文档**不包含任何产品代码改动**；探测用临时脚本位于
> `tools/ime-probe/`（仅测试期使用，不属于产品，后续会删除），本文档保留完整的方式
> 描述、实测数据与结论，不依赖该目录存活。

## 1. 结论速览

| 结论 | 说明 |
| --- | --- |
| ✅ 可用信号 | 微信输入法（WeType）组合中，候选窗 `wetype_candidate` 窗口**可见**；用 `FindWindowW + IsWindowVisible` 可精确判定，已多次实测复现 |
| ✅ 信号归属 | 候选窗属于 WeType 自己的进程（`wetype_renderer.exe`），类名 `wetype.flutter.setting`、标题 `wetype_candidate`，不受微信进程影响 |
| ❌ Imm32 路线 | `ImmGetContext` 对微信窗口恒为 NULL（微信是 TSF 应用，关闭了 IMM 兼容层）。旧实现（2026 年前 `ImmGetCompositionStringW(GCS_COMPSTR)`）在微信里**从不能工作**，其删除（提交 `d9e110f`）合理 |
| ❌ UIA 路线 | 微信编辑框只有 `Value`/`Invoke`/`TableItem` 三个 UIA 模式，**没有** `TextPattern`(10005) / `TextPattern2`(10024) / `TextEdit`(10032)，组合 API 不可用 |
| ❌ 事件路线 | `EVENT_OBJECT_IME_*`（0x800A~C）洪流噪声极大且大量 `hwnd=0`；微信窗口在组合期间**几乎不产生**这些事件（记事本/WebView 会产生，应用差异大），不可作信号 |
| ❌ 微软拼音 | 组合为**内联**（候选画在应用表面），无任何窗口/事件/API 可观测；"微软拼音组合确认 Enter"与"英文直打后 Enter 发送"在全部可观测通道上**逐字节不可区分** |
| ⚠️ 待验证 | UIA 透视 TextInputHost 子树查微软拼音候选元素（v6 探测未实施，见 §7） |

## 2. 测试环境

| 项 | 值 |
| --- | --- |
| 操作系统 | Windows 10 Pro（注册表 25H2 / build 26200.9168） |
| 微信 | `C:\Program Files\Tencent\Weixin\Weixin.exe`，版本 **4.1.13.12** |
| 微信输入法 | WeType **2.1.3.18**（进程：`wetype_renderer` / `wetype_server` / `wetype_service` / `wetype_update`） |
| 微软拼音 | Windows 自带微软拼音（系统输入法切换） |
| 守卫版本 | VERSION = 1.2.0（当前发布版，**不含** IME 放行逻辑，`d9e110f` 已删除 `ime_composing` 通路） |
| 测试时间 | 21:14–21:42（连续 5 轮探测） |
| 测试方式 | 人工按协议操作 + 低层钩子探针自动记录 |

## 3. 探测方式（方法描述，逐轮递进）

探测手段为「低层键盘钩子（WH_KEYBOARD_LL）+ 各类状态通道的快照记录」。共 5 轮，
每轮在 Enter 按下瞬间（及 v5 轮的周期快照）记录以下通道，逐步收窄信号：

| 轮次 | 方式 | 结论 |
| --- | --- | --- |
| 1 | LL 键盘钩子 + Imm32 双上下文快照（前台窗口 / GetGUIThreadInfo 焦点窗口，`ImmGetOpenStatus` / `GCS_COMPSTR` 字符数 / 候选列表数）+ 订阅 `EVENT_OBJECT_IME_SHOW/HIDE/CHANGE`（0x800A~C）跨进程事件 | Imm32 在微信恒 `noHimc`；事件存在**全局洪流**且大量 `hwnd=0`，噪声不可直接用 |
| 2 | 在 1 的基础上给事件窗口补进程归属，并给 Enter 补编辑框 UIA 快照（焦点元素类型、支持的 pattern id 列表、ValuePattern/TextPattern 文本长度与尾部形态）与最近按键序列 | 微信编辑框 UIA 只暴露 `Edit` + `[10000 Invoke, 10002 Value, 10014 TableItem]`，无 TextPattern/TextPattern2；事件洪流多来自守卫自身与 WebView 等常驻窗口；`wetype_candidate` 窗口浮出水面 |
| 3 | 在 2 的基础上，Enter 时刻枚举所有可见顶层窗口（按进程/类名/标题筛 IME 相关）并直查 `FindWindowW("wetype.flutter.setting","wetype_candidate")` 可见性 | **微信输入法组合中 Enter：候选窗 `vis:1` 且编辑框 value 为空（浮动组合）**；对照组 `vis:0`；无 IME 候选窗时 `wetype_candidate` 常驻但隐藏 |
| 4 | 在 3 的基础上，补 TextInputHost 全窗口、XAML/Composition/InputSite 类弹窗、前台窗口（微信）子窗口枚举 | 微软拼音组合无任何可观测窗口；每个进程都有常驻隐藏的 `IME "Default IME"` / `MSCTFIME UI` 窗口（与组合无关，必须排除）；微信子窗口仅自身渲染窗口 |
| 5 | 改为每 2 秒周期快照（wetype 候选窗可见性 + TextInputHost 可见子窗口 + 新出现的顶层窗口 diff），配合「候选可见停住 3 秒」协议 | 微信输入法组合全程 `wetype vis:1`（连续快照 ≥4s）；微软拼音组合全程无任何新窗口/子窗口 |

记录方式：F9 打阶段标记、F10 退出，日志逐行落盘（UTF-8）。环境要求：PowerShell 5.1
STA（WinForms 消息泵 + 钩子回调），脚本需带 BOM（PS 5.1 无 BOM 的 UTF-8 中文会导致
解析失败）。

## 4. 测试协议（每轮相同，F9 打标）

1. 基线：记事本，英文状态按 Enter。
2. 微信输入法组合中按 Enter（记事本 + 微信聊天框各一次）。
3. 微软拼音组合中按 Enter（微信聊天框）。
4. 对照组：微信无输入直接按 Enter。
5. （v5 轮）真实拼音 + 候选可见后停住 3 秒再 Enter，配合 2 秒自动快照。

## 5. 各通道实测数据

### 5.1 Imm32（旧方案所在通道）—— 不可用

微信所有窗口（含焦点窗口）、WebView2、Win11 新记事本均无输入上下文：

```
21:15:58.708 ENTER-DOWN ... fg[hwnd=0x507CA class="Qt51514QWindowIcon" title="微信" pid=21408]
  focus[...同一窗口...] imeFg=[noHimc] imeFocus=[noHimc]
```

- 微信是 Qt 窗口（`Qt51514QWindowIcon`，4.x 版本），走 TSF 直接输入，不启用 IMM 兼容。
- 结论：旧实现（`3aff0a3`~`5d41d25` 的 `is_ime_composing` + `GCS_COMPSTR`）在微信内
  永远返回「未组合」；`d9e110f` 删除该逻辑是对的。**恢复适配时不得复用它，必须换信号。**

### 5.2 UIA —— 编辑框可读文本，但无组合 API

微信编辑框元素（Enter 时刻）：

```
uiaType=Edit patterns=[10000,10002,10014]
```

| Pattern id | 含义 | 可用性 |
| --- | --- | --- |
| 10000 | Invoke | 无关 |
| 10002 | Value | ✅ 可读编辑框全文（守卫草稿预览同源） |
| 10014 | TableItem | 无关 |
| 10015 | TextPattern | ❌ 缺失 |
| 10024 | TextPattern2（`GetActiveComposition`） | ❌ 缺失 |
| 10032 | TextEdit | ❌ 缺失 |

- 无法通过 `GetActiveComposition` 判断组合；Value 文本在微信输入法组合时为空
  （浮动组合），微软拼音组合时为拼音串（内联组合）。

### 5.3 WinEvent IME 事件 —— 噪声，不可用作主信号

- 全局存在持续洪流（`IME-HIDE hwnd=0` 海量、各进程常驻窗口周期性事件）。
- **微信窗口在组合期间几乎不产生 IME 事件**（记事本中微软拼音会发 `IME-SHOW`，
  微信里不发——应用差异大，无通用性）。
- 字母逐键期间微信窗口偶发 `IME-HIDE` 串（微软拼音内联时可见），但焦点切换等
  普通场景也会触发同样事件，无法区分真假。
- 注意：守卫**自身**窗口（class `Window Class`，标题 `WeChatSendGuard`）也持续产生
  `IME-CHANGE`，后续适配若做事件分析必须排除自身进程。

### 5.4 关键发现：微信输入法候选窗可见性 = 组合状态 ✅

窗口特征（多轮复现）：

```
FindWindowW("wetype.flutter.setting", "wetype_candidate")
  → 非空；属主进程 wetype_renderer（WeType 安装目录）
IsWindowVisible(hwnd) → 组合中 = TRUE，非组合 = FALSE
```

实测摘录（Enter 时刻）：

```
# 微信输入法组合中按 Enter（候选可见）——
21:31:40.424 ENTER-DOWN ... value:empty
  wetype_candidate=present vis:1 hwnd=0x200C4
  imeWins[wetype.flutter.setting(wetype_renderer):"wetype_candidate" | ...]

# 同一轮对照组（无输入 Enter / 微软拼音内联）——
21:37:30.205 ENTER-DOWN ... value:len:11 ascii:11 cjk:0 other:0
  wetype_candidate=present vis:0 hwnd=0x200C4
```

v5 连续快照（候选保持期间两个快照均可见）：

```
21:40:53.763 wetype=present vis:1 TIH-children[(none)] newWins[(none)]
21:40:55.763 wetype=present vis:1 TIH-children[(none)] newWins[(none)]
```

要点：

- 候选窗**自输入法启动即常驻**（隐藏），可见性精确对应组合/候选状态；组合确认后
  随即隐藏。
- 判定只花两个 Win32 调用（`FindWindowW` + `IsWindowVisible`），可在钩子回调内
  对 Enter key-down **同步**查询，不存在"标志位过期导致误放行"的竞态。
- 微信输入法其它常驻窗口（同样**必须排除**，均常驻且隐藏，属进程
  `wetype_renderer`/`wetype_update`）：`StatusBarWnd`(wetype.statusbar.window)、
  `语音输入`、`设置`、`微信输入法-关于`、`wetype.update.util.window`、
  `WeTypeUpdateEventDispatchWnd`、`MSCTFIME UI`、`IME "Default IME"`。
  后两类（IME/MSCTFIME UI）在**每个进程**都存在且常驻隐藏，与组合无关。

### 5.5 微软拼音 —— 无可观测信号（客观边界）

- 组合为内联：Value 文本 = 拼音串（`value:len:11 ascii:11` 等实测行）。
- v4/v5 枚举：无新顶层窗口、TextInputHost 无可见子窗口、无 XAML/Composition 弹窗、
  微信窗口无 IME 事件、无 IMM 上下文。
- 推论：`"微软拼音内联组合 + Enter 确认"` 与 `"英文直打 + Enter 发送"` 在物理按键、
  进程窗口、编辑框文本、系统事件**全部通道上不可区分**；组合状态只存在于微信进程
  内的 TSF 上下文，外部无 API 可读。除非注入（见 §6.4），否则对这类内联输入法
  不存在完美检测。

## 6. 对后期适配的指导

### 6.1 推荐方案（主场景：微信输入法）

1. `platform-windows` 新增 `ime.rs`：`is_ime_composition_visible()` ——
   `FindWindowW("wetype.flutter.setting","wetype_candidate")` 非空且
   `IsWindowVisible` 为真。调用方限定为**键盘钩子回调中 Enter key-down 分支**
   （微信输入法组合 Enter 的场景），避免全局周期查询引入过期窗口。
2. 恢复旧通路（参考 `git show d9e110f` 的删除面，位置可直接还原）：
   `KeyboardStroke.ime_composing` → `PhysicalEnter.ime_composing` →
   `handle_physical_enter` 早退放行条件追加 `|| enter.ime_composing`；
   审计轨迹建议单独记 `pass-through-ime-composing`，便于事后核对。
3. 扩展点：把「已知 IME 候选窗模式表」（类名/标题/进程名前缀）做成小函数 +
   表格常量，微信输入法已入表（`wetype.flutter.setting` + `wetype_candidate` +
   `wetype_renderer`）；搜狗、QQ 拼音等自带候选窗的输入法按相同格式补表后即可
   覆盖（未实测，需按 §8 流程验证）。
4. 单元/集成测试：guard-service 增加 `ime_composing=true` 时受保护会话 Enter
   直接 `PassThrough` 的用例；win 侧保持"仅纯数据分类"可测。

### 6.2 明确不推荐

- **键盘突发 + 编辑框内容对照启发式**（"Enter 前 ~1s 内敲过字母且编辑框尾部=这些
  字母 → 放行"）：与英文直打发送不可区分，等于在守卫最该拦截的场景漏拦，
  违反守卫核心承诺，**禁止**。
- **基于 IME 事件洪流的状态机**：微信窗口组合期间事件几乎为零（5.3），无法判真；
  且守卫自身窗口也在发事件。
- **恢复 Imm32 GCS_COMPSTR**：微信内恒 noHimc（5.1），无效。

### 6.3 方向性备注（待立项评审）

- **v6 待验证**：组合期间遍历 TextInputHost 的 UIA 子树，检查微软拼音候选条是否以
  UIA 元素暴露（TSF UI element 不保证有 UIA peer，预期成功率中低；若成功即可用
  非注入轮询覆盖内联输入法）。
- **DLL 注入级全局消息钩子**（WH_GETMESSAGE，直接观测 `WM_IME_STARTCOMPOSITION/
  ENDCOMPOSITION`）：技术上的唯一全解（覆盖所有输入法），但需把 DLL 注入微信进程，
  违背本项目"绝不向微信注入"的信任原则并有杀软/兼容风险，须单独安全评审后决定。
- **产品侧缓解**：向用户默认推荐微信输入法（已覆盖），文档中明确内联输入法的
  限制与替代路径。

### 6.4 风险与边界

- 微信输入法升级后窗口类名/标题可能变化 → 回归时须重新核对（§8）。
- 本数据基于微信 4.1.13.12 + WeType 2.1.3.18 + build 26200；未来版本需复测。
- 钩子内同步查询候选窗为 µs 级调用，对微信输入性能无影响（测量中未观察到卡顿）。
- 守卫自身窗口的 IME 事件（5.3）在做任何事件侧分析时都必须按 pid 排除。

## 7. 待验证方向

| 方向 | 成本 | 预期 | 状态 |
| --- | --- | --- | --- |
| wetype_candidate 可见性（推荐信号） | 已实测 | 高 | ✅ 完成 |
| UIA 透视 TextInputHost（微软拼音） | v6 小 demo | 中低 | ⏳ 未实施 |
| 通用"可见 IME 类顶层窗口"规则 | 需防误报（explorer 输入切换浮层等） | 中 | ⏳ 未实施 |
| 搜狗/QQ 拼音候选窗格式摸底 | 各一轮探测 | 中 | ⏳ 未实施 |
| DLL 注入消息钩子全解 | 大、需安全评审 | 高（确定性） | ⏳ 未实施 |

## 8. 适配后的手动验证清单（候选条目，可并入 `manual-wechat-validation.md`）

前置：记录 Windows / 微信 / 微信输入法版本，专用测试会话。

1. 微信输入法：受保护会话输入拼音、候选可见，按 Enter → **不得弹出守护确认窗**，
   组合正常确认进编辑框。
2. 接上一步再按 Enter → 正常进入守护确认流程（第二次 Enter 才是发送语义）。
3. 对照组：编辑框无输入（或直接英文短句）按 Enter → 正常进入守护确认流程。
4. 切换输入法（微软拼音等）→ 回归第 1~3 条，确认无回归、无卡顿。
5. 输入法升级、微信升级、系统语言包切换后复测第 1~3 条。
6. 长时间值班（≥30 分钟闲置）后复测第 1、3 条，确认候选窗探测无泄漏/误杀。

## 附录：关键原始日志摘录

```
# v2 — 微信窗口 Imm32 全灭（任何输入法、任何状态下）：
21:15:58.708 ENTER-DOWN vk=0x0D scan=0x1C flags=0x00 extra=0 fg[hwnd=0x507CA class="Qt51514QWindowIcon" title="微信" pid=21408] focus[...同...] imeFg=[noHimc] imeFocus=[noHimc]

# v3 — 微信输入法组合确认 Enter（信号=候选窗可见 + 编辑框为空）：
21:31:40.424 ENTER-DOWN ... lastKeys="HW121CEUISAFASDF" lastCharMs=1175
  uiaType=Edit patterns=[10000,10002,10014] value:empty
  wetype_candidate=present vis:1 hwnd=0x200C4
  imeWins[wetype.flutter.setting(wetype_renderer):"wetype_candidate" | Windows.UI.Core.CoreWindow(TextInputHost):"Windows 输入体验"]

# v3 — 微软拼音内联组合确认 Enter（候选窗不可见、value=拼音）：
21:31:51.581 ENTER-DOWN ... value:len:7 tail(7) ascii:7 cjk:0 other:0
  wetype_candidate=present vis:0 hwnd=0x200C4

# v4 — 对照：微信无输入 Enter（候选窗隐藏）：
21:37:39.139 ENTER-DOWN ... value:empty wetype=present vis:0 hwnd=0x200C4

# v5 — 微信输入法候选保持 4 秒（2 秒间隔连续两个快照可见）：
21:40:53.763 wetype=present vis:1 TIH-children[(none)] newWins[(none)] fg=...Weixin...
21:40:55.763 wetype=present vis:1 TIH-children[(none)] newWins[(none)] fg=...Weixin...
```