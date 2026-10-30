---
id: 2026-10-31-shell-log-subject-leaves-working-dir
date: 2026-10-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A shell effect's rendered log path could leave its working dir: a subject that starts with a parent-dir component (or an absolute or empty value) moved the log file outside `working_dir`.
---

## Bug
The tester replay (wave 14) found it on main 30b64bdd: with the pattern
`{subject}-{attempt}.log`, a subject of a parent-dir component and the name escaped wrote escaped-1.log
beside the working dir. Card holon-space/holon-work#871 ("Keep every shell
effect log inside its working dir, one log per attempt").

## Root cause
`crates/holon-net-shell/src/effect.rs:329-337` (base 30b64bdd) substituted
each raw value into the pattern and joined the result onto `working_dir`:
a `/` or `..` in a value added components, an absolute rendering replaced
`working_dir`, and an empty value before `/` rendered an absolute path.
The runner also ran author patterns that are absolute or hold `..`.

## Missing piece
No test drew values or patterns with `/`, `..`, empty strings or absolute
paths; the existing tests used plain identifiers only.

## Remedy
`log_path` renders the pattern one `/` segment at a time; each value becomes
exactly one component (printable ASCII literally, every other byte and `/`,
`%` percent-encoded; `""`, `.`, `..` encoded). Patterns that are absolute,
hold a literal `..`, name no `{attempt}`, have a stray brace or end in no file
name are refused with `ShellError::LogPath` before any child starts. Pinned
by `crates/holon-net-shell/tests/shell_log_paths.rs` (G1, G2 and the named
instances).
