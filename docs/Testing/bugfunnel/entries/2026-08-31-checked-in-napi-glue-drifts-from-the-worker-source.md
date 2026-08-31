---
id: 2026-08-31-checked-in-napi-glue-drifts-from-the-worker-source
date: 2026-08-31
gap: ENVIRONMENT
secondary: null
status: NOTED
summary: >-
  The tracked napi glue for holon-worker had drifted from the Rust source — the
  CommonJS entry was missing an exported function and the type declarations
  documented signatures the source no longer has.
---

## Bug

`napi build` regenerates `frontends/holon-worker/index.d.ts` and
`holon_worker.wasi.cjs`, both tracked. Rebuilding at main fcfe50fb produced a
diff that was not noise: `holon_worker.wasi.cjs` gained
`module.exports.engineSetBlockExpanded`, absent from the tracked copy although
the Rust source has exported it for some time, and `index.d.ts` picked up doc
and ordering corrections for functions unrelated to the current change. Found
while running the web PBT surface (lane webpbt), where the standing advice is to
restore both files after every build as build noise.

The browser path is unaffected today: `web/worker-entry.mjs` calls the napi
module's exports directly and never loads the tracked `.cjs`. Any consumer that
did load it would find `engineSetBlockExpanded` missing.

## Root cause

Generated artifacts are checked in, and no gate regenerates them and diffs the
result, so the tree's copy is only as fresh as the last contributor who happened
to commit a rebuild. Treating the regeneration diff as noise to be reverted is
what keeps the drift in place.

Drift runs in both directions, and the other direction loses work. The tracked
`index.d.ts` carried a hand-written paragraph on `engineSetBlockExpanded`
("INTERNAL to the expand_toggle affordance — never call it alone; the page
sends both legs through `dispatch_expand_toggle`") that exists nowhere in the
Rust source. Regenerating erased it silently — a generated file cannot carry an
edit the generator has no input for.

## Missing piece

No gate asserts that the tracked glue matches what the current source
generates — the one check that would make the drift impossible to carry.

## Remedy

Partly. This lane kept the regenerated files rather than restoring them, because
the same build carried a real signature change (`engineInit` gained the viewer's
UTC offset — see
`2026-08-31-wasm-mints-the-day-page-in-utc-not-the-viewer-zone`), so restoring
would have reverted the source of truth. The erased paragraph was moved into the
Rust doc comment on `engine_set_block_expanded`, which is the only place a
generated file can carry prose from: **hand-edits to generated files are erased
by design, so anything that must survive belongs in the generator's input.**

Still open: no gate rebuilds the glue and diffs it, so the tree's copy stays only
as fresh as the last contributor who committed a rebuild. The durable fix is that
gate, or dropping the artifacts from version control.
