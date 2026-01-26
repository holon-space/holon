# Handoff — Holon GPUI click-to-focus bug: ROOT-CAUSED, fix mid-edit, design decision pending

Date: 2026-06-08. Repo: `/Users/martin/Workspaces/pkm/holon` (jj working copy; work UNCOMMITTED — user squashes into parent. Just edit files). Bg-job scratch (logs): `/Users/martin/.claude/jobs/1dbb53d7/tmp/`.

## ⚠️ FIRST: process directive from the user
> "Add a task to find out why the tests did not detect this **before** actually fixing the bug."

Per CLAUDE.md ("Whenever there's a bug in the UI, always check if the E2E test … can reproduce it. If it doesn't, make prod and E2E more similar so it can"). **Do the test-gap investigation + a failing reproduction FIRST, then fix.** A TaskCreate task was filed this session (see "Test-gap task" below). Two concrete gaps are already identified — see that section.

## Read these memory files first (do NOT re-derive)
- `gpui_focus_variant_rerender_2026-06-08.md` — **THE root cause** (this bug), fully isolated live with the user. Primary context.
- `gpui_focus_handoff_desync_2026-06-08.md` — earlier orthogonal facets + the editor_view.rs scheme/window-focus fixes landed this session.
- MEMORY.md index has both at the top.

## TL;DR
The prod bug ("clicking a non-focused block doesn't make it editable; only the block that already has the cursor reacts") is **root-caused and confirmed live**. The intended fix (`focus_driver` in `reactive_view.rs`) already exists but is a **silent no-op due to a bare-vs-schemed id mismatch**. A fix is **mid-edit and the file currently does NOT compile** (see "Tree state"). A **design decision is open** (how to type the keys) — the user must pick before finishing.

## Root cause (locked — see memory for full chain)
- `assets/default/types/block_profile.yaml`: a block renders `editable_text` iff `is_focused` (line ~65), else `rendered_text` (~73). `is_focused` is evaluated at **interpret time**.
- The main panel is a **tree** → `ReactiveView::create_tree_driver` (`crates/holon-frontend/src/reactive_view.rs:775`). It has a `focus_driver` (added 2026-05-19, commit 4250a700) that is *supposed* to re-interpret the old+new focused rows on focus change and `tree.update` them.
- **THE BUG:** the `focus_driver` looked up affected rows in `row_map` by the **bare** id (`EntityUri::id()`, e.g. `f7730a68…`), and its comment wrongly claimed "Row keys are bare ids." But `row_map`/`MutableTree` keys come from `row.get("id")` = the `block` matview id, which is **schemed** (`block:f7730a68…`, confirmed via `SELECT id FROM block`). Lookup always misses → `updates` empty → `tree.update` never called → variant never swaps. On boot the tree interprets fresh with persisted focus → correct (explains "last-clicked block is editable after restart").
- Confirmed live: MCP click → Turso `current_editor_focus` AND `describe_ui` (engine `focused_block`) both move to the new block, but the user's window keeps the blinking cursor on the old block. `describe_ui` reads the engine snapshot, NOT the live gpui widget (`frontends/mcp/src/tools.rs:1658`) — that's why it "lied" relative to the window. Ruled out: focus-state-update failure and click hit-test/routing (both fine).

## The fix + the OPEN design decision
Functional fix = make the `focus_driver` match rows by **canonical `EntityUri`**, not raw id strings. `EntityUri::from_raw` canonicalizes bare and schemed to the same value, so keying `row_map` by `EntityUri` makes scheme-mismatch impossible (parse-don't-validate).

User pushed back on stringly-typed ids ("IDs as strings have caused so many bugs") and on calling `EntityUri::from_raw` in the driver (archlint rule `entity_uri_from_raw` BLOCKS it — from_raw is only allowed at a true external boundary, e.g. the matview row in `apply_change`). User's last question: *"Does ReactiveRowSet need the concrete key/value types, or can we use `K`/`V` type params?"*

**Key facts for the decision (already established):**
- `ReactiveRowSet.data: MutableBTreeMap<String, Mutable<Arc<DataRow>>>` (`reactive.rs:390`). The **key is derived from the value**: `apply_change` (`reactive.rs:417`) reads `row.get("id")` off the `DataRow`. So a bare `<K>` generic does NOT decouple from `DataRow`; you'd need an injected `key_of: Fn(&V)->K`.
- V is `Arc<DataRow>` everywhere (one instantiation). K is conceptually always "entity id" (today `String`, want `EntityUri`).
- The true parse boundary is `apply_change` (matview row enters here) — the one justified place for `// ALLOW(entity_uri_from_raw): matview row id`.

**Three options presented to the user (awaiting choice):**
1. **Fix `K = EntityUri`, V concrete (my recommendation).** Parse once in `apply_change`; `data: MutableBTreeMap<EntityUri, …>`, `row_mutable(&EntityUri)`, `keyed_rows_signal_vec → (EntityUri, DataRow)`. No `from_raw` in the driver. ~25-30 sites: the trait `keyed_rows_signal_vec` has **5 implementors** (`reactive.rs` ×2 @532/768, `lane_filtered_provider.rs:75`, `provider_cache.rs:120`, `reactive_view.rs:174`) + `&str` lookups + `key_index: Vec<String>` + `MutableTree` keys.
2. **Generic `ReactiveRowSet<K,V>` with injected `key_of: Fn(&V)->K`.** Only viable generic form. YAGNI unless a 2nd instantiation is wanted.
3. **Extract a generic `ReactiveKeyedSet<K,V>` primitive; `ReactiveRowSet` = newtype over `ReactiveKeyedSet<EntityUri, Arc<DataRow>>`.** Cleanest if the container should be reusable/testable in isolation; largest change.

**→ Next session: resume this discussion, get the user's pick, then implement. Recommend 1 unless they want a reusable container (then 3).** Whatever is picked, the `focus_driver` must match by typed `EntityUri` and use the **exact** `MutableTree` node-key string for `tree.update` (don't reconstruct it via `as_str()` — the user objected to that).

