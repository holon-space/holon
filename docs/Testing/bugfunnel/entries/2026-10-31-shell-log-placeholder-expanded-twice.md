---
id: 2026-10-31-shell-log-placeholder-expanded-twice
date: 2026-10-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Shell effect log placeholders were expanded one after another, so a value that spells a placeholder was expanded again: subjects `{attempt}` and `1` got the same log.
---

## Bug
The tester replay (wave 14) found it on main 30b64bdd: with the pattern
`{subject}-{attempt}.log` and attempt 1, the subjects `{attempt}` and `1`
both wrote `1-1.log`. Card holon-space/holon-work#871.

## Root cause
`crates/holon-net-shell/src/effect.rs:330-335` (base 30b64bdd) chained
`str::replace` per placeholder: the text a `{subject}` value inserted was
scanned again by the later `{attempt}` and `{occurrence}` replacements.

## Missing piece
No test drew values that spell a placeholder; no oracle compared the logs of
two different requests.

## Remedy
`render` expands each segment in one pass, and values are encoded before
they are inserted, so inserted text is never read as a placeholder. Pinned by
`a_value_that_spells_a_placeholder_renders_as_its_own_text`,
`subjects_attempt_and_1_get_different_logs` and G3 in
`crates/holon-net-shell/tests/shell_log_paths.rs`.
