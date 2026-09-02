# Windows 与 macOS 发布流程

本项目采用“分支构建、Tag 发布、双平台原子发布”：代码推送只做验证和候选构建；符合规则的 Tag 才能发布 GitHub Release。Windows 和 macOS 必须来自同一个 Tag/Commit，任一平台失败都不发布。

## 先记住

- `VERSION` 是唯一的产品版本来源；Rust workspace、安装器、DMG 和 Tag 必须与它一致。
- 稳定版来自 `main`，Beta 来自 `dev`；稳定 Tag 和 Beta Tag 不能跨分支创建。
- 已发布的版本和 Tag 不移动、不覆盖、不复用。
- 应用版本与配置的 `schemaVersion` 独立；只有新增了实际的配置迁移时，才升级配置架构版本。
- 自动化测试只使用内存假实现，不启动真实微信、不读取草稿、不注入输入，也不访问网络。
- PR、`dev`/`main` 推送只生成带 Commit SHA 的 CI Artifact，不直接创建 GitHub Release。

## 版本规则

版本号是兼容性承诺，不是功能数量或构建次数。格式遵循 [SemVer 2.0.0](https://semver.org/spec/v2.0.0.html)。

- **主版本 `X`**：有不兼容变更时升级。例如配置、审计导出或外部集成无法兼容；取消已有系统支持；普通升级或回滚无法安全完成。`1.4.0` → `2.0.0`。
- **次版本 `Y`**：增加向后兼容的功能、设置、诊断能力或平台支持。旧配置仍可使用。`1.3.2` → `1.4.0`。
- **修订版本 `Z`**：发布向后兼容的 Bug 修复、安全修复、崩溃修复或性能改进。`1.3.2` → `1.3.3`。
- **Beta 序号 `N`**：同一目标版本的候选迭代号，不代表 Bug 数量。内容发生变化就递增：`1.4.0-beta.1` → `1.4.0-beta.2` → `1.4.0`。

当前正式发布只使用以下两种形式：

```text
X.Y.Z
X.Y.Z-beta.N
```

例如，`1.3.2-beta.1` 是 `1.3.2` 的第一个 Beta；修复问题后使用 `1.3.2-beta.2`，验证通过后发布 `1.3.2`。同一 Commit 仅重跑构建不需要改版本；代码或打包脚本变化必须使用新 Commit 和新 Tag。Alpha、RC 和 `+build` 暂不作为公开产品版本；构建号应放在 CI 记录或平台内部元数据中。

Tag 必须严格是 `v<VERSION>`，例如 `VERSION=1.4.0-beta.1` 时只能使用 `v1.4.0-beta.1`。版本变更应在同一个提交中同步更新 `VERSION` 和 Cargo workspace 版本。

## 分支规则

- `dev`：集成和 Beta 分支；`feature/*`、`fix/*` 通过 Pull Request 合入这里。
- `main`：生产稳定分支；只能合入经过验证的代码。
- `release/x.y.z`：只有需要较长人工验证或冻结范围时临时使用，完成后合入 `main` 并同步回 `dev`。
- `hotfix/x.y.z`：从 `main` 修复生产问题；稳定发布后必须回合并到 `dev`。

`main` 和 `dev` 应启用分支保护，要求 Pull Request、CI 和审查者通过。`v*` Tag 应限制创建、更新和删除权限。

## 发布步骤

1. 根据变更影响选择版本号，更新 `VERSION` 和 Cargo workspace 版本。
2. 在 `dev` 发布 Beta，在 `main` 发布稳定版。
3. 完成本地检查和平台人工验证。
4. 创建与版本完全一致的 Tag 并推送。
5. 检查 GitHub Release 的四个资产及 SHA-256；Windows 和 macOS 都成功后才算发布完成。

Beta 示例：

```powershell
git switch dev
# VERSION 和 Cargo workspace 版本均为 1.4.0-beta.1
git tag -a v1.4.0-beta.1 -m "发布 Beta v1.4.0-beta.1"
git push origin dev v1.4.0-beta.1
```

稳定版示例：

```powershell
git switch main
# VERSION 和 Cargo workspace 版本均为 1.4.0
git tag -a v1.4.0 -m "发布 v1.4.0"
git push origin main v1.4.0
```

GitHub Actions 会校验 Tag、版本、来源分支和祖先关系，然后并行构建 Windows 安装器与 macOS Universal DMG。只有两个平台都成功，且四个文件及其校验和都通过检查，才创建同一个 Release；稳定版标为 Latest，Beta 标为 Pre-release。手动运行工作流也会进入发布流程，只能传入已有的稳定版或 Beta Tag，并在运行前确认目标 Tag、权限和发布环境。

## 发布前检查

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
.\packaging\windows\build-installer.ps1
```

Windows 需要 Rust 1.92+、MSVC 工具链和 NSIS 3.x。打包脚本构建 `x86_64-pc-windows-msvc` 版本，校验安装器元数据和 15 MB 上限，并生成 SHA-256 文件。NSIS 不在 `PATH` 时可传入完整路径：

```powershell
.\packaging\windows\build-installer.ps1 -MakensisPath 'C:\path\to\makensis.exe'
```

Windows 产物为 `dist/windows/WeChatSendGuard-Setup-<VERSION>.exe` 及同名 `.sha256`；安装包上限为 15 MB。macOS 使用以下命令构建 Universal DMG：

```bash
MACOS_BUILD_KIND=universal ./packaging/macos/build-app.sh
```

macOS 构建前需要 Rust 1.92+、`aarch64-apple-darwin` 和 `x86_64-apple-darwin` targets 及 Xcode Command Line Tools。构建使用 Hardened Runtime 和 Ad-hoc 签名，不需要 Apple Developer 账号或公证。产物为 `dist/macos/WeChatSendGuard-<VERSION>-universal.dmg` 及同名 `.sha256`；Beta 文件名保留完整预发布版本。Apple 的 `CFBundleShortVersionString` 使用去掉预发布后缀的三段版本，`CFBundleVersion` 使用纯数字构建号。

发布负责人还必须完成：

- SBOM 和许可证审查。
- 确认 Slint About 窗口可从“通用设置”和托盘打开。
- 干净 Windows 虚拟机中的安装、应用内升级、卸载和设置保留验证。
- Beta 的升级、降级和稳定版隔离验证。
- [人工微信验证清单](manual-wechat-validation.md)。真实微信兼容性不是自动化测试目标。
- 记录安装包和 DMG 的 SHA-256、文件大小，以及 macOS 版本、微信版本、bundle/Team 身份、授权状态和 `codesign --verify --deep --strict` 结果。

## 更新与安装行为

稳定版应用内更新只查询稳定 Release；Beta 当前只能从 GitHub Pre-release 页面手动安装，不会进入稳定版更新入口。未来若启用 Beta 自动更新，必须先增加明确的 `stable/beta` 更新通道和独立测试，不能直接让稳定版更新器读取 Pre-release。不要手动重命名或删除发布资产，`.sha256` 由应用内更新流程自动使用。

安装器将应用安装到 `%LocalAppData%\Programs\WeChatSendGuard`，并创建开始菜单和卸载项。稳定版和 Beta 当前共用安装目录、卸载项和应用身份，因此 Beta 是覆盖式安装，不支持与稳定版并行安装。若未来面向更多外部用户开放 Beta，应先增加独立安装目录、显示名称、卸载注册项和更新通道。卸载不会删除 `%LocalAppData%\WeChatSendGuard`；删除设置和审计日志必须由用户单独确认。

macOS Ad-hoc 包未公证，首次打开可能需要用户通过“右键打开”或系统设置中的安全提示放行。生产环境的签名证书、API Key 和发布权限只能放在受保护的 GitHub Environment 中；稳定版使用 `production`，Beta 使用 `beta`。

## 后续平台

Linux 未来必须提供独立的原生桌面包，不能复用 Windows 安装器或 macOS DMG。各平台共享 `guard-core`、Slint 界面、配置语义和平台适配契约，不共享系统 API。
