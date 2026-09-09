# Verify — docs-gate lane (V7 architecture/docs gate steps)

pwd for every command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/docs-gate`
Base asserted: `@-` = `zqqtqulr 830d794f` (integration*/main). Tree assert: `arch-validate` in justfile OK, `@c4` in `crates/holon-rows/src/lib.rs` OK.
`jj status`: `_context/crates/current.json` and `_context/frontends/current.json` show `D` as expected.
Logs: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-docs-gate/logs/`

## Overall verdict: CONFIRMED (with 2 defects and 1 significant gap, none refuting a stated claim)

## C1 — justfile landing-gate — CONFIRMED
`jj diff --git justfile` is ONE hunk, lines 1185-1220, and touches nothing but `landing-gate`.
15 numbered steps in order; new ones are `[7/15] just arch-validate`, `[8/15] /usr/bin/python3 scripts/featuremap.py check`, `[9/15] just analyze-arch`, each `2>&1 | tee target/gate-logs/landing-{arch-validate,featuremap,analyze-arch}.log`. Body has `set -euo pipefail` + new `mkdir -p target/gate-logs`, so each pipeline is judged on the command's own status. No grep in any verdict path.
- `just --evaluate` exit 0; `just --summary` exit 0.
- `./scripts/check-justfile-pipefail.sh` exit 0 ("every `| tee` recipe is a pipefail shebang block").

## C2 — all three GREEN from fresh state, twice — CONFIRMED
`_context/crates` and `_context/frontends` were empty dirs at start.
Run 1 (fresh): `run1-archvalidate.log` exit 0 — "59 modules … 13 modules … Design validation passed — actual matches design." (x2). `run1-featuremap.log` exit 0 — "docs/Architecture/FeatureMap.md is up to date". `run1-analyzearch.log` exit 0 — "104 baselined violation(s) suppressed …, 0 new violation(s)".
Run 2 (with `_context/` regenerated): `run2-*.log` — all exit 0, identical markers. No merge conflict on the second run.
Non-zero work confirmed: 59/13 modules; `archlint --all --format json` → `"files_scanned": 1779`.

## C3 — teeth — CONFIRMED
Scratch root `.../verify-docs-gate/root` = symlinks to the real `crates/ frontends/ assets/ scripts/ experiments/ tools/` + a real `cp -R` of `docs/`.
1. `/usr/bin/python3 <wt>/scripts/featuremap.py check --root <scratchroot>` → exit 0, "up to date" (control).
2. Deleted line 248 (the `` `Search` `` transition line) from the scratch `docs/Architecture/FeatureMap.md`.
3. Re-ran the same command → **exit 1**, "has drifted from the overlay + pin sources (1 changed lines)" + the unified diff re-adding the `Search` line. (`logs/teeth-broken.log`)

Extra teeth I ran (not asked):
- `archidoc ir validate --strict <BASE baseline> _context/crates/current.json` → exit 1, 6 `UNDOCUMENTED` warnings (the ratchet bites when a crate is unratified). (`logs/teeth-archvalidate.log`)
- Mutating `c4_level` of `holon-rows` in a copy of current.json → exit 1 `DIVERGED`. (`logs/mut-level.log`)

## C4 — five @c4 headers + baseline diff — CONFIRMED
Every `uses` arrow equals the crate's `[dependencies]` holon-* set exactly (dev-deps correctly excluded):
kitchen → api/core/profiles/rows; logseq-db → api/core (dev-dep holon-capability not declared, correct); net → api/pattern/rules (dev-dep holon-core not declared, correct); plugin-host → api/core/rows (dev-dep holon-kitchen not declared, correct); rows → api/core.
Layers `Adapters` / `Engine` / `Core` are all pre-existing vocabulary (`grep '@c4 layer' crates/*/src/lib.rs`: Core 12, Adapters 12, Testing 9, Engine 6, Composition 1).
Baseline (crates), structural diff base→current computed in Python:
- top-level crate dirs added: exactly `holon-capability, holon-kitchen, holon-logseq-db, holon-net, holon-plugin-host, holon-rows`; **0 removed**.
- 356 nodes added, 0 removed; **1** pre-existing record changed: `/./holon-api/src` — only its `content` field (a module-doc grammar block that drifted in source since the last baseline). Its `c4_level` and `layer` are unchanged.
- No existing entry's level / layer / relationships changed.
Baseline (frontends): 0 dirs added/removed, 29 nodes added, **0 changed records**.
Note: the acceptance wording "adds crate entries for exactly six crates" holds at the crate level; the lane's own report is right that the file is a whole-tree snapshot (356 file nodes) — verified purely additive.

