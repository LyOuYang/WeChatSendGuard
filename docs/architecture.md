# 项目架构说明

## 版本与范围

`1.0.0` 是 WeChatSendGuard 的第一个正式发布版本。当前只实现并交付 Windows 10/11 x64；macOS 和 Linux 没有隐藏的实验实现，也不共享 Windows 二进制、安装器或系统 API。

本项目的目标是在受支持的微信桌面版中，为一次 `Enter` 发送增加可取消的确认步骤。架构把产品规则、界面和操作系统集成拆开：后续平台可以复用规则与界面契约，但必须独立实现自己的受信任进程识别、前台上下文读取和输入控制。

## 目录与职责

| 目录或包 | 职责 | 能否调用操作系统 API |
| --- | --- | --- |
| `crates/guard-core` | 规则匹配、设置 JSON、确认状态机、审计模型、标题规范化 | 否 |
| `crates/platform-api` | 平台能力接口和内存假实现 | 否 |
| `crates/guard-service` | 把物理按键、当前上下文、规则和确认状态机编排为应用服务 | 否 |
| `crates/desktop-ui` | Slint 视图、中文文案、视图状态和用户意图回调 | 否 |
| `crates/platform-windows` | Win32、UI Automation、键盘钩子、`SendInput`、托盘、启动项、文件对话框 | 是，仅 Windows API |
| `crates/desktop-app` | Windows 组合根：生命周期、设置保存、界面回调与平台服务装配 | 是，通过 Windows 适配器 |
| `packaging/windows` | Windows 安装器脚本和发布校验 | 是，仅构建期 |

依赖关系不是“所有包串成一条链”，而是由 Windows 组合根装配多个窄边界：

```text
desktop-app（Windows 组合根）
├── desktop-ui（Slint 界面）
├── guard-service ──┬── guard-core（规则与数据）
│                  └── platform-api（能力接口）
└── platform-windows ─┬── guard-core（上下文数据）
                       └── platform-api（能力实现）
```

`guard-core` 不调用 Win32、Slint 或微信 API，也不保存某个 Windows 安装目录作为默认值。公共设置中允许存在可选的 `trustedWeixinExecutablePath` 字段，但它的含义、默认值与校验均由 Windows 适配器拥有。`desktop-ui` 只能显示状态和发出意图，不能读取外部应用、安装键盘钩子或注入按键。

## 运行时链路

```text
物理 Enter
  → Windows 键盘钩子（只读取缓存的前台快照）
  → GuardService
  → guard-core 规则判定
  ├─ 放行：原始按键继续
  ├─ 阻止：抑制原始按键，不注入任何内容
  └─ 二次确认：保存 PendingConfirmation 并显示 Slint 确认窗
                                      ↓
                              用户确认或取消
                                      ↓
            Windows 适配器恢复原编辑框并重新读取前台上下文
                                      ↓
                  guard-core 比对原始目标与最新目标
                                      ↓
            仅在完全一致时由 Windows 适配器发送一次带标记的 Enter
```

确认中保存的是窗口句柄、进程身份、会话类型、规范化标题、编辑框焦点状态、按键类型和过期时间，不包含消息正文。确认窗关闭后，Windows 适配器必须重新聚焦并重新读取原目标；进程、窗口、会话、标题或编辑框任一变化都会取消发送。

键盘钩子不能在回调中查询 UI Automation 或做磁盘 I/O，只能使用由前台上下文监视器维护的缓存。`SendInput` 生成的按键附带唯一标记，钩子会忽略该标记，避免把自身注入再次当成物理输入。

## Windows 信任与兼容性边界

默认受支持的可执行文件为：

```text
C:\Program Files\Tencent\Weixin\Weixin.exe
```

设置中未保存 `trustedWeixinExecutablePath` 时，Windows 适配器使用该默认路径。设置页始终自动显示该值；用户保存其他绝对 `Weixin.exe` 路径后，适配器仅精确匹配该自定义路径。路径比较仅忽略 Windows 路径的大小写和分隔符差异，不会因为同名进程而信任目标。

保存路径设置时，`desktop-app` 会先持久化设置，再让 `WindowsContextProvider` 使旧前台快照失效并重新观察。路径无效、进程访问失败、权限级别不一致、UI Automation 控件缺失、聊天身份或编辑框焦点无法确认时，前台上下文均视为不可保护，绝不注入按键。

Windows 适配器仅读取公开的 UI Automation 元数据：会话标题、群聊/联系人类型、消息编辑框和焦点。它不读取微信数据库、进程内存、剪贴板或截图，不注入 DLL，也不访问网络。

## 配置、数据与版本

设置文件位于 `%LocalAppData%\WeChatSendGuard\settings.json`，使用 `camelCase` 字段和 PascalCase 枚举值。应用版本和配置架构版本独立：当前应用版本为 `1.0.0`，`schemaVersion` 仍为 `2`。新增的可选路径字段是向后兼容的扩展，旧设置文件不包含它时仍使用 Windows 默认路径，因此无需升级 `schemaVersion`。

默认路径不会写进 JSON；自定义路径会以如下形式保存：

```json
{
  "trustedWeixinExecutablePath": "D:\\Apps\\Weixin\\Weixin.exe"
}
```

设置通过同目录临时文件和替换操作原子写入。损坏的 JSON 回退到安全默认设置。名单修改、名单模式切换和发送守护开关会即时保存并生效；其他设置由“保存设置”统一提交。审计日志采用 JSON Lines，位于 `%LocalAppData%\WeChatSendGuard\logs`，只保留时间、会话 ID、事件类型和结果，不记录会话名称或草稿内容。

## 界面边界

Slint 是当前和后续平台共用的界面技术。`crates/desktop-ui/ui/app.slint` 是界面和可见交互的唯一基线：三段式设置导航、浮出式主窗口、托盘恢复、确认窗定位、Esc 取消、长按进度、路径设置和中文状态语义都在此约定。

系统窗口句柄、托盘菜单对象、文件选择器和启动项属于平台适配器。界面只通过回调表达“保存路径”“恢复默认”“打开设置”等意图；组合根负责将这些意图交给当前平台服务。

## 后续平台适配边界

后续 macOS 或 Linux 开发者应新增平台包，例如 `platform-macos` 或 `platform-linux`，并实现 `platform-api` 中的能力接口。不得把 `cfg` 分支、Win32 调用、Windows 路径、UI Automation 控件名或 Windows 安装逻辑加入 `guard-core`、`guard-service` 或 `desktop-ui`。

可直接复用的是：

- `guard-core` 的规则、确认状态机、标题规范化和公共配置；
- `guard-service` 的服务编排；
- `platform-api` 的接口与测试假实现；
- `desktop-ui` 的 Slint 视图、中文文案和用户可见行为；
- 审计最小化与不自动测试真实微信的安全政策。

必须重新实现的是：前台应用身份、辅助功能/无障碍 API、物理输入观察、输入抑制与注入、窗口定位和激活、托盘、启动项、应用目录与本地打包。macOS/Linux 不得把 Windows 的可执行路径字段当作自身信任规则；应定义并记录各自的签名包标识、应用标识或其他确定性本机身份。

具体必须保持的能力和安全条件见 [平台适配契约](platform-adapter-contract.md)。

## 打包目标

Windows 安装器是当前用户范围的原生包，包含 Rust 应用和 Slint 运行时。每次发布都必须执行体积门禁，x64 安装包目标不超过 15 MB；用户不需要预装 .NET。macOS 与 Linux 将在未来各自生成签名、认证或包管理器原生格式的独立产物，不能复用 Windows 安装包。
