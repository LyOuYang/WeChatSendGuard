# Windows 发布流程

## 版本规则

`VERSION` 是唯一的产品版本来源，使用 SemVer：`主版本.次版本.修订版本`。每个 Rust 包和 Windows 安装器元数据必须与它一致。应用版本与设置的 `schemaVersion` 独立；当前 `1.1.0` 对应 `schemaVersion: 2`，只有存在经过记录的设置迁移时才升级配置架构版本。

## 本地打包

安装 Rust 1.92+、Windows MSVC 工具链和 NSIS 3.x 后，在仓库根目录执行：

```powershell
.\packaging\windows\build-installer.ps1
```

脚本会构建 `x86_64-pc-windows-msvc` 的 Release 主程序，使用 NSIS/LZMA 生成 `dist/windows/WeChatSendGuard-Setup-<VERSION>.exe`，校验安装器元数据与体积上限，并生成同名 `.sha256` 文件。NSIS 固实压缩会显著缩小体积：约 20 MB 的主程序生成约 5–8 MB 安装包是正常结果；15 MB 是上限而非目标。

NSIS 不在 `PATH` 时，可传入完整路径：

```powershell
.\packaging\windows\build-installer.ps1 -MakensisPath 'C:\path\to\makensis.exe'
```

## 发布门禁

1. 执行 Rust 格式化检查、Clippy、单元测试、配置夹具和假平台集成测试。
2. 执行 `packaging/windows/build-installer.ps1`。脚本会构建干净的 `x86_64-pc-windows-msvc` 发布版本、调用 NSIS，并执行元数据和 15 MB 体积上限检查。
3. 确认输出为 `dist/windows/WeChatSendGuard-Setup-<VERSION>.exe` 及其 `.sha256`，在发布记录中保存 SHA-256 和字节大小。
4. 完成 SBOM/许可证审查，并确认 Slint 官方 About 窗口仍可从“通用设置”和托盘打开。
5. 使用生产证书签名可执行文件和安装器（没有证书时，GitHub Actions 仍可自动构建和发布，但 Windows 可能显示未知发布者提示）。
6. 在干净 Windows 虚拟机完成安装、应用内下载升级和卸载验证，确认用户设置可跨升级保留。
7. 完成 [人工微信验证清单](manual-wechat-validation.md)。真实微信不是自动化测试目标。

当 NSIS 未加入 `PATH` 时，构建脚本可接收 `-MakensisPath`。NSIS 只是开发期打包器，不会进入安装包。

## GitHub Actions 自动发布

`.github/workflows/release-windows.yml` 在 Windows 2022 runner 上运行。发布负责人完成上述人工验证和版本更新后，推送与 `VERSION` 严格一致的标签即可：

```powershell
git tag v<VERSION>
git push origin v<VERSION>
```

工作流会校验标签、格式、Clippy 和测试，安装 NSIS，构建安装器，生成 `WeChatSendGuard-Setup-<VERSION>.exe.sha256`，再把两者作为同名 GitHub Release 的资产发布。手动运行工作流仅上传构建产物，便于预发布复核，不会创建 Release。

应用内更新仅接受这种命名和校验文件齐全的正式 Release；因此不要手动重命名或删除任一资产。代码签名不是自动发布的前置条件，但生产发布建议将签名步骤和证书机密单独接入受保护的发布环境。

## 安装器行为

安装器仅将 Windows 应用安装到 `%LocalAppData%\Programs\WeChatSendGuard`，创建开始菜单组和已注册卸载项。正常卸载不删除 `%LocalAppData%\WeChatSendGuard`；删除设置和审计日志必须由用户做出单独、明确的选择。

## 后续平台

macOS 与 Linux 在未来必须分别产出签名/公证或包管理器原生的独立产物，不能共享 Windows 安装器或二进制。共享的是 `guard-core`、Slint 界面、公共配置语义和平台适配契约，而不是嵌入公共代码中的 Windows API。
