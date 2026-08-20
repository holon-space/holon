# LogSeq-parity corpus (`@wip`)

Sixteen `.feature` files, 67 scenarios, recording behaviour Holon should match
to reach LogSeq parity. Expectations were distilled from driving the live
**LogSeq DB-version** desktop app (fresh graph `HolonTest`) on 2026-08-20 and
reading its persisted datom store as ground truth. Evidence — interaction log,
screenshots, and SQLite datom snapshots — stays out of the repo under
`~/.claude/plans/logseq-gap-2026-08-20/` (`README.md` there has full context).

## The `@wip` contract

Every feature carries a feature-level `@wip` tag alongside its
`@core`/`@power`/`@peripheral`/`@observed`/`@documented-only`/`@hover-revealed`
classification. `@wip` propagates to every scenario in the file.

The strict Gherkin runner **skips** any feature or scenario tagged `@wip`: its
steps are never parsed (the corpus uses step and assertion phrasings the Holon
step registry does not implement yet, so parsing them would fail loud) and the
run reports it as skipped — visible in the summary count, never silently
dropped.

**Un-tag a scenario when you implement it.** Remove `@wip` and the runner stops
skipping it: the scenario executes through `ComposedSut<WideE2E>` and must pass.
Because `@wip` is currently feature-level, un-tagging one scenario means moving
`@wip` down to the sibling scenarios that are still unimplemented, or splitting
the feature.

`logseq_parity_replay.rs` (in `tests/catalog_suite/`) globs this directory and
runs every file through the strict runner. While the corpus is fully `@wip` the
run is green and reports 67 scenarios skipped.

## Candidate-deviation semantics

The LogSeq DB version diverges from the classic file version. Where a recorded
expectation reflects a DB-version choice Holon may or may not want to copy, the
scenario carries a `# NOTE: candidate deliberate-deviation` comment. These mark
decisions to make when implementing the scenario — parity with the DB version
is a candidate, not a mandate.
