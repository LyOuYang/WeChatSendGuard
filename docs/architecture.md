# 项目架构说明

## 版本与范围

`1.2.0` 是 WeChatSendGuard 当前已发布的 Windows 正式版本。本开发分支已新增 macOS 适配候选实现；在完成签名、公证和真实客户端人工清单前，不声明 macOS 正式兼容。Windows 与 macOS 不共享二进制、安装器或系统 API，Linux 仍未实现。

本项目的目标是在受支持的微信桌面版中，为一次 `Enter` 或点击“发送”按钮的消息发送增加可取消的确认步骤。用户可选择仅保护指定名单，或启用全局防护（白名单）模式。架构把产品规则、界面和操作系统集成拆开：后续平台可以复用规则与界面契约，但必须独立实现自己的受信任进程识别、前台上下文读取和输入控制。

## 目录与职责

| 目录或包 | 职责 | 能否调用操作系统 API |
| --- | --- | --- |
| `crates/guard-core` | 规则匹配、设置 JSON、确认状态机、审计模型、标题规范化 | 否 |
| `crates/platform-api` | 平台能力接口和内存假实现 | 否 |
| `crates/guard-service` | 把物理按键、当前上下文、规则和确认状态机编排为应用服务 | 否 |
| `crates/desktop-ui` | Slint 视图、中文文案、视图状态和用户意图回调 | 否 |
| `crates/platform-windows` | Win32、UI Automation、键盘钩子、`SendInput`、托盘、启动项、文件对话框 | 是，仅 Windows API |
| `crates/platform-macos` | AppKit、Accessibility、CoreGraphics 事件 tap、签名校验、LaunchAgent、文件面板 | 是，仅 macOS 公共 API |
| `crates/desktop-app` | 按目标系统装配 Windows 或 macOS 适配器：生命周期、设置保存与界面回调 | 是，仅通过当前平台适配器 |
| `packaging/windows` | Windows 安装器脚本和发布校验 | 是，仅构建期 |
| `packaging/macos` | universal 应用、签名、公证和 DMG 构建 | 是，仅构建期 |

依赖关系不是“所有包串成一条链”，而是由目标平台组合根装配多个窄边界：

```text
desktop-app（目标平台组合根）
├── desktop-ui（Slint 界面）
├── guard-service ──┬── guard-core（规则与数据）
│                  └── platform-api（能力接口）
└── platform-windows ─┬── guard-core（上下文数据）
                       └── platform-api（能力实现）
或
└── platform-macos ───┬── guard-core（上下文数据）
                      └── platform-api（能力实现）
```

`guard-core` 不调用 Win32、Slint 或微信 API，也不保存某个 Windows 安装目录作为默认值。公共设置中允许存在可选的 `trustedWeixinExecutablePath` 字段，但它的含义、默认值与校验均由 Windows 适配器拥有。`desktop-ui` 只能显示状态和发出意图，不能读取外部应用、安装键盘钩子或注入按键。

## 技术应用摘要

WeChatSendGuard 是单进程原生桌面应用，不包含浏览器运行时、.NET、Java、微信插件、DLL/代码注入、截图识别、OCR、网络服务或远程控制。Windows 使用 Win32/UI Automation，macOS 使用 AppKit/Accessibility/CoreGraphics/Security；公共规则和 Slint 界面不调用这些系统 API。

| 能力 | 用途 | 运行时来源 |
| --- | --- | --- |
| Rust + Slint | 设置界面、确认窗和本地规则 | 应用自身 |
| Windows 窗口与进程 API | 读取前台窗口，验证受信任的 `Weixin.exe` 路径与权限 | Windows 自带 |
| UI Automation（无障碍） | 读取微信公开暴露的会话标题、会话类型、消息编辑框与焦点 | Windows 自带 |
| 键盘/鼠标钩子与 `SendInput` | 暂停物理 `Enter` 或已识别的发送按钮点击，在确认后补发一次带标记的 `Enter` | Windows 自带 |

UI Automation、控件树、事件订阅和缓存不是四套额外依赖：它们都是同一个 Windows 无障碍接口的不同用法。控件树是目标窗口公开的界面目录；应用仅在其中查找完成会话识别所需的控件。UI Automation 不能拦截物理按键，因此键盘钩子仍是发送守护不可替代的一部分。

