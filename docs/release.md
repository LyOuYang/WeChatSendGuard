# Windows 发布流程

## 版本规则

`VERSION` 是唯一的产品版本来源，使用 SemVer：`主版本.次版本.修订版本`。每个 Rust 包和 Windows 安装器元数据必须与它一致。应用版本与设置的 `schemaVersion` 独立；当前 `1.0.0` 对应 `schemaVersion: 2`，只有存在经过记录的设置迁移时才升级配置架构版本。

## 发布门禁

1. 执行 Rust 格式化检查、Clippy、单元测试、配置夹具和假平台集成测试。
2. 执行 `packaging/windows/build-installer.ps1`。脚本会构建干净的 `x86_64-pc-windows-msvc` 发布版本、调用 NSIS，并执行元数据和 15 MB 体积门禁。
3. 确认输出为 `dist/windows/WeChatSendGuard-Setup-<VERSION>.exe`，在发布记录中保存 SHA-256 和字节大小。
4. 完成 SBOM/许可证审查，并确认 Slint 官方 About 窗口仍可从“通用设置”和托盘打开。
5. 使用生产证书签名可执行文件和安装器。
6. 在干净 Windows 虚拟机完成安装、升级和卸载验证，确认用户设置可跨升级保留。
7. 完成 [人工微信验证清单](manual-wechat-validation.md)。真实微信不是自动化测试目标。

当 NSIS 未加入 `PATH` 时，构建脚本可接收 `-MakensisPath`。NSIS 只是开发期打包器，不会进入安装包。

## 安装器行为

安装器仅将 Windows 应用安装到 `%LocalAppData%\Programs\WeChatSendGuard`，创建开始菜单组和已注册卸载项。正常卸载不删除 `%LocalAppData%\WeChatSendGuard`；删除设置和审计日志必须由用户做出单独、明确的选择。

## 后续平台

macOS 与 Linux 在未来必须分别产出签名/公证或包管理器原生的独立产物，不能共享 Windows 安装器或二进制。共享的是 `guard-core`、Slint 界面、公共配置语义和平台适配契约，而不是嵌入公共代码中的 Windows API。
