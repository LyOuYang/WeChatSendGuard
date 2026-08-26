# WeChatSendGuard

Windows tray utility that adds a confirmation step before sending an `Enter`-submitted Weixin group or contact message.

## Safety boundaries

- Reads only Windows UI Automation metadata exposed by the foreground Weixin window: current chat title, group/contact container state, and message-editor focus.
- Does not modify Weixin, inject a DLL, access its database, capture screenshots, read the clipboard, or send network requests. During a confirmation only, it may read the visible draft through UI Automation to display it in the dialog; the draft is never written to settings or audit logs.
- Intercepts the independently configurable main `Enter` and optional numpad `Enter` only while the foreground window is the trusted `Weixin.exe`, the message editor is focused, and the active group or contact matches the active rule mode. `Shift+Enter` and IME candidate confirmation always pass through.
- On confirmation, it rechecks the foreground Weixin window and chat title, restores the message-editor focus, and emits one tagged `Enter` with `SendInput`.

## Requirements

- Windows 10 22H2 or Windows 11 x64
- Weixin desktop at `C:\Program Files\Tencent\Weixin\Weixin.exe`
- A Weixin version exposing these UI Automation controls: `chat_input_field`, `current_chat_name_label`, and `mmui::ChatTitleBarChatRoomView`
- .NET 10 SDK for development

## Run

```powershell
& 'C:\Program Files\dotnet\dotnet.exe' run --project .\src\WeChatSendGuard.App\WeChatSendGuard.App.csproj
```

The first run opens settings. Switch to a Weixin group or contact, then use **加入当前会话** or `Ctrl+Alt+B`; adding, importing, and removing sessions take effect immediately. Hold `Ctrl` or `Shift` to select multiple sessions before using **移除选中项**. Confirmation, input-entry, startup, and log settings take effect only after **保存设置** is clicked. There are two independent lists and rule modes:

- **保护名单模式**: only sessions in the “需要二次确认” list require confirmation.
- **免确认名单模式**: sessions in the “免确认的会话” list send directly; every other recognized Weixin group or contact requires confirmation.

The settings window shows only the list for the active mode. Switching modes changes the visible list and active rule, but does not merge or remove either stored list.

Open the tray menu to pause protection, grant a 1/5/15-minute bypass in protection-list mode, view status, or exit. Startup registration is off by default and can be enabled in settings.

## Build and publish

```powershell
& 'C:\Program Files\dotnet\dotnet.exe' build .\WeChatSendGuard.slnx
& 'C:\Program Files\dotnet\dotnet.exe' publish .\src\WeChatSendGuard.App\WeChatSendGuard.App.csproj -c Release -r win-x64 --self-contained true -p:PublishSingleFile=false -o .\publish\win-x64
```

The published `win-x64` directory is the deliverable: keep all of its files together and run its `WeChatSendGuard.exe`; do not copy the EXE out by itself. Settings and audit metadata are stored locally under `%LocalAppData%\WeChatSendGuard`. Audit entries intentionally contain no message text or chat title. The current day's diagnostic log is `%LocalAppData%\WeChatSendGuard\logs\audit-YYYY-MM-DD.jsonl`. A confirmed send writes `send: injected`; an injection failure also records the safe diagnostic fields `stage`, `input-size`, `sent`, and `win32` so it can be diagnosed without exposing message content.

## Manual smoke test

1. Open a group or contact in Weixin and focus its message editor.
2. Add it to the current mode's list, switch modes, and verify that only the corresponding independent list is shown.
3. Change a confirmation or input-entry setting, close and reopen settings without saving, and verify the previous setting remains active. Then click **保存设置** and verify the new setting persists.
4. Select several sessions with `Ctrl` or `Shift`, remove them together, and verify the current list is updated immediately.
5. Press main `Enter` and numpad `Enter`: a dark confirmation window must appear only when the active rule requires it and the message must remain a draft.
6. Hold the confirm button until it completes: the app must return to the same message editor and send exactly once. A short hold or cancellation must keep the draft. The matching audit log must contain `send: injected`.
7. Check that `Shift+Enter`, IME candidate confirmation, ordinary Weixin chats, and non-Weixin applications behave normally.
8. Switch groups or contacts, close Weixin, or switch to another application while confirmation is open: sending must be cancelled.
9. If sending does not occur, inspect the same attempt's `send-diagnostic` entry in the audit log. Do not send the message text or chat title when sharing that entry.

The MVP deliberately does not intercept clicks on Weixin's Send button.
