# Test Policy

## Automated Tests

Automated tests cover deterministic logic without connecting to Weixin:

- rule matching and unknown-context decisions;
- settings migration, sanitization, import/export, and atomic persistence behavior;
- pending-confirmation lifecycle and stale-target rejection;
- UI view-model state, confirmation timing behavior, screen-coordinate drag math, and popup-centering math;
- guarded `Escape` classification and suppression bookkeeping without installing a hook;
- Windows adapter structure using `FakeChatContextProvider` and `RecordingInputInjector`.

The test target must not launch Weixin, enumerate Weixin windows, attach UI Automation to Weixin, send input to Weixin, read a real message draft, use an account, or make network requests.

Debug-only `--ui-preview`, `--ui-snapshot`, and `--confirmation-snapshot` modes construct only fixed demo settings and demo text. They return before configuration loading, Weixin context monitoring, keyboard hook installation, audit logging, tray setup, or input injection. Snapshot images therefore cannot include a real chat title or draft.

## Safe Test Doubles

`platform-api` provides explicit test doubles. The fake context provider returns only test-created snapshots. The recording injector records a requested key in memory and has no Win32 call path. These doubles are the only integration target used by automated state-machine and UI tests.

## Manual Compatibility Validation

Real Weixin validation is intentionally manual and performed only by the release owner using the isolated checklist in [manual-wechat-validation.md](manual-wechat-validation.md). A release may state compatibility only after that checklist is completed for the declared Windows and Weixin versions.
