---
id: 2026-09-17-recursive-cte-receive-failed-not-reproduced
date: 2026-09-17
gap: FALSE-ALARM
secondary: null
status: NOTED
summary: >-
  "Recursive CTEs over `block` reliably fail with `Receive failed`" is not a
  defect at all: the engine answers a `WITH RECURSIVE` document walk correctly
  through both `execute_raw_sql` and `execute_query`, and the transport error
  belongs to an earlier call in the session whose reply was never written.
---

## Bug

Reported by the vault-spine-dedupe lane (queue 2026-09-17, item 53b):
"recursive CTEs over `block` via execute_raw_sql/execute_query reliably fail
with `Receive failed: no pending response`". Lane: `fix-mcp-tool-defects`,
base `bd8b719a103b`.

## Verdict: NOT REPRODUCED as a CTE defect

`mod query_survival_tests` (`frontends/mcp/src/tools.rs`) drives both tools
against a real engine over a seeded three-level chain
(`block:root → block:child → block:grandchild`) with the doc-subtree walk
`WITH RECURSIVE d(id, parent_id, depth) AS (… UNION ALL …) SELECT … FROM d`.
Both answer with all three rows and depths 0/1/2
(`lane-logs/11-nextest-full.log`). The handler future COMPLETES under
`catch_unwind`, so no panic exists on that path at all.

The hypothesis that the fork cannot run recursive CTEs is falsified: the walk
returns exactly the rows a naive expansion would, and the store's own
document reads are built on the same recursive shape
(`crates/holon-turso/src/turso.rs:4308`).

## Root cause of the reported symptom

A call whose handler panics never gets a reply written, and the client reports
exactly `Receive failed: no pending response` for it — the same string the CTE
calls were reported with. The lane's own record lists the `render_org`
absolute-path attempts with that error BEFORE the CTE work
(`…/handoffs/holon-2026-09-15/vault-task-audit-2026-09-17.md:311`), so an
unanswered request is a sufficient explanation for the text. See
`2026-09-17-render-org-doc-id-panic-kills-the-mcp-server`.

An earlier revision attributed this to a dead server; a verifier refuted that
for a debug build — rmcp spawns each handler in its own task, so the process
survives and only that request loses its reply. The correction does not change
this entry's verdict: nothing here depends on whether the process died, only on
whether the CTE calls could have produced that error themselves, and they
cannot.

Not established: that this was the lane's actual sequence. The app-side log
for that session was not in reach from this lane, so this entry claims only
what is measured — the CTE path is healthy, and an unanswered request is a
sufficient explanation for the error text.

## Missing piece

No escape: the CTE path has no defect, which is what `gap: FALSE-ALARM` records
(the taxonomy's bucket for a failure with no product defect behind it, kept out
of the four-class escape distribution). What the report DID expose is a
diagnosis gap — `Receive failed: no pending response` names neither the request
that went unanswered nor the panic that swallowed it, so a healthy path was
blamed for a session-wide symptom. Distinguishing them from inside the harness
needs a stdio round-trip rung (spawn the `holon-mcp` binary, drive it over the
protocol, assert every request is answered).

## Remedy

None needed for the CTE path. The panic that did kill the server is fixed
(`2026-09-17-render-org-doc-id-panic-kills-the-mcp-server`). The residual —
a panic in any OTHER handler still ends the process — is recorded there.