## C5 — DEVELOPMENT.md + fmt — CONFIRMED
`DEVELOPMENT.md:530` tier-L row now reads `… gate-arch · arch-validate · featuremap.py check · analyze-arch (archlint) · keystone-smoke …` — correct names, correct order. (The row was already an abbreviation and still omits latency-slo-gate/guests-verify/target-gc; that predates this change.)
`cargo fmt --all -- --check` exit 0 (`logs/fmt.log`).

## C6 — pre-existing redness — CONFIRMED (and better than claimed)
Base tree extracted: `git -C /Users/martin/Workspaces/pkm/holon archive 830d794f878f | tar -x` → 4447 files (non-empty, assert passed).
- `archidoc ir check-deps` on BASE: exit 1, **56 missing**, **10 stale**. On the lane tree: exit 1, **42 missing**, **10 stale**. Diff of the finding lines: the only difference is the 14 arrows the five new headers now satisfy. Nothing new is introduced; the lane *reduces* the debt 56→42. Stale set identical.
- `archlint --all` on BASE (its own `archlint/` from base): exit 0, "104 baselined … 0 new", "baseline stale - 5 entry(ies)". Byte-identical to the lane tree's output. Pre-existing confirmed.
- Corroboration of "red on arrival": on the base tree `featuremap.py check` exits 1 ("3 changed lines") and `archidoc ir validate --strict` exits 1 with the same 6 UNDOCUMENTED warnings. `grep -c '@c4' base crates/holon-{kitchen,rows}/src/lib.rs` = 0, i.e. the five really had no header.

## Defects (evidence only, not fixed)

D1 — `arch-compile` MERGES into a stale `_context/*/current.json` and fails hard; the gate is not self-healing.
Reproduced without touching the tree: seeded a scratch dir with `git show 830d794f878f:_context/crates/current.json` and ran
`archidoc ir compile <wt>/crates --output-dir <scratch>/ctxprobe/crates` → **exit 1**,
`error: merge conflict at 'holon-kitchen': conflicting C4 levels: existing 'unknown' vs new 'component'` (`logs/ctxprobe.log`).
Consequence: untracking `_context/` must land in this same commit (it currently does — `jj status` shows both `D`, `.gitignore:66` = `_context/`). Independently of tracking, ANY developer or lane with a stale local `_context/` compiled before this change gets a hard red at `[7/15]` with a confusing message. `just arch-compile` does not `rm -rf _context` first.

D2 — `arch-validate --strict` does NOT gate `layer` or `uses` arrows, only presence + `c4_level`.
Evidence (mutations of a copy of `_context/crates/current.json`, validated against the committed baseline):
- `holon-rows.layer` "Core" → "Adapters" → **exit 0**, "Design validation passed" (`logs/mut-layer.log`)
- `holon-rows.relationships` → `[]` → **exit 0**, "Design validation passed" (`logs/mut-rels.log`)
- `holon-rows.c4_level` "component" → "container" → exit 1 `DIVERGED` (`logs/mut-level.log`)
So the commit message's "@c4 structure matches the committed design baseline" overstates the step: the layer assignments and `uses` arrows the lane derived by judgment are ungated by step `[7/15]`. The check that WOULD gate arrows, `just arch-check-deps`, is red (42/10) and is deliberately not in the gate.
(Benign side-effect of the same narrowness: doc-comment edits do NOT churn the baseline — mutating a `content` string still validated exit 0.)

## Gaps / notes
- The lane report's line "`_context/…` are restored to base byte-for-byte and do not appear in `jj status`" is now STALE — the orchestrator untracked and deleted them, which per D1 is the correct and required state. Anyone re-reading that report could re-add them and break the gate.
- `scan_root` in both committed baselines is an absolute workspace path; base pointed at `_sw_integ`, current points at `docs-gate`. Verified NOT compared by `validate` (validation passed across differing scan_roots), so it is cosmetic churn only.
- I did not run the full `just landing-gate` (build cost); steps `[1/15]`, `[7/15]`, `[8/15]`, `[9/15]` were each run individually and green.
- The 5 stale archlint baseline entries were not enumerated (would require `--update-baseline`, a write).
