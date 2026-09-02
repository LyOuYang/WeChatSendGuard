# Windows 与 macOS 构建和发布流程

本项目采用“分支构建、Tag 发布、双平台原子发布”模型：推送代码只做验证和候选包构建；推送符合规则的 `v<版本>` Tag 才会构建、验证并创建 GitHub Release。`main` 只发布稳定版，`dev` 只发布 Beta；Windows 和 macOS 必须来自同一个 Tag/Commit，任一平台失败都不会创建 Release。macOS 当前使用免费 Ad-hoc 签名，不要求 Apple Developer 账号；未公证状态必须在用户文档中明确说明。

## 版本规则

`VERSION` 是产品版本来源，使用 SemVer。稳定版使用 `X.Y.Z`，Beta 使用 `X.Y.Z-beta.N`，例如 `1.4.0-beta.1`。每个 Rust 包、Windows 安装器和 DMG 文件名必须与它一致；Tag 必须严格为 `v<VERSION>`。稳定 Tag 对应提交必须已经在 `main` 分支中，Beta Tag 对应提交必须已经在 `dev` 分支中。应用版本与设置的 `schemaVersion` 独立；只有存在经过记录的设置迁移时才升级配置架构版本。

## 分支构建

`.github/workflows/ci.yml` 在 PR、`dev`/`main` 推送和手动运行时执行格式、Clippy、测试以及 Windows x64 和 macOS universal 构建。`dev` 产物用于集成验证，`main` 产物用于发布前复核；两者都只上传带 Commit SHA 的 Actions Artifact，不创建 GitHub Release。平台 Job 独立运行，单个平台失败时可以在 Actions 中只重跑该 Job。

## 分支规则

- `main`：生产稳定分支。只能通过 Pull Request 合入经过验证的代码，稳定版 Tag 只能从该分支的提交创建。
- `dev`：集成与 Beta 分支。功能分支和修复分支先合入这里，Beta Tag 只能从该分支的提交创建。
- `feature/*`、`fix/*`：短期工作分支，完成后通过 Pull Request 合入 `dev`，不直接发布 GitHub Release。
- `release/x.y.z`：只有需要冻结版本并进行较长人工验证时临时创建；验证完成后合入 `main`，并同步回 `dev`。
- `hotfix/x.y.z`：从 `main` 修复生产问题，发布稳定 Tag 后必须回合并到 `dev`。

`main` 和 `dev` 都应启用分支保护，要求 Pull Request、CI 通过和至少一名审查者。`v*` Tag 应配置仓库 Ruleset，限制创建、更新和删除权限；已发布 Tag 不移动、不复用。分支 Push 只产生 CI 候选 Artifact，不直接创建 Release。

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
5. macOS 使用 Ad-hoc 签名构建 Universal DMG，不配置 Apple Developer Secrets；未来切换 Developer ID 与公证时，必须另行完成签名、公证和人工兼容验证。
6. 在干净 Windows 虚拟机完成安装、应用内下载升级和卸载验证，确认用户设置可跨升级保留。
7. 完成 [人工微信验证清单](manual-wechat-validation.md)。真实微信不是自动化测试目标。

Beta 额外要求测试升级、降级和稳定版隔离。当前 Beta 只通过 GitHub Pre-release 页面手动安装，稳定版应用不会自动获取 Beta；Beta 应由明确的测试人员使用，不作为普通用户的更新入口。

当 NSIS 未加入 `PATH` 时，构建脚本可接收 `-MakensisPath`。NSIS 只是开发期打包器，不会进入安装包。

## GitHub Actions 正式发布

`.github/workflows/release.yml` 在 Windows 2022 和 macOS 14 runner 上并行构建。工作流根据 Tag 自动选择发布渠道：`vX.Y.Z` 使用 `main` 并创建稳定版，`vX.Y.Z-beta.N` 使用 `dev` 并创建 GitHub Pre-release。发布负责人完成版本更新、双平台构建和人工验证后，推送与 `VERSION` 严格一致的标签即可。

Beta 发布示例：

```powershell
git switch dev
# VERSION 和 Cargo workspace 版本均为 1.4.0-beta.1
git tag -a v1.4.0-beta.1 -m "发布 Beta v1.4.0-beta.1"
git push origin dev v1.4.0-beta.1
```

