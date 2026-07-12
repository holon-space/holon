# LogSeq parity snapshot — dogfood-explorer session 2026-07-10

Live GPUI desktop app (debug build at `d52266c929`+dirty, sandboxed vault, driven
over the embedded MCP on port 8620) compared against LogSeq's core feature set.
Two passes: empty vault (default seed) and org-seeded vault (5-level tree,
CJK/emoji/diacritics, mid-word `_`, TODO/DONE, `[[Linked Page]]`, `#tags`).
Bug rows for everything broken are in [BugFunnel.md](BugFunnel.md) (2026-07-10
dogfood #2 rows). Evidence (screenshots, logs, latency reports, vault files):
`~/.claude/jobs/ceb646ab/tmp/dogfood-evidence/`.

| Feature | Status | Evidence / gap |
|---|---|---|
| Outline editing (type, Enter-split mid-text, Backspace-join) | **partial** | split/join hit SQL+disk; Enter at end-of-line does NOT create a new block — silently fails via the stale-buffer length check (BugFunnel P1 row) |
| Indent/outdent (Tab/Shift-Tab) | **works** | reparent verified in SQL both directions; indent with no previous sibling silently no-ops |
| `[[page link]]` typing → autolink/page creation | **missing** | stays literal text; no page created, no autocomplete; seeded `[[…]]` stripped to plain text on ingest (data loss — BugFunnel row) |
| Backlinks / linked references section | **missing** | no backlink UI anywhere; links aren't entities (marks NULL) |
| `#tag` behavior | **missing** | `#dogfood` never extracted to `tags`/`block_tags`; renders as plain text |
| Journal: today's page auto-created | **partial** | empty-vault boot only (block under the Journals page, correct date); org-seeded vaults get NO journal infra at all (BugFunnel row — parity-blocking) |
| Journal: day-to-day navigation | **missing** | journals are blocks under one page; no prev/next-day pages |
| Block references `((uuid))` | **missing** | literal text; `embed_entity` op renders a raw `{{transclude:…}}` marker |
| Page search / switcher | **missing** | only the magnifier = debug INSPECTOR panel; no user search; `list_commands` empty |
| TODO/DONE cycling | **works** | TODO→DOING→DONE via op; both properties set; keyword written to disk |
| SCHEDULED / DEADLINE | **partial** | parsed to properties + rendered; writeback position violates org syntax (below the drawer — BugFunnel row); no date UI |
| Page properties | **partial** | `:PROPERTIES:` drawers round-trip; no property UI |
| Zoom-in on block | **partial** | `navigation.focus` op works; no UI gesture found (bullet click doesn't zoom — medium confidence, coordinate clicks) |
| Collapse/expand outline | **partial** | chevron works visually; never persisted, and `collapsed` field changes don't reach the view (BugFunnel row) |
| Page delete + sidebar refresh | **works** | sidebar row gone within ~1s |
| Unicode/emoji/CJK rendering | **works** | pixel-perfect; mid-word underscores round-trip without subscript mangling |
| Undo/redo | **missing** | nothing reaches the undo stack from the GPUI editor path; cmd+z unbound (BugFunnel row) |

Latency at toy scale (SLO p95 e2e < 200ms — PASS, says nothing about vault
scale): set_field e2e p95 12.4ms (empty) / 26.9ms (seeded); split_block e2e
p95 173ms; join_block 26ms. `e2e` now emitted for split/join (improvement over
the skill's note); still absent for create/indent/outdent/cycle_task_state.

## Dogfood #4 re-validation (2026-07-12, this-worktree debug build, sandboxed empty + seeded vaults, MCP 8620)

Status deltas vs the table above; new bug rows in BugFunnel.md (2026-07-12).

| Feature | 07-10 | 07-12 | Evidence |
|---|---|---|---|
| Enter at end-of-line creates block | broken (P1) | **works** (fresh block, correct sibling) — but intermittently races: `Split position N exceeds content length N-4` aborts silently in log, no block (BugFunnel row) | SQL + log |
| `[[page link]]` typing | missing | **partial+** — typed `[[Some Page]]` → content-with-marks (clean content, Link mark, `block_links` junction row, dangling `resolved_id` null per lazy ruling); disk keeps `[[Some Page]]`; seeded `[[Linked Target]]` ingests to a mark (07-10 data loss FIXED). Still missing: link render styling/click-nav (plain text in read mode), autocomplete, lazy page-create surface | SQL/marks + Journals.org + screenshot |
| Empty `[[]]` | untested | **corrupts**: re-edit converts to zero-width mark, writeback emits `]][[`, compounds per cycle (BugFunnel row) | disk diff across restart |
| Backlinks | missing | **plumbing exists** (`backlinks` IVM matview; empty for dangling links — needs `resolved_id`); still NO UI | sqlite_master + query |
| `#tag` | missing | still missing (not extracted to tags/block_tags) | SQL |
| Block refs `((id))` | missing | still missing (literal text, no mark) | SQL |
| Journal today-page auto-create | partial (empty vault only) | **REGRESSED to none**: machinery dormant behind test-only `HOLON_JOURNALS_MACHINERY_SEED`; no date block on empty OR seeded boot (BugFunnel row). Seeded vaults now DO get the Journals page (07-10 gap fixed) | SQL + wide_e2e.rs grep |
| Undo/redo | missing | **landed but unsafe**: engine stack + `undo_log` persistence + loud stale-guard exist; split/join/indent/set_field undo correct in isolation (join-undo restores original id — 07-07 bug fixed). BUT: slot-typing poisons the stack with `__virtual:` entries → no-op undos, and one undo-after-delete DESTROYED 2 unrelated blocks (P1); `create` redo re-mints a new uuid; `cycle_task_state` undo is a silent no-op (BugFunnel rows). cmd+z untestable over MCP (chord router gap) | undo_log dump + SQL |
| Templates | untested | **works (ops-only)**: `instantiate_template` deep-copies subtree with `{{var}}` substitution, deterministic ids, idempotent per context_key, fail-loud on undeclared bindings (`template`+`template_vars` properties required). No UI surface | live op calls |
| Page search / switcher / quick capture / favorites / recents | missing | still missing (magnifier = debug inspector only) | UI sweep |
| Drag-block reordering | untested | not testable over MCP (no drag primitive); `move_up`/`move_down`/`move_to_position` ops exist as substrate | list_operations |
| Collapse persistence across restart | broken | not re-tested (07-10 row stands) | — |

Latency dogfood #4 (empty vault, toy scale): set_field e2e p95 58.9ms (n=83),
split 18ms, join 19ms — SLO pass. Outlier: dispatch-stage set_field p95 1293ms
max 1682ms (n=6, likely MCP-origin template set_fields; e2e unaffected — watch
at vault scale). New cross-cutting P1 found on restart: `block` IVM matview
duplicates rows per id after re-ingest over an existing DB (BugFunnel row).
