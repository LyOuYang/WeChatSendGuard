# WeChatSendGuard

在工作群、客户群和私人聊天之间频繁切换时，一次误发可能造成麻烦。WeChatSendGuard 是一款本地运行的微信发送保护工具：你可以自行设置哪些联系人或群聊需要保护；当向这些会话发送消息时，应用会在发送前弹出确认，帮助你避免把消息发错对象。

## 它如何工作

- 按你的名单配置，决定哪些联系人和群聊在发送前需要确认；
- 支持拦截按 `Enter` 和点击微信“发送”按钮这两种发送方式；
- 带修饰键的 Enter 组合（例如 `Ctrl+Enter`）始终保持微信原有行为；
- 支持“仅保护指定名单”和“全局防护（白名单）”两种模式；
- 仅当目标会话、窗口和输入框状态仍然一致时，才会继续发送；
- 发送保护全程本地运行，不上传消息，不保存消息正文。

发布产物包含 Windows 10/11 x64 安装包和 macOS 11+ Universal DMG。macOS 包使用免费 Ad-hoc 签名，未使用 Apple Developer ID 或公证，首次打开可能需要按系统提示允许。项目采用 Rust + Slint 原生实现，终端用户不需要安装 .NET、Java、浏览器运行时或其他额外运行时。

## Windows 用户说明

### 安装与日常使用

从发布页获取 `WeChatSendGuard-Setup-<版本>.exe`，双击并按向导安装即可。安装器仅为当前 Windows 用户安装应用、创建开始菜单入口和卸载项，不要求管理员权限。

首次启动后：

- 启动微信并保持一个实例运行；应用会自动识别正在运行的微信并记录其实际路径，不需要手动填写安装目录；
- 在“守护名单”中选择仅保护指定会话，或启用全局防护（白名单）模式；
- 手动打开微信中的群聊或联系人，并将焦点放进消息输入框后，选择“加入当前微信会话”；
- 加入、移除或导入会话名单、名单模式切换和“启用发送保护守护”开关会立即生效；别名及其余设置点击“保存设置”后生效；
- “关于”页默认会在启动后自动检查正式版更新，也可手动检查、忽略当前版本的自动提醒，或下载并安装；更新检查只访问 GitHub Releases；
- “通用设置”中的“日志与诊断”可打开日志目录、导出诊断包或清空日志。诊断包包含操作系统和微信版本、无内容的拦截链路，不包含消息、会话名称或设置文件；
- 托盘菜单可按 1/5/15 分钟临时暂停或启用守护、临时放行当前会话、恢复被最小化或隐藏的设置窗口；
- 正常卸载不会删除 `%LocalAppData%\WeChatSendGuard` 下的设置和最小化审计日志。

### 微信识别与兼容性

Windows 会在启动时和运行期间自动识别正在运行的微信。检测到唯一的 `Weixin.exe` 实例时，应用会记录其实际绝对路径并用于精确身份校验，不要求用户填写固定安装路径。未检测到微信或同时检测到多个安装位置时，应用不会用不确定结果更新信任路径，请先启动微信或关闭多余实例后重试。

当前适配依赖微信暴露的 UI Automation 控件来识别会话和消息编辑框。识别到的路径无效、客户端版本或界面不兼容、无法确认编辑框焦点、权限级别不一致时，应用会安全地不注入按键。

## macOS 用户说明

### 安装与首次打开

从发布页下载 `WeChatSendGuard-<版本>-universal.dmg`，双击打开后，将 `WeChatSendGuard.app` 拖到“应用程序”文件夹。macOS 首次打开未公证应用时，按以下任一方式允许启动：

- 在应用上点按右键，选择“打开”，然后在弹窗中再次选择“打开”；
- 先点“取消”，再打开“系统设置 → 隐私与安全性”，在“安全性”区域选择“仍要打开”。

若系统显示“应用已损坏”，请先从 GitHub Release 重新下载；仍无法打开时，可在终端执行以下高级排障命令：

```bash
xattr -dr com.apple.quarantine /Applications/WeChatSendGuard.app
```

首次启用发送保护时，还需要在“系统设置 → 隐私与安全性”中允许 WeChatSendGuard 使用“辅助功能”和“输入监控”。

### 隐私与安全边界