当用户从微信切回设置窗口以“加入当前微信会话”时，设置窗口会自然成为前台。Windows 适配器因此保留一份短时有效、且仅在微信会话已识别并且消息编辑框有焦点时取得的上下文快照，供名单配置使用。该快照绝不用于授权发送；发送仍必须依赖当前前台窗口和最终重新校验。

## 运行时链路

```text
物理 Enter 或已识别的“发送”按钮点击
  → 当前平台输入门控（只读取缓存的前台快照）
  → GuardService
  → guard-core 规则判定
  ├─ 放行：原始按键继续
  ├─ 阻止：抑制原始按键，不注入任何内容
  └─ 二次确认：保存 PendingConfirmation 并显示 Slint 确认窗
                                      ↓
                              用户确认或取消
                                      ↓
            当前平台适配器恢复原编辑框并重新读取前台上下文
                                      ↓
                  guard-core 比对原始目标与最新目标
                                      ↓
            仅在完全一致时由当前平台适配器发送一次带标记的 Enter
```

确认中保存的是窗口句柄、进程身份、会话类型、规范化标题、编辑框焦点状态、按键类型和过期时间，不包含消息正文。确认窗关闭后，当前平台适配器必须重新聚焦并重新读取原目标；进程、窗口、会话、标题或编辑框任一变化都会取消发送。

键盘钩子不能在回调中查询 UI Automation 或做磁盘 I/O，只能使用由前台上下文监视器维护的不可变缓存。监视器以 WinEvent 前台、焦点、移动、缩放和布局事件为主触发刷新，并保留低频自适应校验作为无障碍事件缺失时的看门狗；事件风暴经过短去抖，所有 UI Automation 查询仍只在后台线程执行。按钮坐标与微信根窗口矩形成对缓存，纯移动窗口时在点击路径按窗口偏移立即换算，无需重新遍历控件树；尺寸、DPI 或两栏/三栏布局变化时，把旧边界标记为待刷新，并仅在两个窄候选区内保守拦截，直到事件刷新替换几何信息。UI Automation 使用属性条件直接定位消息编辑框、会话标题与发送工具栏；发送按钮优先按编辑框附近的工具栏选择，避免三栏布局中的右侧网页控件干扰。上下文与按钮边界作为一个快照同时发布，扫描开始先发布同窗口的保守状态，扫描完成时才写入新的观察时间；每次扫描带有单调序号，较慢的旧扫描不能覆盖较新的结果。快照暂时不可用或过期时，Enter 安全阻止；鼠标诊断和拦截使用同一份快照。用户关闭按钮拦截时不查询按钮控件且鼠标回调只做原子旁路；Enter 与按钮均关闭或总开关关闭时，后台扫描和事件处理停止。确认后的注入仍需重新聚焦并重新校验。`SendInput` 生成的按键附带唯一标记，钩子会忽略该标记，避免把自身注入再次当成物理输入。

## Windows 信任与兼容性边界

默认受支持的可执行文件为：

```text
C:\Program Files\Tencent\Weixin\Weixin.exe
```

设置中未保存 `trustedWeixinExecutablePath` 时，Windows 适配器使用该默认路径。设置页始终自动显示该值；用户保存其他绝对 `Weixin.exe` 路径后，适配器仅精确匹配该自定义路径。路径比较仅忽略 Windows 路径的大小写和分隔符差异，不会因为同名进程而信任目标。

保存路径设置时，`desktop-app` 会先持久化设置，再让 `WindowsContextProvider` 使旧前台快照失效并重新观察。路径无效、进程访问失败、权限级别不一致、UI Automation 控件缺失、聊天身份或编辑框焦点无法确认时，前台上下文均视为不可保护，绝不注入按键。

Windows 适配器仅读取公开的 UI Automation 元数据：会话标题、群聊/联系人类型、消息编辑框和焦点。它不读取微信数据库、进程内存、剪贴板或截图，也不注入 DLL。发送保护链路不访问网络；仅应用更新模块会在用户开启自动检查或主动操作时通过 HTTPS 访问 GitHub Releases。

## macOS 信任与兼容性边界

