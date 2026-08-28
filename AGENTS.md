# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 workspace for a Windows 10/11 WeChat send-confirmation utility. Keep dependencies flowing toward platform-neutral code:

- `crates/guard-core`: rules, settings, confirmation state, and audit models; no UI or OS APIs.
- `crates/platform-api`: platform interfaces and in-memory test doubles.
- `crates/guard-service`: application orchestration across core rules and platform interfaces.
- `crates/desktop-ui`: Slint UI, assets, and view-state code.
- `crates/platform-windows`: Win32, UI Automation, keyboard hooks, tray, and Windows services.
- `crates/desktop-app`: Windows composition root and executable entry point.
- `docs/` contains architecture, security, testing, and release contracts; `packaging/windows/` builds the NSIS installer.

Do not introduce Windows-specific code into `guard-core`, `guard-service`, or `desktop-ui`.

## Build, Test, and Development Commands

Run these from the repository root:

```powershell
cargo fmt --all -- --check                 # verify Rust formatting
cargo clippy --workspace --all-targets -- -D warnings  # lint as errors
cargo test --workspace                     # run isolated automated tests
cargo run -p wechat-send-guard             # launch the desktop application
.\packaging\windows\build-installer.ps1   # build and verify the x64 installer
```

The installer requires Rust 1.88+, the Windows MSVC toolchain, and NSIS 3.x. `VERSION` is the single source of the product version; keep package and installer metadata aligned with it.

## Coding Style & Naming Conventions

Use `rustfmt` defaults (four spaces) and keep Clippy warning-free. Follow existing Rust naming: `PascalCase` types and enum variants, `snake_case` functions, modules, and fields, and descriptive test names such as `protected_list_matches_groups_and_contacts_without_cross_matching`. Keep UI text in the Slint view and express OS capabilities through `platform-api` traits rather than cross-layer calls.

## Testing Guidelines

Place unit tests beside code and integration tests under each crate's `tests/` directory; store stable JSON samples in `tests/fixtures/`. Automated tests must use the in-memory platform doubles. Never start WeChat, enumerate its windows, attach UI Automation, inject input, use accounts, access the network, or read real drafts. Real-client compatibility is a manual release check; follow `docs/manual-wechat-validation.md`.

## Commits & Pull Requests

Recent commits use short, focused Chinese summaries, sometimes with a `fix` prefix (for example, `fix：无法获取微信联系人bug`). Keep that concise style and make each commit one logical change. Pull requests should explain the behavioral and security impact, list validation commands run, link relevant issues, and include screenshots for Slint UI changes. Flag changes to settings schemas, trusted executable handling, keyboard interception, or audit data for explicit review.
