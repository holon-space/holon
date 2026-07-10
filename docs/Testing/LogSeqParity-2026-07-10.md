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
