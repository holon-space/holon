---
id: 2026-08-16-rebuilds-browser-worker-engine-leaves-live
date: 2026-08-16
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `reset_vault` rebuilds the browser worker's engine but leaves the live page
  rendering the torn-down one, so the tool reports success while the UI shows
  the pre-reset vault forever.
source_line: 690
---

## Bug

(web-arm PBT increment 2; found by the arm's own bring-up while wiring the
keystone replay) **`reset_vault` rebuilds the browser worker's engine but
leaves the live page rendering the torn-down one, so the tool reports
success while the UI shows the pre-reset vault forever.** The handler drops
the old state, builds a fresh `:memory:` engine, seeds the structural
working page and self-checks that `block:parent`/`c1`/`c2` landed
(`frontends/holon-worker/src/lib.rs`, `reset_vault` arm) — and all of that
succeeds: reading `block_raw` over the MCP relay immediately after returns
25 rows including `block:structural-page`. But the page's rendered set is
byte-identical to what it was before the reset (sidebar still only
Welcome/Journals), measured through a 3s wait AND a navigation click that
re-queries. The page's data subscriptions stay bound to the engine instance
that was torn down; nothing rebinds them to the new slot.

## Root cause

web-arm PBT increment 2, found by the arm's own bring-up while wiring the
keystone replay: **`reset_vault` in the browser build rebuilds the worker's
engine but leaves the live page rendering the torn-down one, so the tool
reports success while the UI shows the pre-reset vault forever.** The
handler drops the old state, builds a fresh `:memory:` engine, seeds the
structural working page and self-checks that `block:parent`/`c1`/`c2` landed
(`frontends/holon-worker/src/lib.rs`, `reset_vault` arm) — and it all
succeeds: reading `block_raw` over the MCP relay right after shows 25 rows
including `block:structural-page`. But the page's rendered set is
byte-identical to what it was before the reset (sidebar still only
Welcome/Journals), measured through a 3s wait AND a navigation click that
re-queries. The page's data subscriptions are bound to the engine instance
that was torn down; nothing rebinds them to the new slot. IMPACT on the web
arm: the hand-authored keystone corpus is authored over the wide seed,
`reset_vault` is the only way to install that seed in a browser, and its
blocks therefore never become gesture-reachable — all 60 cases cap out and
the replay leg asserts nothing (loudly disclosed by
`web_arm_replays_hand_authored_keystone_cases`). ENVIRONMENT: the reset path
exists only in the wasm worker, and no gate before this arm booted the
browser stack deep enough to call it. NOT FIXED — the fix is a page-side
rebind-on-reset in dioxus-web/holon-worker, which is a production change
outside increment 2's scope; escalated with the increment-2 report.)

## Missing piece

The reset path is wasm-worker-only code that no headless wiring instantiates
— the keystone resets by rebuilding its own SUT, never by asking a live
frontend to swap engines underneath itself — so no invariant could go red.
No gate before this arm booted the browser stack deep enough to call
`reset_vault` at all.

## Remedy

NOT FIXED — the remedy is a page-side rebind-on-reset in
dioxus-web/holon-worker, a production change outside increment 2's scope;
RULED by the orchestrator as item 1 of increment 3, ahead of gate wiring.
IMPACT meanwhile: the hand-authored keystone corpus is authored over the
wide seed, `reset_vault` is the only way to install that seed in a browser,
so none of its blocks are gesture-reachable and all 60 cases cap out —
`web_arm_replays_hand_authored_keystone_cases` replays zero cases and says
so with a loud disclosed banner. The replay loop is live, not stubbed, so it
begins asserting the moment the rebind lands.