macOS 适配器固定校验受信任微信的代码签名身份：bundle 标识 `com.tencent.xinWeChat` 与 Team ID `5A4RE8SF68` 必须同时匹配，且系统代码签名有效性检查必须通过；进程名和安装路径不参与放宽匹配。适配器只读取 Accessibility 暴露的前台窗口、标题、编辑框、按钮和焦点元数据，并通过公开的 CoreGraphics 事件 tap 抑制物理输入、以 `kCGEventSourceUserData` 标记自身补发事件。

首次运行必须获得“辅助功能”和“输入监控”授权。授权缺失或撤销、签名不匹配、Accessibility 树不兼容、窗口无法稳定映射、上下文过期、焦点恢复失败或最终目标变化时均失败关闭，不补发输入。详细实现、权限与人工门禁见 [macOS 适配说明](macos-adaptation.md)。

## 配置、数据与版本

Windows 设置文件位于 `%LocalAppData%\WeChatSendGuard\settings.json`，macOS 设置文件位于 `~/Library/Application Support/WeChatSendGuard/settings.json`；两者使用 `camelCase` 字段和 PascalCase 枚举值。应用版本和配置架构版本独立：当前应用版本为 `1.2.0`，`schemaVersion` 仍为 `2`。新增的可选路径字段是向后兼容的扩展，旧设置文件不包含它时仍使用 Windows 默认路径；macOS 忽略该 Windows 专用字段。

默认路径不会写进 JSON；自定义路径会以如下形式保存：

```json
{
  "trustedWeixinExecutablePath": "D:\\Apps\\Weixin\\Weixin.exe"
}
```

设置通过同目录临时文件和替换操作原子写入。损坏的 JSON 回退到安全默认设置。名单修改、名单模式切换、发送守护开关和自动检查更新开关会即时保存并生效；其他设置由“保存设置”统一提交。更新偏好 `autoCheckUpdates` 默认开启，`ignoredUpdateVersion` 只抑制该版本的自动提醒，不隐藏“关于”页的手动升级入口。审计日志采用 JSON Lines，位于各平台应用数据目录下的 `logs`，记录时间、匿名链路 ID、会话 ID、事件类型、结果和必要环境版本；不记录会话名称或草稿内容，并受 1–30 天、单文件 1 MiB、总量 50 MiB 的边界约束。

## 界面边界

Slint 是当前和后续平台共用的界面技术。`crates/desktop-ui/ui/app.slint` 是界面和可见交互的唯一基线：四段式设置导航、浮出式主窗口、托盘恢复、确认窗定位、Esc 取消、长按进度、路径设置、日志诊断和应用更新状态都在此约定。

系统窗口句柄、托盘菜单对象、文件选择器和启动项属于平台适配器。界面只通过回调表达“保存路径”“恢复默认”“打开设置”等意图；组合根负责将这些意图交给当前平台服务。

## 后续平台适配边界

后续 Linux 开发者应新增 `platform-linux` 并实现 `platform-api` 中的能力接口；macOS 已按同一边界落在 `platform-macos`。不得把 `cfg` 分支、Win32/AppKit 调用、平台路径、无障碍控件名或安装逻辑加入 `guard-core`、`guard-service` 或 `desktop-ui`。

可直接复用的是：

- `guard-core` 的规则、确认状态机、标题规范化和公共配置；
- `guard-service` 的服务编排；
- `platform-api` 的接口与测试假实现；
- `desktop-ui` 的 Slint 视图、中文文案和用户可见行为；
- 审计最小化与不自动测试真实微信的安全政策。

必须重新实现的是：前台应用身份、辅助功能/无障碍 API、物理输入观察、输入抑制与注入、窗口定位和激活、托盘、启动项、应用目录与本地打包。macOS/Linux 不得把 Windows 的可执行路径字段当作自身信任规则；应定义并记录各自的签名包标识、应用标识或其他确定性本机身份。

具体必须保持的能力和安全条件见 [平台适配契约](platform-adapter-contract.md)。

## 打包目标

Windows 安装器是当前用户范围的原生包，包含 Rust 应用和 Slint 运行时，x64 安装包目标不超过 15 MB。macOS 使用独立的 universal `.app`/`.dmg`，正式发布必须使用 Developer ID 签名、Hardened Runtime、公证和 stapling；应用内更新分别选择 `.exe` 或 universal `.dmg` 及其同名 SHA-256 文件。用户无需预装 .NET 或其他运行时，两个平台的安装产物不能互用。
