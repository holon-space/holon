---
id: 2026-10-12-needs-auth-condition-promises-an-in-session-grant
date: 2026-10-12
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The IntegrationNeedsAuth condition told the user to authorize and declared
  the grant would clear it in-session, but the grant only takes effect after a
  restart.
---

## Bug
Found while dogfooding (honeymoon 1), lane oauth-restart (holon-work#167).
After the user opened the authorization URL and granted, the integration did
not connect and the condition stayed. Ruling OAUTH-dead.b: change the promise
now; an in-session grant is a later feature.

## Root cause
`ConditionKind::IntegrationNeedsAuth` (crates/holon-api/src/condition_bus.rs)
documented its all-clear as "the grant completing, i.e. the provider
connecting", and its profile (condition_profile.rs, `INTEGRATION_NEEDS_AUTH`)
used `AllClear::NextSuccessOf(IntegrationConnect)`. Its detail text only said
"needs authorizing. Open:" with no mention of the restart. Nothing in the
process reconnects after the grant.

## Missing piece
No test pinned what the condition promises, so text and all-clear could claim
behavior the app does not have. The keystone PBT cannot reach it: the OAuth
grant is an external browser action outside the generated alphabet, and the
oracle (condition text vs behavior) did not exist.

## Remedy
The detail text now says to restart Holon after the grant; the profile is
`AllClear::UntilRestart`; the variant doc says the same. Pinned by
`needs_auth_tells_the_user_to_restart_holon_after_the_grant` and
`nothing_in_session_clears_needs_auth` in crates/holon-api/src/condition_detail.rs.
Red log: `docs/Testing/red-logs/2026-10-12-needs-auth-restart-RED.txt`.
