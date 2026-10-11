---
id: 2026-10-11-outdent-of-journal-page-child-is-modelled-refusal-but-logs-error-keystone-red
date: 2026-10-11
gap: FALSE-ALARM
secondary: null
status: OPEN
summary: >-
  Under a Loro+Turso (Cell leg) draw the keystone generates Outdent on a direct child of a
  journal day page; the op engine refuses it (ADR 0028 D1), the model treats it as a no-op, but
  the refusal is logged at ERROR so inv-no-errors / inv-no-observed-errors go red.
---

## Bug
`just keystone-smoke` (`pbt general 1`) went red on the places-exp2 tip with
`inv-no-errors` / `inv-no-observed-errors`; ERROR
"shape gate: `block.outdent` could not be simulated, so its result cannot be judged: Cannot
outdent a direct child of a page: block block:gen-1 would escape its page container (ADR 0028 D1)".
`scripts/keystone-known-reds.sh` classified it `[novel]`. Shrunk case:
`JumpToSearchHit(query "2026-01-16", hit block:368857d2-b6f5-4a8d-5c03-a6daedd2100f)` ->
`CreateBlockUnderFocus(content "a", id block:gen-1)` -> `Outdent(block:gen-1)`,
wiring storage={Loro,Turso} actors={MCPServer} editor-leg=Cell.

Attribution (A/B, hand-authored replay of the shrunk case, `HOLON_HAND_AUTHORED_CASE`):
red 3/3 on main 1f3c1e46 and red 3/3 on places-exp2 5b2d780b. It is NOT caused by places-exp2.
The same sequence under storage={Turso} (dispatch floor) is green 3/3 on both trees.
Logs: scratchpad `replay-base-ab-outdent-journal-child-loro-{1,2,3}.log`,
`replay-exp-ab-outdent-journal-child-loro-{1,2,3}.log`.

## Root cause
- `outdent_preconditions` (crates/holon-integration-tests/src/pbt/transitions/outdent.rs:50-82)
  only requires `grandparent(block).is_some()`. A journal day page has a parent (`journals`), so a
  direct child of it passes; a top-level seed page such as `block:structural-page` has no
  grandparent and is excluded, which is why the draw rarely reaches this.
- The op engine refuses it: `outdent` in crates/holon-core/src/traits.rs:1574-1603
  (`forbidden_at_page_boundary`, `is_page_authoritative(parent_id)`), wrapped by the shape gate
  at crates/holon-core/src/shape_gate.rs:305 ("could not be simulated").
- The refusal is intended prod behaviour (toast) and `outdent_apply_to_ref`
  (outdent.rs:105-125) models it as a no-op; the dispatch floors swallow it
  (`is_page_boundary_outdent_refusal`, crates/holon-integration-tests/src/pbt/op_write_cap.rs:54;
  mutation_driver.rs:125-140). The Cell-leg keystroke path goes through
  `dispatch_intent_awaitable`, which logs `tracing::error!` on every Err
  (crates/holon-frontend/src/reactive.rs:4580-4583) and calls `record_error()`. The test scope
  collects every ERROR event (crates/holon-integration-tests/src/test_tracing.rs:110-112), so the
  modelled refusal reds `inv-no-observed-errors` and `inv-no-errors`. No allowlist covers it.

## Missing piece
The model and the invariants disagree about one documented refusal: the transition model says
"refused, no-op", the error-log invariants say "any ERROR is a bug". No known-reds row exists.

## Remedy
OPEN. Two options for the owner: (a) add `parent is not a page` to `outdent_preconditions`
(prod hides the op at the page boundary via `boundary_behavior`, so the generator should not
offer it; keep the refusal case as a separate hand-authored no-op lock); or (b) make the
disclosed page-boundary refusal an expected problem in the observed-errors window. (a) is
simpler. Pin with a hand-authored case: the shrunk sequence above (Loro+Turso+MCPServer, Cell leg).
