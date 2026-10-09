---
id: 2026-10-09-opening-settings-freezes-the-window-behind-a-keychain-prompt
date: 2026-10-09
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Opening the Settings modal in a freshly built holon-gpui freezes the window:
  the render reads every secret preference from the login keychain on the main
  thread, and macOS asks for the keychain password before it answers.
---

## Bug

Lane `crash-history`, round 3, during `just live-verify 8730` (throwaway config
and vault). A click on the previous-run toast's "Show crash history" opened
Settings. The next MCP `click` never answered, and a macOS dialog asked for the
login keychain password for "holon-integrations". The window stayed frozen
until the process was killed with SIGKILL (SIGTERM was not handled). The dialog
stayed on screen after the process died.

## Root cause

The main thread was blocked in the Settings render (lane-logs/r3-live-sample.txt
in the crash-history lane):

- `frontends/gpui/src/lib.rs:1238` — the Settings overlay calls
  `FrontendSession::preferences_render_data` on every frame it is open.
- `crates/holon-frontend/src/lib.rs:894-900` — `stored_secret_keys` calls
  `KeychainStore::load` for every secret preference.
- `crates/holon-secrets/src/mac.rs:35` — `MacKeychainStore::load` is a
  synchronous keychain read. A new debug or release binary has a new code
  identity, so the keychain item's ACL asks again, and the call blocks until
  the person answers.

The throwaway config dir does not isolate the keychain: production `main`
grants the login keychain, so a live-verify instance reads the developer's real
"holon-integrations" items.

## Missing piece

Every test injects an in-memory secret store (`FrontendSession::use_secret_store`),
so no test runs the real keychain read, and none runs it on the render path.
Same class as 2026-10-06-release-boot-stalls-behind-integration-keychain-prompt
(a synchronous keychain read that a prompt blocks), here on the UI thread.

## Remedy

Open. Read the stored-secret flags off the main thread, once, and render
"checking keychain" until they arrive; give live-verify an opt-out from the
login keychain so an agent's instance never prompts the developer.