## Tree state / hazards
**`crates/holon-frontend/src/reactive_view.rs` is MID-EDIT and does NOT compile** (E0308 at ~909/941/954; archlint `entity_uri_from_raw` blocks ~3 `from_raw` calls). Half-applied: `row_map` retyped to `HashMap<EntityUri,(String,Arc<DataRow>)>`; the `focus_driver` block rewritten to match by `EntityUri` + destructure `(tree_key, row)`; the three `key.clone()` inserts changed to `from_raw`; but the `Replace`-arm `k.clone()` insert (~909) and the two `row_map.remove(&key)` sites (~941/954) are still `String`-form. **Recommended recovery: `jj restore crates/holon-frontend/src/reactive_view.rs` to HEAD, then re-implement cleanly per the chosen option** (the half-edit isn't worth salvaging — the keying strategy changes it anyway). The CORRECT focus_driver logic to re-apply: collect `affected: Vec<EntityUri>` from `[last_focus, new_focus]`, look up `row_map` by `EntityUri`, and pass the stored exact tree-key string to `tree.update`.

**Landed + COMPILING + orthogonal (keep — do NOT revert):**
- `frontends/gpui/src/views/editor_view.rs` — (a) cursor-subscription filter now compares canonical `EntityUri` (`from_raw` both sides) instead of raw strings; (b) `handle_cross_block_nav` writes the bare id. Fixes window-focus grab for schemed `editor_cursor` writes (arrow-nav / split follow-ups). Distinct from the variant-rerender bug. `cargo check -p holon-gpui` was clean.
- `crates/holon-core/src/cell.rs`, `crates/holon-frontend/src/lib.rs`, `crates/holon-frontend/src/reactive.rs` (CDC→focused_block bridge) — from prior sessions; keep.

The working tree also has large PRE-EXISTING uncommitted nav-rework (many `pbt/*`, `di.rs`, etc.) that is NOT from these sessions — leave it. Do NOT `jj restore` wholesale.

## Test-gap task (filed this session — DO THIS BEFORE re-fixing)
Why did the PBT / E2E miss a total click-to-focus failure? Two concrete gaps identified:
1. **The PBT can't see the real widget variant.** `wait_for_engine_focus` / `wait_for_focus_to_match` (`sut.rs:611`) and `describe_ui` read the **engine `focused_block` / matview**, never the **live gpui rendered variant** (rendered_text vs editable_text) nor window keyboard focus. The whole bug lives in the engine→widget gap the harness is blind to. (Same blind spot noted for the window-focus race in `gpui_focus_handoff_desync_2026-06-08.md`.)
2. **The PBT's rows are likely keyed bare**, so the bare-vs-schemed mismatch that breaks prod is masked in tests. Verify how the PBT/headless `ReactiveRowSet` keys rows vs prod (`block` matview = schemed). If they differ, that divergence is the reason the `focus_driver` "passed."
→ Add a real-window assertion that, after a content-block click, the clicked block actually renders as `editable_text` (and ideally owns window keyboard focus) — not just that `focused_block`/matview moved. Make prod and E2E more similar per CLAUDE.md.

## How to reproduce / verify live (no rebuild of your own window needed if the user's app is running)
The user runs the GPUI app via `cargo run -p gpui`; a `holon` MCP (`holon-live`) is attached. Reproduce: MCP `click` (entity_id) a non-focused block → `SELECT * FROM current_editor_focus` + `describe_ui` both show it focused/editable, but the real window cursor stays put. After a fix, the window must swap the clicked block to an editor. The user rebuilds/restarts to test fixes (real window). Run real-window `gpui_ui_pbt` SOLO; it's timing-flaky in split-heavy regions (pre-existing, see other memory).

## Suggested skills
- `debugger-mcp` / `rust-dap-debug` — if the re-fix still doesn't swap the variant, step `focus_driver` → `row_map.get` → `tree.update` → `ReactiveShell::set_content` to confirm each hop fires (compile with the `debugger` profile).
- `holon-live-mcp-debugging` / `ui-inspection` — live inspect Turso `current_editor_focus`, `describe_ui` (engine), `describe_navigation` (focus path) vs the real window; the duality is the diagnostic.
- `tdd` — for the test-gap task: write the failing real-widget/window-focus assertion FIRST, then fix.
- `ast-outline` — map `reactive_view.rs` (`create_tree_driver`/`create_flat_driver`/`focus_driver`), `reactive.rs` (`ReactiveRowSet`, `keyed_signal_vec`, `apply_change`), and the `ReactiveRowProvider` trait + 5 impls before the typing refactor.
