# Security and Privacy Boundaries

- The application observes only the foreground target client and only the accessibility metadata needed for title, target kind, and editor focus.
- Trust is based on a deterministic target-client identity, not a broad process-name match.
- A failed lookup, stale element, elevation mismatch, unsupported client layout, or revalidation mismatch cannot inject a send key.
- Synthetic input is marker-tagged so the hook does not process its own event as physical input.
- A draft preview is optional, in-memory, and scoped to one confirmation window. It is never persisted or logged.
- Audit records are deliberately minimal and have bounded retention.
- Settings and audit data remain in the current user's local application-data directory.
- The application does not use a network service, telemetry pipeline, client database access, code injection, screen capture, clipboard capture, or remote control.
