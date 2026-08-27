# Release Process

## Versioning

`VERSION` is the release source of truth and uses SemVer: `MAJOR.MINOR.PATCH`. Every Rust package and Windows installer metadata value must match it. Application version is independent of `schemaVersion`, which remains `2` until a documented settings migration is introduced.

`1.0.0` is the first production release. A release script will reject a package if manifests or installer metadata diverge from `VERSION`.

## Windows Release Gates

1. Run Rust formatting, linting, unit tests, configuration fixtures, and safe fake-platform integration tests.
2. Run `packaging/windows/build-installer.ps1`. It builds a clean `x86_64-pc-windows-msvc` release, invokes NSIS, and runs the metadata and 15 MB size gate.
3. Confirm the output is `dist/windows/WeChatSendGuard-Setup-<VERSION>.exe` and record its SHA-256 and byte size in the release record.
4. Produce SBOM/license review and confirm Slint's official About widget remains reachable from both System Settings and the top-level tray menu.
5. Code-sign the executable and installer using the production certificate.
6. Install, upgrade, and uninstall on a clean Windows VM; verify user settings survive upgrade.
7. Complete the manual Weixin validation record. Real Weixin is not a test automation target.

The build script accepts `-MakensisPath` when NSIS is not on `PATH`. NSIS is a development-only packager; it is not included in the installer.

## Installer Behavior

The installer installs only the Windows application for the current user under `%LocalAppData%\\Programs\\WeChatSendGuard`, creates a Start menu group and a registered uninstaller, and does not delete `%LocalAppData%\\WeChatSendGuard` during a normal uninstall. Removing settings and audit logs requires a separate explicit choice.

## Future Platforms

macOS and Linux releases get separate signed/notarized or package-manager-native artifacts. They never share a Windows installer or binary. Shared behavior is carried by `guard-core`, Slint UI, configuration schema, and the platform adapter contract, not by embedding Windows APIs in common code.