- Windows 只读取前台受信任微信窗口公开的 UI Automation 元数据；macOS 只读取前台受信任微信窗口公开的 Accessibility 元数据：会话标题、会话类型和消息编辑框焦点。
- 不修改微信，不注入 DLL，不读取数据库、剪贴板或进程内存，也不截屏。只有启用更新检查或用户主动下载安装时，才通过 HTTPS 访问 GitHub Releases；不会上传消息、会话、日志或设置。
- 草稿预览仅在确认窗口打开期间按需保存在内存中，不写入设置或审计日志。
- 确认发送前会再次核验前台窗口、进程、会话类型、规范化标题和编辑框焦点；任一条件变化都会取消，不会发送按键。
- 审计日志只记录时间、受保护会话 ID、匿名链路 ID、事件类型、结果和必要环境版本；不包含聊天名称或消息内容。日志按天轮转、单文件最多 1 MiB、总量最多 50 MiB，并按用户设置的 1–30 天自动清理。

## 所有平台共用的说明

Windows 和 macOS 共享以下产品契约，Linux 适配也必须保持一致：

- `guard-core` 的名单匹配、未知会话处理、二次确认、超时与目标变更拒绝逻辑；
- 设置 JSON 的公共字段、受保护/放行名单语义和最小化审计数据结构；
- 确认流程的取消优先、目标重新校验、输入只在最终校验成功后发生；
- 不读取微信数据库、进程内存、剪贴板或截图，不持久化草稿；更新功能仅访问 GitHub Releases，不使用遥测或其他远程服务；
- Slint 界面的中文文案、信息层级、键盘可用性和状态含义。

平台相关能力不能放进公共规则层。后续系统应新增自己的平台适配器，实现 `platform-api` 契约；不得复制或通过条件编译复用 Windows 的 Win32、UI Automation、键盘钩子或安装器代码。

## 架构与维护文档

- [项目架构说明](docs/architecture.md)：目录职责、依赖方向、运行时链路、配置与版本边界。
- [平台适配契约](docs/platform-adapter-contract.md)：macOS、Linux 后续开发者需要遵守的接口和安全条件。
- [界面契约](docs/ui-parity.md)：Slint 视图的视觉与交互基线。
- [测试策略](docs/testing-policy.md)：自动化测试与真实微信人工验证的严格边界。
- [人工微信验证清单](docs/manual-wechat-validation.md)：发布负责人使用的非自动化验证步骤。
- [安全与隐私边界](docs/security.md)、[macOS 适配说明](docs/macos-adaptation.md) 与 [发布流程](docs/release.md)。

## Windows 开发与发布

开发环境需要 Rust 1.92+、Windows MSVC 工具链，以及仅用于构建 Windows 安装包的 NSIS 3.x。NSIS 不会进入最终安装包。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
.\packaging\windows\build-installer.ps1
```

如果 `makensis.exe` 不在 `PATH`，可显式传入其路径：

```powershell
.\packaging\windows\build-installer.ps1 -MakensisPath 'C:\path\to\makensis.exe'
```

构建产物位于 `dist\windows\WeChatSendGuard-Setup-<版本>.exe`，同目录会生成对应的 `.sha256` 校验文件。`VERSION` 是唯一的产品版本来源，Rust workspace 版本和安装器元数据必须与它一致；配置格式的 `schemaVersion` 独立维护。

安装器使用 NSIS 的 LZMA 固实压缩，因此不应直接拿它和 `target\...\release\WeChatSendGuard.exe` 比大小：约 20 MB 的主程序通常会得到约 5–8 MB 的安装包；15 MB 是发布上限，不是目标大小。

推送与 `VERSION` 完全一致的标签，会触发 GitHub Actions：检查格式、静态检查和测试，构建 Windows 安装包、macOS Universal DMG 及对应校验文件，并分别创建稳定版 GitHub Release 或 GitHub Pre-release。`main` 只能发布稳定版，`dev` 只能发布 Beta。Beta 当前通过 Pre-release 页面手动安装；手动触发发布工作流也会按已有标签执行发布门禁。

自动化测试只使用内存假实现，不会启动、枚举、附加 UI Automation、读取草稿或向真实微信发送输入。真实微信兼容性只能由发布负责人依照人工验证清单完成。