稳定版发布示例：

```powershell
git switch main
# VERSION 和 Cargo workspace 版本均为 1.4.0
git tag -a v1.4.0 -m "发布 v1.4.0"
git push origin main v1.4.0
```

工作流先校验 Tag、`VERSION`、Cargo 版本和来源分支祖先关系，再独立构建 Windows 安装器和 macOS universal DMG。稳定发布设置 `prerelease: false` 和 `make_latest: true`；Beta 发布设置 `prerelease: true` 和 `make_latest: false`。最终 Publish Job 只有在两个平台都成功、四个资产和 SHA-256 全部校验通过后才创建同一个 GitHub Release。Windows 成功而 macOS 失败时，只需重跑 macOS Job；如果代码、版本或打包脚本发生变化，则必须使用新 Commit 和新 Tag 重新构建两个平台。

macOS Job 使用 `MACOS_CODESIGN_IDENTITY=-` 执行 Ad-hoc 签名，不需要配置证书、钥匙串或公证 Secrets。产物可以正常用于公开下载，但首次打开可能触发 Gatekeeper，需要用户通过“右键打开”或“系统设置 → 隐私与安全性 → 仍要打开”。

应用内稳定版更新只查询稳定 Release；Beta 当前不启用应用内自动更新，只从 GitHub Pre-release 页面手动下载。因此不要手动重命名或删除任一资产。`.sha256` 由应用内更新流程自动使用，普通安装不要求用户手动校验。未来启用 Beta 自动更新时，必须先增加明确的 `stable/beta` 更新通道和独立测试，不能直接放开稳定版更新器对 Pre-release 的读取。生产发布使用受保护的 `production` Environment，Beta 使用 `beta` Environment；相关签名证书和 API Key 只能放在受保护的 GitHub Environment 中。

## 安装器行为

安装器仅将 Windows 应用安装到 `%LocalAppData%\Programs\WeChatSendGuard`，创建开始菜单组和已注册卸载项。当前稳定版和 Beta 共用安装目录、卸载项和应用身份，因此 Beta 是覆盖式安装，不支持与稳定版并行安装；Beta 测试人员应使用独立测试环境。若未来面向更多外部用户开放 Beta，应先为 Beta 增加独立安装目录、显示名称、卸载注册项和更新通道。正常卸载不删除 `%LocalAppData%\WeChatSendGuard`；删除设置和审计日志必须由用户做出单独、明确的选择。

## macOS 构建与发布

macOS 构建前安装 Rust 1.92+、Apple Silicon/Intel 两个 target 和 Xcode Command Line Tools，然后运行：

```bash
MACOS_BUILD_KIND=universal ./packaging/macos/build-app.sh
```

脚本先执行格式、Clippy 和全工作区测试，再构建 universal `.app`，启用 Hardened Runtime，使用 Ad-hoc 签名生成 `dist/macos/WeChatSendGuard-<VERSION>-universal.dmg` 及同名 `.sha256`。Beta 的 DMG 文件名保留完整版本，例如 `WeChatSendGuard-1.4.0-beta.1-universal.dmg`；`CFBundleShortVersionString` 使用去除预发布后缀的 `1.4.0`，`CFBundleVersion` 使用纯数字构建号，以满足 Apple Bundle 元数据格式。GitHub Actions 正式发布直接使用同一方式，不需要 Apple Developer 账号或额外 Secrets。`.sha256` 只由应用内更新自动校验，普通用户不需要手动执行命令。

发布门禁还必须记录 macOS 版本、微信版本、bundle/Team 身份、辅助功能与输入监控授权、DMG SHA-256、`codesign --verify --deep --strict` 结果和完整人工清单。Ad-hoc 包未公证，不能把 Gatekeeper 放行或 stapler 成功当作本次发布条件；GitHub Release 中的 DMG 文件名必须与应用内更新选择规则一致。

## 后续平台

Linux 未来必须产出桌面环境认可的独立原生包，不能共享 Windows 安装器或 macOS DMG。三个平台共享的是 `guard-core`、Slint 界面、公共配置语义和平台适配契约，而不是系统 API。
