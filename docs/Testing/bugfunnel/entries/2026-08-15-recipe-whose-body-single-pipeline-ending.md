---
id: 2026-08-15-recipe-whose-body-single-pipeline-ending
date: 2026-08-15
gap: ORACLE
secondary: null
status: UNCLASSIFIED
summary: >-
  A `just` recipe whose body is a single pipeline ending in `\ | tee` always
  exits 0
source_line: 703
---

## Bug

(task-#29 gate-composition lane; found by a fresh-context verifier refuting
this lane's own report; no test produced it, and none can — the defect is
that the gate cannot report failure) **A `just` recipe whose body is a
single pipeline ending in `\ | tee` always exits 0**, because without a
`#!/usr/bin/env bash` line `just` uses `sh -cu`, which has no `pipefail`, so
the status is `tee`'s. Proven: a `false`-bodied replica of `gate-compile`
passes `just precommit` with `Tier 1 PASS`. Disclosed as THIS lane's
regression — #29 lifted `cargo check --workspace` out of precommit's
bash+pipefail block into a bare recipe, so step 2 could fail before and not
after. The trap is already documented on `hand-authored`
(`justfile:148-152`, seen 2026-07-25) and was reintroduced anyway. Nine more
live instances: `mutants`, `build`, `clippy`, `test`, `deny`, `machete`,
`duplication`, `analyze-deny`, `analyze-machete` — `just test` runs the
whole suite and exits 0 on any failure.

## Root cause

task-#29 gate-composition lane, found by a FRESH-CONTEXT VERIFIER refuting
this lane's own report — no test produced it, and no test can, because the
defect is that the gate cannot report failure: **a `just` recipe whose body
is one pipeline ending in `| tee` always exits 0, so the gate passes however
red its command is.** Without a `#!/usr/bin/env bash` line `just` runs the
body under `sh -cu`, which has no `pipefail`, and the recipe's status is
`tee`'s. The justfile has documented this exact trap at `hand-authored`
(`justfile:148-152`, observed 2026-07-25) since then, and it was
reintroduced anyway — which is the point: a comment on ONE recipe does not
generalise. Verifier proof: a `false`-bodied replica of `gate-compile`
passes `just precommit` and prints `Tier 1 PASS`. THIS LANE'S REGRESSION,
disclosed: task #29 lifted `cargo check --workspace` OUT of `precommit`'s
bash+`pipefail` block into a bare `gate-compile` recipe, so precommit step 2
COULD fail before the change and could not after — a gate-integrity
regression shipped inside a gate-integrity task. FIXED: both `gate-compile`
and `gate-arch` are now shebang+`set -euo pipefail` bodies, proven
red-for-the-right-reason (`lane-logs/t29-pipefail-proof.log`: induced
`false` → `just gate-compile` exit 1; bogus nextest flag → `just gate-arch`
exit 2; inducements reverted). **NINE MORE LIVE INSTANCES, unfixed, found by
scanning every recipe** — `mutants`, `build`, `clippy`, `test`, `deny`,
`machete`, `duplication`, `analyze-deny`, `analyze-machete`
(`justfile:504,524,528,532,563,567,571,659,663`). `just test` is the
sharpest: it runs the whole workspace suite and exits 0 on any number of
failures. Left for a ruling because changing `just test`/`just build`
semantics has cross-lane blast radius mid-flight, not because they are
acceptable. Structural remedy beyond patching each: a guard that greps the
justfile for a `| tee` body with no shebang, which is what would have caught
both 2026-07-25 and this one.)

## Missing piece

nothing asserts that a gate recipe CAN fail; a per-recipe comment does not
generalise, and no guard greps the justfile for a `\

## Remedy

tee` body without a shebang | FIXED 2026-08-15 for `gate-compile` +
`gate-arch` (shebang + `set -euo pipefail`), proven red-for-the-right-reason
in `lane-logs/t29-pipefail-proof.log` (exit 1 and exit 2 under induced
failures, reverted). The nine others are OPEN pending a ruling — changing
`just test`/`just build` semantics has cross-lane blast radius mid-flight.
