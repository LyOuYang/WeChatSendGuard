# Windows 与 macOS 构建和发布流程

本项目采用“分支构建、Tag 发布、双平台原子发布”模型：推送代码只做验证和候选包构建；推送 `v<版本>` Tag 才会签名、验证并创建正式 GitHub Release。Windows 和 macOS 必须来自同一个 Tag/Commit，任一平台失败都不会创建 Release。

## 版本规则

`VERSION` 是产品版本来源，使用 SemVer：`主版本.次版本.修订版本`。每个 Rust 包、Windows 安装器和 macOS bundle 元数据必须与它一致；Tag 必须严格为 `v<VERSION>`，且 Tag 对应提交必须已经在 `main` 分支中。应用版本与设置的 `schemaVersion` 独立；只有存在经过记录的设置迁移时才升级配置架构版本。

## 分支构建

`.github/workflows/ci.yml` 在 PR、`dev`/`main` 推送和手动运行时执行格式、Clippy、测试以及 Windows x64 和 macOS universal 构建。`dev` 产物用于集成验证，`main` 产物用于发布前复核；两者都只上传带 Commit SHA 的 Actions Artifact，不创建 GitHub Release。平台 Job 独立运行，单个平台失败时可以在 Actions 中只重跑该 Job。

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

## GitHub Actions 正式发布

`.github/workflows/release.yml` 在 Windows 2022 和 macOS 14 runner 上并行构建。发布负责人完成版本更新、`main` 双平台构建和人工验证后，推送与 `VERSION` 严格一致的标签即可：

```powershell
git tag v<VERSION>
git push origin v<VERSION>
```

工作流先校验 Tag、`VERSION`、Cargo 版本和 `main` 祖先关系，再独立构建 Windows 安装器和 macOS universal DMG。最终 Publish Job 只有在两个平台都成功、四个资产和 SHA-256 全部校验通过后才创建同一个 GitHub Release。Windows 成功而 macOS 失败时，只需重跑 macOS Job；如果代码、版本或打包脚本发生变化，则必须使用新 Commit 和新 Tag 重新构建两个平台。

正式 macOS Job 使用 `production` Environment，并要求配置以下 Secrets：`MACOS_CODESIGN_IDENTITY`、`MACOS_CERTIFICATE_P12_BASE64`、`MACOS_CERTIFICATE_PASSWORD`、`MACOS_NOTARY_API_KEY_BASE64`、`MACOS_NOTARY_KEY_ID`、`MACOS_NOTARY_ISSUER_ID`。证书在临时 keychain 中导入，App Store Connect API Key 用于 `notarytool`；没有这些凭据时流程会失败关闭，不会发布临时签名包。分支构建允许临时签名，仅用于开发验证。

应用内更新仅接受这种命名和校验文件齐全的正式 Release；因此不要手动重命名或删除任一资产。生产签名和公证是 macOS 正式发布的前置条件，相关证书和 API Key 只能放在受保护的 GitHub Environment 中。

## 安装器行为

安装器仅将 Windows 应用安装到 `%LocalAppData%\Programs\WeChatSendGuard`，创建开始菜单组和已注册卸载项。正常卸载不删除 `%LocalAppData%\WeChatSendGuard`；删除设置和审计日志必须由用户做出单独、明确的选择。

## macOS 候选包

macOS 构建前安装 Rust 1.92+、Apple Silicon/Intel 两个 target 和 Xcode Command Line Tools，然后运行：

```bash
MACOS_BUILD_KIND=universal ./packaging/macos/build-app.sh
```

脚本先执行格式、Clippy 和全工作区测试，再构建 universal `.app`，启用 Hardened Runtime，生成并签名 `dist/macos/WeChatSendGuard-<VERSION>-universal.dmg` 及同名 `.sha256`。正式发布必须设置 `MACOS_CODESIGN_IDENTITY` 与 `MACOS_NOTARY_KEYCHAIN_PROFILE`，完成 notarytool、公证等待和 stapling；临时签名产物只能用于开发验证。

发布门禁还必须记录 macOS 版本、微信版本、bundle/Team 身份、辅助功能与输入监控授权、DMG SHA-256、`codesign --verify --deep --strict`、`spctl`/stapler 结果和完整人工清单。任一缺失都不能声明 macOS 正式兼容。GitHub Release 中的 DMG 文件名必须与应用内更新选择规则一致。

## 后续平台

Linux 未来必须产出桌面环境认可的独立原生包，不能共享 Windows 安装器或 macOS DMG。三个平台共享的是 `guard-core`、Slint 界面、公共配置语义和平台适配契约，而不是系统 API。
