# macOS 适配说明

## 当前状态

本分支提供与 Windows 公共规则、确认状态机和 Slint 界面一致的 macOS 候选实现。自动化门禁只证明平台边界、纯逻辑和 macOS 构建可用；只有在签名候选包上完成 `docs/manual-wechat-validation.md` 的 macOS 清单后，发布记录才能声明具体 macOS/微信版本兼容。

适配支持 macOS 11 及以上、Apple Silicon 与 Intel universal 包。开发时确认的官方微信身份为：

- bundle 标识：`com.tencent.xinWeChat`
- Team ID：`5A4RE8SF68`

两个值与系统代码签名有效性必须同时通过。安装位置、进程名称或同名应用不能获得信任。

## 平台能力映射

| 产品能力 | macOS 实现 | 失败行为 |
| --- | --- | --- |
| 前台应用与签名信任 | `NSWorkspace` + Security.framework 签名信息 | 非官方签名不观察、不注入 |
| 会话、编辑框与发送按钮 | Accessibility 公开属性和几何信息 | 控件缺失或布局不兼容时安全阻止 Enter，不猜测消息内容 |
| 物理 Enter、Esc 与按钮点击 | CoreGraphics session event tap | 无“输入监控”权限时拒绝启动守护 |
| 确认后补发 | 标记 `kCGEventSourceUserData` 的单次 Quartz Enter | 恢复焦点、目标重校验或 PostEvent 任一失败都不发送 |
| 窗口恢复与原生文件面板 | AppKit | 系统拒绝焦点时仍保持取消优先 |
| 登录启动 | 当前用户 `~/Library/LaunchAgents/io.github.lyouyang.WeChatSendGuard.plist` | 不请求管理员权限；写入失败则设置不生效并显示错误 |
| 设置与日志 | `~/Library/Application Support/WeChatSendGuard` | 原子设置写入；日志异步、有界、无消息正文和会话名 |

Accessibility 扫描只在标题栏和消息编辑区读取定位所需字符串；不会遍历读取聊天消息正文。草稿只在确认窗显式请求时从已重新定位的编辑框读取最多 240 个字符，保留于内存且不进入日志。

## 权限与首次启动

首次运行会请求“辅助功能”；安装输入事件 tap 时会请求“输入监控”。用户需要在“系统设置 → 隐私与安全性”中同时允许 WeChatSendGuard，并在授权变化后重新打开应用。任一权限被撤销后，适配器发布不可用快照并拒绝合成输入，不降级成可能漏拦截的模式。

开发期直接运行裸二进制时，系统权限记录可能绑定到构建路径；发布验证必须使用签名 `.app`，不能把开发二进制的授权结果当作发布证据。

## 设置兼容

macOS 继续读写 `schemaVersion: 2`。历史字段 `trustedWeixinExecutablePath` 仅由 Windows 使用；macOS 界面只读显示签名身份并忽略该字段。为保持现有 schema，历史 JSON 字段 `startWithWindows` 在 macOS 组合根中解释为“登录 macOS 时启动”，但只由 macOS LaunchAgent 适配器执行，不改变规则语义。

## 构建与签名

准备 Rust 1.92+。Apple Silicon 开发包只需 `aarch64-apple-darwin` target，并可使用临时签名：

```bash
./packaging/macos/build-app.sh
```

当前公开发布不需要 Apple Developer 账号，使用 Ad-hoc 签名构建 Universal DMG：

```bash
MACOS_CODESIGN_IDENTITY=- \
MACOS_BUILD_KIND=universal \
./packaging/macos/build-app.sh
```

默认脚本生成 `dist/macos/WeChatSendGuard-<VERSION>-arm64.dmg` 和同名 `.sha256`。设置 `MACOS_BUILD_KIND=universal` 时还需安装 `x86_64-apple-darwin` target，并生成 GitHub Release 使用的 universal DMG。Beta 文件名可以包含完整 SemVer 预发布后缀，例如 `WeChatSendGuard-1.4.0-beta.1-universal.dmg`。由于 Apple 要求 `CFBundleShortVersionString` 和 `CFBundleVersion` 只能包含数字与点，脚本会将应用版本 `1.4.0-beta.1` 映射为数字短版本 `1.4.0` 和纯数字构建号；应用界面仍显示 Cargo 的完整版本。应用内更新只接受 universal 命名；Windows 仍只接受自己的 `.exe` 资产。普通用户不需要手动校验 `.sha256`，应用内更新会自动完成校验。

Ad-hoc 签名不能替代 Developer ID 或公证，因此首次打开可能需要用户在 Finder 中右键选择“打开”，或在“系统设置 → 隐私与安全性”中选择“仍要打开”。如果系统报告“应用已损坏”，先重新下载；仍失败时可使用 `xattr -dr com.apple.quarantine /Applications/WeChatSendGuard.app` 排障。未来购买 Apple Developer 账号后，可以在不改变产物命名的前提下增加 Developer ID、公证和 stapling。

## 验证边界

自动化测试不得启动或枚举真实微信、操作真实账号、读取真实草稿或发送真实输入。发布负责人必须使用隔离测试账号和私有测试会话人工验证：联系人/群聊分类、Enter 与数字键盘 Enter、发送按钮、Esc、三种确认方式、窗口/会话切换后的拒绝、权限撤销、登录启动、托盘恢复、日志隐私、DMG 升级以及签名/公证结果。任何失败或跳过都不能用于声明功能一致。
