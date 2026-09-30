---
id: 2026-09-30-a-mistyped-vault-root-silently-mints-a-new-vault
date: 2026-09-30
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  A frontend started on a vault root that does not exist created the
  directory and booted an empty vault instead of saying the root is wrong.
---

## Bug
Found during verification of dogfooding phase 1 (Inc 1b/1c, caveat C): a
mistyped vault root silently created directories and a new, empty vault. The
verifier confirmed the refusal that replaced it
(`lane-logs/dogfood-1bc-verify.md` of the dogfood lane, row 5B') and flagged
it as a behavior change to ratify.

## Root cause
Nothing checked that the vault root exists before the session wrote to it.
`crates/holon-app/src/vault_lock.rs` (`VaultLock::acquire`) now refuses a root
that is not a directory, before anything is created.

## Missing piece
No test booted a frontend on a missing vault root.

## Remedy
The refusal is pinned by
`frontends/tui/tests/boot_refusals.rs::a_missing_vault_root_refuses_to_start_and_names_it`:
the TUI exits non-zero, the terminal shows
`× refusing to start: vault <path> does not exist or is not a directory`, the
log records it, and nothing is created. PARTIAL until Martin ratifies that a
missing root refuses to start, which means a brand-new vault directory must be
created by the user first (vault task "Martin: ratify that a missing vault
root refuses to start", `Cross-Cutting Concerns.org`).
