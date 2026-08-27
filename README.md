# WeChatSendGuard

WeChatSendGuard 是一个本地运行的发送确认工具：当受支持的微信桌面版会话通过 `Enter` 发送消息时，它会依照规则增加一次确认，以降低误发风险。

当前正式版本为 `1.0.0`，只发布 Windows 10/11 x64 安装包。项目已采用 Rust + Slint 原生实现，终端用户不需要安装 .NET、Java、浏览器运行时或其他额外运行时。

## Windows 用户说明

### 安装与日常使用

从发布页获取 `WeChatSendGuard-Setup-<版本>.exe`，双击并按向导安装即可。安装器仅为当前 Windows 用户安装应用、创建开始菜单入口和卸载项，不要求管理员权限。

首次启动后：

- 在“守护名单”中选择仅保护指定会话，或启用全局防护（白名单）模式；
- 手动打开微信中的群聊或联系人，并将焦点放进消息输入框后，选择“加入当前微信会话”；
- 名单修改和“启用发送保护守护”开关会立即生效；其余设置点击“保存设置”后生效；
- 托盘菜单可暂停或启用守护、临时放行当前会话、恢复被最小化或隐藏的设置窗口；
- 正常卸载不会删除 `%LocalAppData%\WeChatSendGuard` 下的设置和最小化审计日志。

### 受支持的微信与路径设置

默认支持的微信桌面版路径是：

```text
C:\Program Files\Tencent\Weixin\Weixin.exe
```

“通用设置”会自动填入该路径，因此大多数用户无需修改即可使用。若微信安装在其他本地磁盘，可改为实际的绝对 `Weixin.exe` 路径并保存；“恢复默认”会回到上述官方默认路径。自定义路径是进程信任边界，应用只会精确匹配该路径的进程，不会仅依据进程名信任目标。

当前适配依赖微信暴露的 UI Automation 控件来识别会话和消息编辑框。路径不匹配、客户端版本或界面不兼容、无法确认编辑框焦点、权限级别不一致时，应用会安全地不注入按键。

### 隐私与安全边界

- 只读取前台受信任微信窗口公开的 UI Automation 元数据：会话标题、会话类型和消息编辑框焦点。
- 不修改微信，不注入 DLL，不读取数据库、剪贴板或进程内存，不截屏，也不发起网络请求。
- 草稿预览仅在确认窗口打开期间按需保存在内存中，不写入设置或审计日志。
- 确认发送前会再次核验前台窗口、进程、会话类型、规范化标题和编辑框焦点；任一条件变化都会取消，不会发送按键。
- 审计日志只记录时间、受保护会话 ID、事件类型和结果，不包含聊天名称或消息内容。

## 所有平台共用的说明

Windows 是 `1.0.0` 的唯一交付平台，但以下内容是后续 macOS、Linux 适配必须保持的产品契约：

- `guard-core` 的名单匹配、未知会话处理、二次确认、超时与目标变更拒绝逻辑；
- 设置 JSON 的公共字段、受保护/放行名单语义和最小化审计数据结构；
- 确认流程的取消优先、目标重新校验、输入只在最终校验成功后发生；
- 不读取消息正文以外的私有数据，不持久化草稿，不使用远程服务；
- Slint 界面的中文文案、信息层级、键盘可用性和状态含义。

平台相关能力不能放进公共规则层。后续系统应新增自己的平台适配器，实现 `platform-api` 契约；不得复制或通过条件编译复用 Windows 的 Win32、UI Automation、键盘钩子或安装器代码。

## 架构与维护文档

- [项目架构说明](docs/architecture.md)：目录职责、依赖方向、运行时链路、配置与版本边界。
- [平台适配契约](docs/platform-adapter-contract.md)：macOS、Linux 后续开发者需要遵守的接口和安全条件。
- [界面契约](docs/ui-parity.md)：Slint 视图的视觉与交互基线。
- [测试策略](docs/testing-policy.md)：自动化测试与真实微信人工验证的严格边界。
- [人工微信验证清单](docs/manual-wechat-validation.md)：发布负责人使用的非自动化验证步骤。
- [安全与隐私边界](docs/security.md) 与 [Windows 发布流程](docs/release.md)。

## Windows 开发与发布

开发环境需要 Rust 1.88+、Windows MSVC 工具链，以及仅用于构建安装包的 NSIS 3.x。NSIS 不会进入最终安装包。

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

构建产物位于 `dist\windows\WeChatSendGuard-Setup-<版本>.exe`。`VERSION` 是唯一的产品版本来源，Rust workspace 版本和安装器元数据必须与它一致；配置格式的 `schemaVersion` 独立维护。

自动化测试只使用内存假实现，不会启动、枚举、附加 UI Automation、读取草稿或向真实微信发送输入。真实微信兼容性只能由发布负责人依照人工验证清单完成。
