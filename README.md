# WeChatSendGuard

Windows 原生托盘工具：当支持的微信桌面版会话通过 `Enter` 发送消息时，按规则增加一次确认，减少误发风险。

当前生产实现使用 Rust + Slint，面向 Windows 10/11 x64。终端用户安装后不需要 .NET、Java、浏览器运行时或其他额外运行时。

## 安装与使用

从发布页获取 `WeChatSendGuard-Setup-<version>.exe`，双击后按向导安装即可。安装器为当前 Windows 用户安装应用、创建开始菜单入口和卸载项，不要求管理员权限。

首次启动后：

- 在“发送守护”中选择保护名单或全局拦截模式；
- 手动打开微信中的群聊或联系人，并将焦点放进消息输入框后，选择“加入当前会话”；
- 名单的新增、删除和导入会立即生效；其他设置需要点击“保存设置”；
- 使用托盘菜单可临时暂停守护、临时放行当前已保护会话，或显示主窗口；
- 卸载不会删除 `%LocalAppData%\WeChatSendGuard` 下的设置和最小化审计日志。

支持的微信桌面版路径是 `C:\Program Files\Tencent\Weixin\Weixin.exe`。当前适配依赖微信暴露的 UI Automation 控件；如果客户端版本或界面不兼容，应用会安全地不注入按键。

## 安全与隐私边界

- 只读取前台受信任微信窗口公开的 UI Automation 元数据：会话标题、会话类型和消息编辑框焦点。
- 不修改微信、不注入 DLL、不读取数据库、剪贴板或进程内存，不截屏，也不发起网络请求。
- 草稿预览仅在确认窗口打开期间按需读取、显示在内存中；不会写入配置或审计日志。
- 确认发送前会再次核验前台窗口、受信任进程、会话类型、规范化标题和编辑框焦点；任一条件变化都会取消，不会发送按键。
- 审计日志只记录时间、受保护会话 ID、事件类型和结果，不包含聊天名称或消息内容。

详细架构和跨平台适配边界见 [架构说明](docs/architecture.md) 与 [平台适配契约](docs/platform-adapter-contract.md)。界面由 [Slint](https://slint.dev/) 构建。

## 开发与发布

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

构建产物在 `dist\windows\WeChatSendGuard-Setup-<version>.exe`。`VERSION` 是唯一的产品版本来源，和 Rust workspace 版本、安装器元数据必须一致；配置格式的 `schemaVersion` 独立维护。

## 测试边界

自动化测试只使用内存假实现，绝不启动、枚举、附加 UI Automation、读取草稿或向真实微信发送输入。真实微信兼容性由发布负责人按照 [人工验证清单](docs/manual-wechat-validation.md) 手工完成；该清单建议使用专用测试账号和私有测试会话。

发布门禁、签名、安装升级和体积要求见 [发布流程](docs/release.md)。
