# Verify: lane `funnel-false-alarm` (D63.a — fifth class FALSE-ALARM)

Verdict: **CONFIRMED** on all five claims, with three non-blocking gaps noted below.
Everything here was produced in this session, in
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/funnel-false-alarm`
(`pwd` printed; `@` = tmusutpo f50c0cae, parent qlsowply 4870faab).

## (1) `check` accepts FALSE-ALARM, rejects near-misses loudly — CONFIRMED

Baseline: `/usr/bin/python3 scripts/bugfunnel.py check` -> `588 entries, 0 problems`, exit 0.

Probe: the notify entry was copied aside (sha256
`b8196d9443db699660fa8f188a3413d905021841766cbad4823f6f15936ff978`), its `gap:`
rewritten to each value below, `check` re-run, then the file restored and the
sha256 re-printed identical.

| gap value | check output | verdict |
|---|---|---|
| `false-alarm` | `gap 'false-alarm' is not one of ('ENVIRONMENT', 'COVERAGE', 'PERCEPTION', 'ORACLE', 'FALSE-ALARM')` — 1 problem | rejected |
| `FALSEALARM` | same shape, 1 problem | rejected |
| `OPEN` | same shape, 1 problem | rejected |
| `FALSE ALARM` | same shape, 1 problem | rejected |
| `False-Alarm` | same shape, 1 problem | rejected |
| (empty) | `missing gap` + `gap None is not one of (...)`, 2 problems | rejected |
| `FALSE-ALARM ` (trailing space) | `0 problems` | accepted — YAML strips trailing whitespace; identical for all four legacy gaps, so pre-existing, not a lane regression |

Exit code on a bad value verified separately: `check exit on problem=1`.

## (2) Percentages over escapes only; FALSE-ALARM on its own line — CONFIRMED

`counts` output:

```
ENVIRONMENT: 227 (38.7%)
COVERAGE: 169 (28.8%)
PERCEPTION: 77 (13.1%)
ORACLE: 114 (19.4%)
TOTAL ESCAPES: 587
FALSE-ALARM: 1 (excluded from the distribution above)
```

Independent count straight off the entry files
(`grep -l "^gap: <G>$" *.md | wc -l` in `docs/Testing/bugfunnel/entries/`):
ENVIRONMENT 227, COVERAGE 169, PERCEPTION 77, ORACLE 114, FALSE-ALARM 1;
588 files total. `grep -h "^gap:" | sort | uniq -c` shows no sixth value.
227+169+77+114 = 587 = TOTAL ESCAPES. Percentages sum to 100.0.

`index` agrees: table rows 227/169/77/114, `Total escapes: 587`,
`Not escapes — FALSE-ALARM: 1`. INDEX.md is gitignored (.gitignore:79), so
regenerating it did not dirty the tree.

Denominator was tested behaviorally, not only read:

- Adding a second FALSE-ALARM entry left all four percentages and
  `TOTAL ESCAPES: 587` unchanged, `FALSE-ALARM: 2`. (temp file removed)
- Reclassifying a real ENVIRONMENT entry
  (`2026-09-01-subtree-share-tmp-leftover-race.md`) to FALSE-ALARM moved the
  denominator: `ENVIRONMENT: 226 (38.6%) … ORACLE: 114 (19.5%)`,
  `TOTAL ESCAPES: 586`. Restored; sha256
  `2b0d7ade67ade17452effde81330ca4f32ad7f2776772a4b9e9bef57f52fd3f3` before and after.

`list --gap FALSE-ALARM` works and returns exactly the notify entry.

Note (cosmetic, not a claim): the STATUS block under `counts` still counts over
all entries (305+16+2+234+6+25 = 588), i.e. it includes the false alarm. Only
the gap distribution is escape-scoped. Defensible, but the two blocks now have
different denominators with nothing on screen saying so.

## (3) Other readers of the gap enum — one real gap found

Repo-wide `grep PERCEPTION` (excluding `entries/`) gives these enum sites:

- `scripts/bugfunnel.py` — updated (GAPS/CLASSES split).
- `scripts/bugfunnel-migrate.py:22-24` — NOT updated. It is a one-shot converter
  whose input is the legacy `docs/Testing/BugFunnel.md`, which is now a
  tombstone ("Bug Funnel — moved"). It never reads `entries/`, so it cannot
  miscount the fifth class. Latent hazard, pre-existing and unrelated to D63.a:
  it wipes and rebuilds `entries/` from the tombstone, so running it today would
  delete all 588 entries, not just the new one.
- **`.claude/skills/dogfood-explorer/SKILL.md:446-451` — NOT updated.** This is a
  live reader: its "Litmus:" line enumerates only COVERAGE / ORACLE /
  ENVIRONMENT / PERCEPTION and is the classification instruction a
  dogfood-explorer agent follows at triage time. An agent following that skill
  has no route to FALSE-ALARM and will pick "least-wrong of four" — exactly the
  mislabelling the new skill text warns against. Nothing breaks mechanically;
  the fifth class is simply unreachable from the dogfood channel.
- `docs/Testing/BugFunnelFormat.md` — mentions gaps only in a historical example
  and a frozen `computed totals:` line from the migration. No enum declaration.
  No update needed.
- No justfile recipe, no Rust code, and no CI/hook reads the gap enum; the
  remaining Rust/feature-file hits are prose citations of individual entries.

Second, smaller inconsistency: the lane switched `bugfunnel.py` invocations to
`/usr/bin/python3` in SKILL.md and the module docstring because bare `python3`
on this machine is Homebrew and lacks `yaml` (reproduced:
`ModuleNotFoundError: No module named 'yaml'`, exit non-zero). But
`CLAUDE.md:4`, `docs/Testing/BugFunnel.md` (5 occurrences) and
`.claude/skills/dogfood-explorer/SKILL.md:450` still say bare `python3`. Those
commands are broken as written. Pre-existing, but the lane fixed the same
problem two files over, so it reads as half-done.

## (4) Skill text and the relabelled entry — CONFIRMED

`.claude/skills/bug-gap-triage/SKILL.md` adds "The fifth class: `FALSE-ALARM`
(not an escape)" and states plainly: "Do NOT use it for a flake whose cause you
have not found: an unexplained failure stays OPEN in its escape class until it
is root-caused". The frontmatter description, the `gap:` comment enum and the
procedure block are all updated consistently. `preamble.md` carries the same
rule in the ledger header.

The notify entry's new `## Class` section states the class in four lines with no
history narration; the old 18-line apology section ("The four gaps do not cover
this one", "a value had to be chosen", "ENVIRONMENT is the least-wrong") is gone.

Caveat, pre-existing and untouched by this diff: further down the same entry the
Bug section still says "**The old oracle is a genuine race — it is NOT
deterministic**, and this entry previously said otherwise." That is history
narration about the entry itself, in a file the lane was editing. Out of scope
for D63.a, but it is the one place a reader still learns what the entry used to
claim.

## (5) `jj status` — CONFIRMED

After every probe was restored, `jj status` lists exactly the four intended
files and nothing else:

```
M .claude/skills/bug-gap-triage/SKILL.md
M docs/Testing/bugfunnel/entries/2026-09-01-notify-watcher-arm-first-event-oracle.md
M docs/Testing/bugfunnel/preamble.md
M scripts/bugfunnel.py
```

No jj or git write command was run. The AST-level diff renders the PERCEPTION
table row as "modified"; the git-level diff shows it as pure context with no
whitespace change — rendering noise, not an edit.

## Summary

The mechanism does what D63.a asked. The one thing worth routing to a fix is the
dogfood-explorer skill's four-way litmus, which is the main production path for
new entries and cannot produce a FALSE-ALARM classification.
