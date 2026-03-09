# Org renderer drops `#+TODO:` header — wide PBT blocker

**Status:** open; blocks wide-PBT validation of Phase C migrations (and likely other custom-keyword-set runs).

**Where it bites:** `crates/holon-integration-tests/src/pbt/sut_check_invariants.rs:2954` — `inv-org-render-fixed-point` panics on the first generated case that lands a doc with a custom TODO keyword set.

## Symptom

`general_e2e_pbt` and `general_e2e_pbt_sql_only` panic before any `[apply]` transition runs. Exact panic:

```
[inv-org-render-fixed-point] /var/folders/.../__wmqh__gg__685470.org would be
rewritten by the next re_render_all_tracked → echo-suppression loop risk.

--- disk (169 bytes) ---
#+ID: ref-doc-1
#+TODO: DOING | CLOSED CANCELLED
* CLOSED EdcblAe3o xoV Fgic
:PROPERTIES:
:ID: -790cfi-5lmk-x--r
:END:
* DOING Js    Eo6
:PROPERTIES:
:ID: ph9oprn
:END:

--- rendered from SQL (136 bytes) ---
#+ID: ref-doc-1
* CLOSED EdcblAe3o xoV Fgic
:PROPERTIES:
:ID: -790cfi-5lmk-x--r
:END:
* DOING Js    Eo6
:PROPERTIES:
:ID: ph9oprn
:END:
```

Delta = 33 bytes = exactly the missing `#+TODO: DOING | CLOSED CANCELLED\n` line.

The headlines themselves use the custom keywords (`CLOSED`, `DOING`) just fine — the parser captured them and the SQL preserves the per-block `task_state`. Only the **document-level header line** is missing.

## Root cause (high-confidence hypothesis)

`crates/holon-orgmode/src/org_renderer.rs::render_document` doesn't emit the `#+TODO:` header line. The parser side **does** capture it: `org_sync_controller.rs:405-413`:

```rust
// Sync #+TODO: keywords from the parsed file to the document block.
// ...
// via render_document() omit the #+TODO: header.            ← acknowledgement
let parsed_kws = new_parse.document.todo_keywords();
let existing_kws = document.todo_keywords();
if parsed_kws != existing_kws {
    doc.set_todo_keywords(parsed_kws);
}
```

So the document **stores** todo_keywords, but `render_document` doesn't put them back. The comment at line 407-408 explicitly admits the round-trip gap.

## Recommended fix

In `crates/holon-orgmode/src/org_renderer.rs::render_document` (entry point at line 16 area):

1. Read the document's `todo_keywords` property (or whatever accessor `OrgDocument::todo_keywords()` exposes — see `crates/holon-orgmode/src/traits.rs:114`).
2. If non-empty, emit a line of the form `#+TODO: <todo_kws> | <done_kws>\n` right after the `#+ID:` line and before the first headline.
3. Canonical format: pipe-separated, with TODO keywords before the `|` and DONE keywords after. Match what the parser at `org_sync_controller.rs:409` accepts.

Look at how `org_to_doc::parse_document` extracts `#+TODO:` for the exact format reference (it's the inverse function).

## Validation

After the fix:

```sh
# In the worktree
PROPTEST_CASES=4 PBT_ATOMIC_EDITOR=1 timeout 1800 cargo nextest run \
  --features pbt -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/wide-pbt.log
```

Expected:
- No `inv-org-render-fixed-point` panics
- Both `general_e2e_pbt` and `general_e2e_pbt_sql_only` pass (or fail on a different invariant — file separately)

Bonus: add a unit test in `crates/holon-orgmode/tests/round_trip_pbt.rs` that constructs a doc with todo_keywords, renders, parses back, asserts the keywords round-trip.

## Why this blocks Phase C verification

Phase C migrations (`kvuvvtnm 9b61aedf`) are type-checked + slim-slice-verified + unit-tested but the wide PBT failure happens **before any transition runs** (failure is in the invariant block right after WriteOrgFile-seeded state). With this fix, the wide PBT should reach the migrated transitions (`apply_focus_editable_text`, `apply_click_block`, `apply_split_block`, `apply_toggle_state`, `apply_trigger_slash_command`) and exercise them end-to-end.

## Relevant files

- `crates/holon-orgmode/src/org_renderer.rs` — render_document entry point (fix here)
- `crates/holon-orgmode/src/org_sync_controller.rs:405-413` — parser-side capture (mirror format)
- `crates/holon-orgmode/src/traits.rs:114` — OrgBlockExt / document metadata accessors
- `crates/holon-integration-tests/src/pbt/invariants/bodies/org_render_fixed_point.rs` — invariant body that catches the divergence

## Where Phase C left off

- Working copy: jj change `kvuvvtnm 9b61aedf` ("Phase C #1–#7") on top of `main`
- Worktree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/phase-c-focus-edit-2`
- All `apply_intent` smells removed from PBT transition bodies
- 5 capability primitives extracted; 5 transitions migrated; 2 transitions deleted
- `inv-viewmodel-editable-text-triggers` promoted from deferred to live
- See commit message for full delta + verified test runs
