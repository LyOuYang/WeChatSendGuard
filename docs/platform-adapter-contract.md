# Platform Adapter Contract

## Purpose

This document is the handoff boundary for a later macOS or Linux implementation. It describes what must remain compatible with Windows v1; it does not authorize or prescribe development of those implementations now.

## Stable Shared Contracts

An adapter must preserve all of the following:

- `guard-core` rule evaluation and confirmation state-machine behavior;
- settings file location conventions for its platform while preserving the same JSON shape and `schemaVersion: 2`;
- protected and exemption list semantics, including group/contact separation and normalized title matching;
- confirmation modes: click, hold, phrase, cancellation, timeout, and stale-target rejection;
- privacy limits and audit schema;
- UI language, hierarchy, status meaning, and keyboard accessibility defined in [ui-parity.md](ui-parity.md).

An adapter must not share code by placing `cfg` branches inside the domain layer. OS-specific implementation belongs in a platform crate or package.

## Required Capabilities

The future adapter exposes the following capabilities to `desktop-app` through `platform-api`:

| Capability | Required behavior |
| --- | --- |
| Foreground context | Return a snapshot containing app trust, process identity, native window identifier, supported target kind, normalized title source, editor focus, and observation generation. |
| Input gate | Observe candidate send keys and suppress only a decision explicitly marked for suppression. Callback work must use cached state and return promptly. |
| Input injection | Emit one tagged send key only after core revalidation succeeds. It must be distinguishable from physical input. |
| Confirmation ownership | Identify confirmation windows so they are not mistaken for the protected target. |
| Confirmation presentation | Center the confirmation over the pending native target when bounds are available, request focus best-effort, and consume `Escape` while confirmation is pending so it cannot reach the protected target. |
| Tray and lifecycle | Show and restore settings from a minimized or hidden state, show the actual protection state plus the next toggle action, offer temporary bypass only where valid, and terminate cleanly. |
| Startup registration | Use a per-user mechanism and never require administrator rights. |
| Settings and audit storage | Persist atomically in a user-private application directory with bounded retention. |

## Trust Model

The adapter must define a deterministic trust rule for the target application. A process name alone is insufficient. On Windows v1, trust is an exact canonical executable path. A future adapter may use a signed bundle identifier, sandbox identity, or another platform-native proof, but must document it and reject ambiguity.

If the target client exposes no stable, read-only accessibility metadata for current chat identity and editor focus, the adapter cannot claim send protection support. It may surface an unavailable state, but must not infer a target from text capture, screenshots, private databases, injected code, or memory inspection.

## macOS Notes

macOS implementation work must use Accessibility APIs and input-monitoring permissions with a clear consent flow. It must perform a final focused-element and target revalidation immediately before a synthetic key event. A future macOS app must document notarization, hardened runtime, accessibility permission recovery, and bundle-signing behavior.

## Linux Notes

Linux implementation work must distinguish X11 from Wayland. X11 can offer global input observation subject to desktop-environment behavior. Wayland often intentionally forbids global hooks and synthetic input; a Linux adapter must not claim support unless it has a documented, desktop-environment-approved integration. Unsupported environments should show an unavailable status rather than degrade protection silently.

## Platform Test Rule

Automated tests use fake context providers and recording injectors only. A real Weixin client, a real user account, chat data, and external recipients are never part of CI or scripted tests. Platform compatibility is verified through the manual protocol in [manual-wechat-validation.md](manual-wechat-validation.md).
