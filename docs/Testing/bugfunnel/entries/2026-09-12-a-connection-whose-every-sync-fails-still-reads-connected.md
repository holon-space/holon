---
id: 2026-09-12-a-connection-whose-every-sync-fails-still-reads-connected
date: 2026-09-12
gap: ORACLE
secondary: PERCEPTION
status: FIXED
summary: >-
  A connection whose every sync batch has failed since boot paints "Connected"
  with a green dot, indistinguishable from a working one, with no toast, banner
  or status change — only a WARN in the log.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`).

Two introduced connections ran side by side in the same app for twenty minutes.
`fixturebox` failed every single sync (see
`2026-09-12-an-integer-id-column-makes-every-connection-sync-fail-with-datatype-mismatch`);
`fixturetext` synced correctly. On screen they are identical:

| | Config | Status | Sidebar dot | Rows in SQL |
|---|---|---|---|---|
| fixturebox | unconfigured | Connected | green | 0 |
| fixturetext | unconfigured | Connected | green | 2 |

Screenshot `scratchpad/dogfood-uc/shots/04-failing-connected.png`. The backing
projection agrees with the screen — `SELECT id, enabled, status FROM
integration_state` returns `Connected` for both — so this is not a render/SQL
divergence. The stored status is simply wrong.

The only trace of the failure is a WARN repeated every ten seconds in the app
log. No toast appeared, none had appeared earlier and been dismissed, and the
degraded bus that carries the boot-time refusals was not used.

This is the error tier the project's `CLAUDE.md` names as never allowed:
silently degrading to look fine. A user watching this screen concludes the
connection works and that the peer has no data.

## Root cause

Not fully traced by this pass, and the entry says so rather than guessing.

What is measured: `IntegrationStatus` is written at connect time
(`crates/holon-app/src/mcp_integrations.rs` calls `record_status` on the
connect-failure paths, and the projection reports `providers=7 enabled=N`), and
the sync engine's per-poll failure is handled inside
`crates/holon-mcp-client/src/mcp_integration.rs`, which logs
`poll resync failed error=...` at WARN and returns. Nothing on that path writes
back a status or reaches the degraded bus, so the `Connected` recorded at boot
stands unchallenged for the life of the process.

The candidate cause is therefore that connection status is a BOOT fact rather
than a running one. Naming it as the cause would need a measurement tying the
poll failure to the absence of a status write, which this pass did not make.

## Missing piece

ORACLE. The interaction is entirely generatable — an enabled connection polling
a peer is what the feature does — and the failure is loud inside the process.
What does not exist is any invariant of the form "a provider that has failed
every sync since boot must not report Connected", or more generally "the
disclosed status must be consistent with what the runtime last observed". The
`user-connections` lane built careful disclosures for every REFUSAL at load and
none for a failure AFTER connect.

Secondary PERCEPTION: the sidebar's green dot carries the same wrong claim, and
no headless assertion covers the dot.

Keystone repro: not possible today for the same reason as the sync entry — no
connection-sync transition exists. The natural home is a test in `holon-app`
that drives a sidecar against a mock which returns a payload the schema rejects,
then asserts the projected `integration_state.status` is NOT `Connected` and
that a disclosure reached the degraded bus.

## Remedy

FIXED, and along the axis the entry argued for rather than "show red when a poll
fails".

The candidate cause named above holds and is now measured: `Connected` was
written at `crates/holon-app/src/mcp_integrations.rs` immediately after
`request_initial_sync()`, which only ENQUEUES the first batch. So the status was
a statement about the connect, made before any sync had run, and nothing ever
revisited it.

- `SyncHealth` / `SyncHealthSignal`
  (`crates/holon-mcp-client/src/mcp_integration.rs`) publish what the serialized
  sync loop has observed since boot: `Untried`, `Healthy`, `Failing`. The loop
  folds in EVERY batch outcome — initial sync, subscription resync, poll tick —
  through `ResyncSink::health()`.
- The fold is deliberately asymmetric: a success always reaches `Healthy`, and a
  failure only reaches `Failing` if no success has happened since boot. That is
  exactly the property the entry said is expressible without a flap policy — a
  briefly unreachable peer cannot flap the row.
- Two new status words, not one. `SyncFailing` ("Sync failing") is what a
  never-succeeded connection carries, and `Syncing` ("Syncing") is what a
  connected integration carries between connect and its first batch. The second
  exists because leaving such a row at `Pending` would have been a true
  statement that is INDISTINGUISHABLE from the escape
  `integration_state_boot_records_status` guards — a status write that was
  refused and lost. A recorded `Syncing` keeps those apart. That test caught the
  omission on the first full run, which is exactly its job.
- `IntegrationStatus::SyncFailing` is the word the row carries,
  with its glyph in
  `crates/holon-frontend/src/shadow_builders/integration_status.rs` so the
  sidebar dot stops making the same wrong claim. The existing exhaustiveness
  tripwire in `crates/holon-app/tests/integrations_section_seed.rs` caught the
  new variant and now covers it.
- A connected integration that HAS sync entities no longer gets `Connected` at
  connect; a task follows its health and writes the status its batches justify.
  One with no sync entities still earns `Connected` at connect, because there is
  nothing further for it to prove.

The ORACLE gap is closed at TWO levels, because the first level alone was not
enough — a verifier showed that disabling the whole app-side wiring left every
test green, since the loop tests live in `holon-mcp-client` and cannot see the
app. The app-level rung is
`crates/holon-integration-tests/tests/frontend_suite/integration_state_sync_failing.rs`:
a real `TestEnvironment` boot with an introduced `rest` connection against a
loopback mock that holds the response open and then fails it, asserting the
projected `integration_state.status` reads `Syncing` while the first batch is in
flight and exactly `Sync failing` once it has failed. Disabling the connect-loop
branch turns it red with `'failing-rest' reads Some((1, "Connected")) after 60s.
The connection answered (its list tool was called 1 times) and every call failed
with HTTP 500`.

The painted half is pinned separately by
`frontends/gpui/tests/integration_sync_failing_row_windowed.rs` — a real window
in which the `Sync failing` row paints a different glyph from the `Connected`
one and never the unknown `?` marker. Removing the glyph-table line turns it red
with `"Sync failing" painted the unknown marker "?"`. The row model is not the
paint, and this feature had no painted evidence until that rung existed.

Below those, two tests drive the REAL serialized loop
(`sync_loop_gate_debounce_tests` in the same file):
`a_connection_whose_every_sync_fails_never_reads_healthy` and
`one_success_earns_healthy_and_a_later_failure_does_not_flap_it`. Teeth proof:
removing the three `health().record(...)` lines turns both red with "every batch
failed, so the integration has never synced and must not read healthy"; the file
restores byte-identical (sha256
`4a0fe8c923f9c1a3a9e83487df5548b17d9fd8b5e6c32c7169928b2741a39806`).

KNOWN LIMITATION, stated rather than hidden: a TOOLS-ONLY connection — one
declaring no sync entities — still earns `Connected` at connect, and keeps it
even if its first tool call fails. The health signal this fix rests on is fed by
the SYNC loop, and such a connection never enters it; there is no second signal
today for "a dispatched tool call failed". The status is therefore honest about
what it measures (rows landing) and silent about what it does not (one-shot
calls). Closing that needs a per-call outcome signal, which is a different
mechanism, not a wider version of this one.

Left undone on purpose: no degraded-bus disclosure is raised for a failing sync.
The bus carries sticky conditions that each need a named all-clear, and the
status column is the visible signal the entry asked for. A banner would be a
second decision, worth its own ruling.

## Attribution

PRE-EXISTING, not a `user-connections` regression. Verified by reading the tree
at `a5e161c0` (the commit before that lane): every line named above is already
there — `git show a5e161c0:<path>`. What the lane changed is reachability: it
made the files user-supplied, so a shape that had only ever been authored
in-tree became one a user can write.
