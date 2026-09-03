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

The installer requires Rust 1.92+, the Windows MSVC toolchain, and NSIS 3.x. `VERSION` is the single source of the product version; keep package and installer metadata aligned with it.

## Versioning and Release Rules

- Treat `VERSION` and the Cargo workspace version as the public product version, not a local compile counter. Local Debug builds, `cargo run`, and ordinary CI builds must not increment `VERSION`.
- Use a separate build identifier for local and CI output, such as `local.<UTC date>.<short SHA>` or `ci.<run ID>.<short SHA>`. Build identifiers are for traceability and must not be used as update precedence.
- Before answering which version should be released, check the remote GitHub Releases and Tags, the current branch, the local `VERSION`, and whether any higher candidate was distributed to testers. Do not infer a public Beta number from local commits or prior AI builds.
- For a target whose highest published version is `X.Y.Z-beta.N`, the next public Beta is `X.Y.Z-beta.(N+1)` only when higher candidates were not distributed. A candidate that was distributed to testers consumes its number even if it was never uploaded to GitHub.
- Never reuse, move, overwrite, or retag a public version. A changed binary, resource, installer, or packaging script needs a new unique distributed build; a same-Commit rebuild does not need a product-version bump before publication.
- Keep public versions in the repository-supported forms `X.Y.Z` and `X.Y.Z-beta.N`. Do not publish local `-dev` versions or `+build` metadata as public release Tags.
- Update `VERSION` and the Cargo workspace version in the same Commit before creating the exact `v<VERSION>` Tag. Stable releases come from `main`; Beta releases come from `dev`.
- `dev` is Beta-only and may publish only `vX.Y.Z-beta.N`; `main` is stable-only and may publish only `vX.Y.Z`. Reject any other branch/version combination before starting release work.
- When a user asks to release, first audit remote Releases/Tags, the current branch, local `VERSION`, Cargo workspace version, and tester-distribution history. Then report the inferred version, evidence, reason, target branch, and expected Tag for review.
- The audit phase must not push code, create or push Tags, start a tag-triggered build, or manually dispatch the release workflow. Only after the user explicitly approves the proposed version may the agent update version files, commit, and push to trigger the build.
- If the user has not explicitly approved the proposed version, or the version/distribution evidence is incomplete, stop before any push or release build.
- Every GitHub Actions Release must include a concise, reviewed summary of the actual changes since the previous public Tag. Do not rely only on auto-generated commit lists or mention local-only builds.
- Use the fixed release-note format in `docs/release.md`: separate `Windows` and `macOS` sections, use short bullets, include platform-specific compatibility/validation, and finish with `已知问题`. Omit inapplicable bullets and never claim unverified compatibility.
- Store the reviewed Release body at `docs/release-notes/<VERSION>.md` in the tagged Commit. The release workflow must validate that file and use it as the GitHub Release body; a release without the fixed sections is invalid.
- Local installer tests must use a disposable VM or a separate Local installation identity. Do not assume that a different displayed version isolates the shared Windows install directory and uninstall registration.

## Coding Style & Naming Conventions

Use `rustfmt` defaults (four spaces) and keep Clippy warning-free. Follow existing Rust naming: `PascalCase` types and enum variants, `snake_case` functions, modules, and fields, and descriptive test names such as `protected_list_matches_groups_and_contacts_without_cross_matching`. Keep UI text in the Slint view and express OS capabilities through `platform-api` traits rather than cross-layer calls.

## Testing Guidelines

Place unit tests beside code and integration tests under each crate's `tests/` directory; store stable JSON samples in `tests/fixtures/`. Automated tests must use the in-memory platform doubles. Never start WeChat, enumerate its windows, attach UI Automation, inject input, use accounts, access the network, or read real drafts. Real-client compatibility is a manual release check; follow `docs/manual-wechat-validation.md`.

## Logging and Diagnostics

Every user-facing workflow, state transition, platform recognition decision, and failure or early-return path must emit a structured audit entry with a stable `eventType` and `result`. Include an opaque `traceId` for actions that may need to be correlated, plus the smallest content-free state needed to distinguish configuration, context, timing, and platform failures. When investigating a user report, inspect the actual logs first, correlate version, `sessionId`, `processId`, `traceId`, and timestamps, and do not rely on code-path guesses alone.

Logs must never contain message text, draft text, chat titles, clipboard data, screenshots, or full user paths. New diagnostic fields must be added to the platform audit allow-list and covered by tests so useful state is retained without weakening the privacy boundary. Add automated assertions for the important success, rejection, expiry, and recovery log branches of each new workflow.

## Commits & Pull Requests

Recent commits use short, focused Chinese summaries, sometimes with a `fix` prefix (for example, `fix：无法获取微信联系人bug`). Keep that concise style and make each commit one logical change. Pull requests should explain the behavioral and security impact, list validation commands run, link relevant issues, and include screenshots for Slint UI changes. Flag changes to settings schemas, trusted executable handling, keyboard interception, or audit data for explicit review.

## logs

1. Log Key Functions: Core features must include troubleshooting logs to ensure user-reported issues are traceable.
2. Prioritize log analysis when errors occur. First, clarify whether the issue requires local or external logs; if external, request the specific log directory. Proactively ask for details such as the time of occurrence and symptoms to aid diagnosis.
3. Supplement Missing Logs: If existing logs cannot identify the issue, they must be added concurrently during the fix.
