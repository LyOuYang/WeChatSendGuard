# Manual Weixin Validation

## Preconditions

- Use a dedicated test account and a private test group or saved-message target with no unintended recipients.
- Record Windows version, Weixin version, application version, installer hash, and test date.
- Start from a fresh application launch after installing the candidate package.
- Do not run automated test tooling while this checklist is being performed.

## Settings and Tray

1. Verify first launch opens the settings window and `--silent` starts in the tray.
2. Verify protected-list and exemption-list modes retain independent lists when switching modes.
3. Add a current group and a current contact manually; verify their target-kind badges and aliases.
4. Change a non-list setting, close settings without saving, reopen it, and verify the prior active setting remains. Save it and verify persistence after restart.
5. Toggle protection from both the main switch and the tray. Verify it takes effect without selecting Save, remains correct after restart, and the tray shows both the current state and the next action (`暂停` or `启用`).
6. Minimize the settings window, click the tray icon, and verify the existing settings window is restored rather than remaining minimized.
7. Toggle temporary bypass from the tray. Verify bypass choices are unavailable or rejected outside protected-list mode.

## Sending Protection

1. Focus a protected group editor with a harmless unsent draft. Press main `Enter`; verify the draft remains unsent, a confirmation opens centered over the Weixin window, and the confirmation receives focus.
2. Repeat with numpad `Enter` when enabled, then disable it and verify it passes through according to the client behavior.
3. Verify `Shift+Enter` and IME candidate confirmation do not open the guard dialog.
4. In hold mode, release early, leave the button, press Escape, click cancel, and let the timeout expire. Each case must leave the draft unsent; Escape must close the confirmation instead of affecting or minimizing Weixin.
5. Complete a hold. Verify the dialog closes, the original editor is still the target, and exactly one message is sent.
6. While the dialog is visible, switch to a different chat or window, then confirm. Verify no message is injected.
7. Verify click and phrase modes, including phrase mismatch and phrase-enter confirmation.
8. Verify unknown or incompatible chat context follows the configured safe behavior and never injects when identity cannot be revalidated.

## Privacy and Recovery

1. Inspect settings and audit files. Confirm no draft text or chat title appears in audit lines.
2. Close Weixin, restart the application, and verify the tray reports unavailable rather than failing or affecting unrelated apps.
3. Test the candidate when Weixin is elevated and the guard is not. Verify it fails closed and communicates that it cannot protect the client.
4. Uninstall and reinstall an upgrade candidate. Verify `%LocalAppData%\\WeChatSendGuard` settings and logs are preserved unless the user explicitly removes them.

## Sign-off Record

Store the completed checklist with the release record. Any failed or skipped row blocks a compatibility claim for that version.
