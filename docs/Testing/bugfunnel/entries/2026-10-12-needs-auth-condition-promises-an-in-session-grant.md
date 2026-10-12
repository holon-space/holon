---
id: 2026-10-12-needs-auth-condition-promises-an-in-session-grant
date: 2026-10-12
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The IntegrationNeedsAuth condition told the user to authorize and promised
  the grant would clear it, but no OAuth grant can complete in this build.
---

## Bug
Found while dogfooding (honeymoon 1), lane oauth-restart (holon-work#167).
After the user opened the authorization URL and granted, the integration did
not connect and the condition stayed.

## Root cause
`ConditionKind::IntegrationNeedsAuth` (crates/holon-api/src/condition_bus.rs)
documented its all-clear as "the grant completing, i.e. the provider
connecting", its profile used `AllClear::NextSuccessOf(IntegrationConnect)`,
and its text said "needs authorizing. Open:". None of that can happen: the MCP
OAuth grant cannot complete in this build. No handler exists for
`holon://oauth/callback`, and `complete_oauth` has no caller. A restart does not
help either, because the grant is never stored.

## Missing piece
No test pinned what the condition promises, so text and all-clear could claim
behavior the app does not have. The keystone PBT cannot reach it: the OAuth
grant is an external browser action outside the generated alphabet, and the
oracle (condition text vs behavior) did not exist.

## Remedy so far (the promise only)
The text now says the sign-in cannot be completed in this build and names the
way out: switch the integration off in Settings > Integrations. The profile's
all-clear is `NextSuccessOf(IntegrationSwitchedOff)`; the supervisor clears the
condition when the integration's enablement turns off. Pinned by
`needs_auth_says_the_sign_in_cannot_complete_and_names_the_off_switch` and
`switching_the_integration_off_clears_needs_auth` (holon-api) and
`switching_a_needs_auth_integration_off_clears_its_condition` (holon-app).
Red log: `docs/Testing/red-logs/2026-10-12-needs-auth-restart-RED.txt` (the
first, restart-based text; superseded by the ruling above).

## Still open
The real gap stays: no handler for `holon://oauth/callback`, and
`complete_oauth` has no caller, so an MCP integration that needs OAuth cannot
connect. The in-session grant is deferred to holon-space/holon-work#169. Move
this entry to FIXED when that lands and the text and all-clear can promise it.
