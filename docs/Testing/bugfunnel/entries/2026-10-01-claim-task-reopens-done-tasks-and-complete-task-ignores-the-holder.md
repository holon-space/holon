---
id: 2026-10-01-claim-task-reopens-done-tasks-and-complete-task-ignores-the-holder
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  MCP `claim_task` sets DOING whatever the state (re-opening a DONE task), a
  lost race can leave mixed `claimed-at`/`claimed-from`/`task_state` values,
  and `complete_task` does not check who holds the claim.
---

## Bug
Found by reading, not reproduced: read-only spike SP6 section 3
(`scoped-net/sp6-human-agent.md` in the session scratchpad).

## Root cause
By reading, not reproduced. `claim_task` (`frontends/mcp/src/tools.rs:2160`)
refuses only when `assigned-to` holds another id; it never reads `task_state`.
It then issues four separate `set_field` dispatches (`assigned-to`,
`claimed-at`, `claimed-from`, `task_state = DOING`), sleeps 1 s and re-reads
`assigned-to`. A DONE task with no assignee is re-opened. Two interleaving
claimers each write four fields; the loser reports `lost-race` but its
`claimed-at`, `claimed-from` and `task_state` writes can be the last ones, and
nothing rolls them back. `complete_task` (`tools.rs:2403`) writes `task_state =
DONE` and `completed-at` for any caller. ADR 0032 section 5 names the assignee
claim as its example of a leased, loud-on-loss transition.

## Missing piece
No transition draws `claim_task`/`complete_task` against a DONE task, two
concurrent claimers, or a non-holder completer; no invariant says a claim
leaves one holder's values only.

## Remedy
OPEN. Refuse a claim on a DONE (or non-claimable) state, refuse `complete_task`
from a non-holder, and write the claim as one atomic change or a lease. Add the
keystone transitions first.
