---
id: 2026-09-26-markdown-ingest-never-updates-planning-or-property-changes
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
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
Red first, one test per shape in
`crates/holon-markdown/tests/planning_property_reingest.rs`
(`lane-logs/h2-red-markdown.log`: 10 of 11 red, each "no update op" or, for
the create, no `aisle` param). Green in `lane-logs/h2-green-markdown.log`.

- `content_differs` is one function, `holon_markdown::params::content_differs`,
  used by both adapters. It compares priority, `scheduled`, `deadline` and
  the user properties (`drawer_properties()`) as well.
- `build_block_params` emits the user properties, and `Value::REMOVED` for a
  priority, planning field or property that `previous` carried and the file
  no longer does. This is the org leg's rule, not a second mechanism.
- The markdown params carry `key:: value` properties because the Loro-authority
  create already does (`BlockCreateRequest::of` packs `block.properties`); only
  the params path dropped them. A property that names a `block_raw` storage
  column is dropped with a warning, as on the org leg, because
  `partition_params` would write it into that column.

Adapter-level only, like `task_marker_reingest.rs`: the keystone cannot drive
Markdown vaults (D56.a), so there is no hand-authored case.
