---
id: 2026-09-26-markdown-ingest-never-updates-planning-or-property-changes
date: 2026-09-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  On the LogSeq leg, changing or removing a block's SCHEDULED, DEADLINE,
  priority or `key:: value` property in the file dispatches no update, so the
  store keeps the old value; the markdown params also have no eraser for them.
---

## Bug
Found by the verifier of the markdown task-marker fix (lane `md-task-keyword`,
`lane-logs/md-verify.md`, "Probe 5"). The verifier drove
`LogseqMarkdownAdapter` through its real `parse`, `content_differs` and
`build_block_params`:

| File edit | Update dispatched |
|-----------|-------------------|
| `SCHEDULED:` line removed | no |
| `SCHEDULED:` date changed (2026-01-01 to 2026-02-02) | no |
| `DEADLINE:` line removed | no |
| `- TODO [#A] buy milk` to `- TODO buy milk` (priority removed) | no |
| `- buy milk` + `aisle:: 3` to `- buy milk` (property removed) | no |

The file is authoritative for a read-only foreign vault, so the store then
contradicts the file permanently. Latent: the markdown adapters are not in the
production format registry (`crates/holon-app/src/wiring.rs`, D56.a), so no
user can reach this today.

## Root cause
1. `content_differs` in `crates/holon-markdown/src/logseq.rs:328` (and
   `crates/holon-markdown/src/obsidian.rs:383`) compares `content`, `marks`,
   `tags` and `task_state` only. Planning lines, the priority cookie and
   `key:: value` lines are stripped from `content`, so an edit to any of them
   alone dispatches no update. The org adapter compares all of them
   (`crates/holon-orgmode/src/file_format.rs`, `content_differs`).
2. `build_block_params` in `crates/holon-markdown/src/params.rs:77-85` emits
   `priority`, `scheduled` and `deadline` only when the block has them, and
   has no removal branch for them. It emits no user properties at all and has
   no eraser loop over `previous`. The org leg has both
   (`crates/holon-orgmode/src/block_params.rs`: `priority` always emitted,
   `Null` when absent; the `previous.drawer_properties()` loop emits
   `Value::REMOVED`).

## Missing piece
No test changes a planning line, a priority or a block property in an existing
LogSeq file. The keystone cannot drive Markdown vaults at all (D56.a), and
`crates/holon-markdown/tests/task_marker_reingest.rs` covers task markers only.

## Remedy
OPEN. Red first per shape in the style of `task_marker_reingest.rs`. Then make
the markdown `content_differs` compare the planning fields, the priority and
the properties, and give the markdown params the org leg's erasers instead of
a second mechanism. Decide whether the markdown params must carry `key:: value`
properties at all; today they never reach the store through this builder.
