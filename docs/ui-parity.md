# Windows UI Parity Baseline

## Source of Truth

The existing WPF application is the Windows v1 visual and interaction baseline. This document freezes its user-visible contract for the Slint migration. When an older design note conflicts with the running WPF implementation, the running implementation wins.

## Settings Window

- Frameless compact desktop window with custom minimize and close controls.
- Main window starts at 920 x 580 logical pixels, has an 8 px resize border, a draggable 40 px caption area, and an inset 8 px floating shell with a restrained border and shadow. Close hides to the tray; minimize uses the normal Windows minimized state and a tray click restores it.
- Left navigation with `发送守护`, `规则名单`, and `系统设置` sections.
- Top status area reports protection state and current Weixin recognition status before configuration detail.
- Guard view includes enable switch, two rule-mode cards, active-mode list, aliases, add-current-session action, import/export, multi-select removal, and immediate-effect status messaging. The enable switch persists and takes effect immediately instead of entering the unsaved queue.
- Rules view exposes confirmation mode, hold duration, phrase, timeout, main Enter, numpad Enter, and fixed Shift+Enter pass-through behavior.
- System view exposes unknown-context behavior, start-with-Windows, and log-retention days.
- List operations take effect immediately. Other changed controls, except the enable switch, show an unsaved state until the user selects the primary save command. The tray displays the current guard state and a matching `暂停` or `启用` action.

## Confirmation Window

- Separate compact window with target-kind badge, target name, optional ephemeral draft preview, and clear cancel-default behavior.
- Frameless, always-on-top confirmation card starts at 460 px wide, is centered over the original trusted target window when its bounds are available, and presents an `Esc 取消` affordance without a native title bar.
- `Escape` cancels and is consumed by the guard while confirmation is active, even if Windows has not yet transferred foreground focus. Phrase mode initially focuses the phrase input; other modes initially focus cancel.
- Hold mode shows live progress, resets if the pointer leaves, and pauses/extends the cancellation countdown during a held attempt, matching current behavior.
- A target change, process change, lost editor focus at final revalidation, cancellation, or timeout must never result in key injection.

## Visual Tokens

| Token | Value |
| --- | --- |
| Window background | `#FFFFFF` |
| Sidebar background | `#F5F7F9` |
| Ink | `#111827` |
| Muted text | `#6B7280` |
| Border | `#E2E8F0` |
| Weixin green | `#07C160` |
| Green hover | `#06AD56` |
| Green pressed | `#059B4C` |
| Soft green | `#E8F8F0` |
| Warning | `#D97706` |
| Warning soft | `#FEF3C7` |
| Danger | `#DC2626` |

Use `Segoe UI`, `Microsoft YaHei UI`, `PingFang SC`, and the system sans-serif fallback chain. Cards use 8 px corners; ordinary controls use 6 px corners; primary controls are compact rather than marketing-style. The interface must stay quiet, dense, and work-focused.

## Accessibility and Regression Checks

The Slint implementation must preserve keyboard traversal, focus visibility, disabled states, text fit at 100% and 150% Windows scale, high-contrast legibility, and the cancellation-first confirmation flow. Screenshot comparisons are part of the UI review, but no screenshot or automation may inspect a real Weixin conversation.
