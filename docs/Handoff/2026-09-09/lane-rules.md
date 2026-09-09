# Lane rules (Holon repo, orchestrator session bc7b1e67) — read once, apply throughout

Base for every lane: the commit the orchestrator names in your brief (`main` = `830d794f878f` unless told otherwise). STEP 0: `cd <your workspace> && jj log -r '@-' -T 'commit_id.short(12)' --no-graph` must print exactly `830d794f878f`; on a miss return status `stale-base` + workspace path, no workarounds.

## Standards to read first
`~/.claude/skills/commenting/SKILL.md`; the repo `CLAUDE.md`; `.claude/skills/holon-feature/SKILL.md` (every behaviour change: red-for-the-right-reason PBT BEFORE the change, green after, red log kept); `.claude/skills/bug-gap-triage/SKILL.md` (a bug found outside a test = ONE new entry under `docs/Testing/bugfunnel/entries/`; flipping an entry to FIXED names the covering test; then `/usr/bin/python3 scripts/bugfunnel.py check`).

## Workspace + shell
Work EXCLUSIVELY in your workspace; prefix EVERY shell command with `cd <workspace> &&` (the agent shell cwd resets between calls — relative paths hit the wrong tree). After each edit, `grep` a distinctive new line on disk before building.

## VCS — hard ban
NEVER run jj/git write commands: no `jj commit/describe/new/squash/restore/abandon/edit/fix`, no `git add/commit/checkout/stash/reset/clean`. Leave all changes uncommitted; the orchestrator commits. Read-only `jj log/status/diff`, `git log/show/diff` are fine. Revert a probe edit by copying the file aside and writing it back (sha256 proof), never via jj.

## Builds
- Wrap every heavy cargo/just command: `bash ~/.claude/skills/orchestrator/scripts/with-build-slot.sh <cmd>` (machine-wide semaphore; it BLOCKS until a slot frees — a blocking command IS the wait). A 0-byte log means QUEUED, not failed: wait, never re-issue.
- You are FORBIDDEN from creating or waiting on background commands. Never a second concurrent cargo run (target-dir lock). If the harness force-backgrounds a command at 600 s, poll ITS log in the SAME turn with a bounded loop (`until grep -qE 'Summary|test result:|error\[|could not compile' <log>; do sleep 10; done`) until the runner's summary appears; never end a turn "waiting".
- Helper scripts you write go under `<workspace>/lane-logs/` too — NEVER under `scripts-lane/` or anywhere tracked (that directory is being deleted from main; a stray helper in your diff is a commit defect).
- Tee every run to a UNIQUE path under `<workspace>/lane-logs/` (timestamp or $$ in the name); read the runner's own summary line (nextest `Summary [...] N tests run`, cargo `test result:`); "0 tests run" = failure. Never invoke a gate through a pipe.
- No `cargo update`; never set CARGO_INCREMENTAL; never pkill cargo/rustc/ld (report PIDs).
- Known pass-with-note signatures under load (identical text only): `quick_open_search_at_vault_scale` TIMEOUT, `cursor_filtered_main_panel`, `a_dispatched_switch_reaches_the_seeded_section`, `integration_toggle_round_trip` 5 s poll, `undo_concurrent_keystrokes`, `e2e_backend_engine_test` matview reds, `test_multi_peer_sync_iroh`, `test_turso_backend_state_machine`, and whatever `scripts/keystone-known-reds.sh <log>` classifies as known. ANY other failure is yours to explain.
- Lanes touching `assets/default/*`, render builders or widget code add `just keystone-smoke` and `just hand-authored` to their gate.
- Every lane's gate includes `cargo nextest run -p holon-architecture-tests` and `just analyze-arch` (the chain gate runs them; a lane that skips them learns about a violated architecture rule one weave too late).
- Lanes touching a crate that also compiles for wasm32 (`holon-api`, `holon-core`, `holon-filesystem`, `holon-frontend`, `holon-pattern`, `holon-worker` deps) add `just check-worker-wasm` and `just check-frontend-wasm` to their gate: a new dependency or an un-`cfg`-gated `pub use` breaks the wasm build while every native check stays green.

## Secrets
Never `set`/`env`/`printenv`/`export -p`; never display secret-bearing files (`.env`, `secrets*`, `id_*`); echo single variables by name only; on any exposure STOP and report.

## Real data
The vault `/Users/martin/Workspaces/pkm/holon-pkm` is READ-ONLY. Never open the live vault in a Holon instance (it writes org files back) — copy what you need to a scratch dir first; never copy vault content into the repo or a report (synthesize anonymized fixtures).

## Context budget
If you pass ~150k tokens with work left: update your report with exact state (verified / unverified / next step) and return status `handoff`.

## Deliverable
`<workspace>/lane-report-<lane>.md`, readable by Martin: what changed (files), red log excerpt + green summary lines per gate, teeth/sha256 proof where a fix is claimed, gaps and open questions, and a `## Commit` section with ONE fenced commit message (subject ≤ 72 chars, conventional prefix, body says why). Return a ≤300-token summary: status `done|blocked|handoff|stale-base`, report path, the one thing the orchestrator must decide (if any).
