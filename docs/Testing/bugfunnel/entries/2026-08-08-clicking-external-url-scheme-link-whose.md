---
id: 2026-08-08-clicking-external-url-scheme-link-whose
date: 2026-08-08
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Clicking an external URL, or any scheme link whose target has no block row,
  silently BLANKS the whole main panel.
source_line: 1187
---

## Bug

(Martin dogfooding; reproduced against INGESTED links in a throwaway vault
at main e3cc10fe) **Clicking an external URL, or any scheme link whose
target has no block row, silently BLANKS the whole main panel.** Six-kind
matrix clicked in read mode with geometry re-measured before each click:
page link `Name{…}`, dangling page link, and block link to a real block all
navigate correctly; `Scheme{tag:rust}`, `Scheme{cc-session:0f3a}` and
`External{https://example.com}` all leave `tree [1 items] / tree_item /
(empty)` and open no browser. The dispatch is the bug:
`params={"region":"main","block_id":"https://example.com"}` — a URL as a
block_id, likewise the bare schemes. Two stacked defects:
`rendered_text.rs:299` routes `External{url}` into `nav_focus` at all, and
the `resolves_entity` gate at `:286` answers "is this SCHEME registered"
(`link_parser.rs:60`) not "does this INSTANCE exist", so `nav_focus`
(`:321-332`) builds the op from the raw target with no parse and no
existence check — and `navigation.focus` accepts it and renders empty
instead of refusing. Zero ERROR lines across all three failures.
Deliberately probed with ingested links so it stays disjoint from the
same-day typed-`[[wiki links]]`-never-parse row (task #12). COVERAGE, not
ENVIRONMENT: link spans are byte ranges inside one `StyledText` with no
entity id and no registered bounds (`rendered_text.rs:195-318`), so no
harness rung can address one — the interaction is ungeneratable. Martin's
interception hypothesis is REFUTED for these kinds; the navigation genuinely
dispatches.

## Root cause

Martin dogfooding, reproduced against INGESTED links in a throwaway vault at
main e3cc10fe — **clicking an external URL, or any scheme link whose target
has no block row, silently BLANKS the whole main panel**. Deliberately
probed with links ingested from org files (real `Link` marks, verified in
SQL) so this stays disjoint from the same-day "typed `[[wiki links]]` are
never parsed" row (task #12). Six-kind matrix, each clicked in read mode
with geometry re-measured immediately beforehand: page link `Name{Beta
Page}` OK navigates · dangling page `Name{Nonexistent Page}` OK mints via
`create_page_from_link` then navigates · block link `Scheme{block:…}` to a
real block OK zooms — then `Scheme{tag:rust}` FAIL,
`Scheme{cc-session:0f3a}` FAIL, `External{https://example.com}` FAIL, all
three leaving `column / view_mode_switcher / tree [1 items] / tree_item /
(empty)` and no browser. The dispatch is the whole bug: `params={"region":
String("main"), "block_id": String("https://example.com")}` — a URL passed
as a block_id; likewise `block_id: "cc-session:0f3a"` and `block_id:
"tag:rust"`. TWO stacked defects: (a)
`frontends/gpui/src/render/builders/rendered_text.rs:299` routes
`External{url}` into `nav_focus` at all, and (b) the `resolves_entity` gate
at `:286` answers "is this SCHEME registered"
(`crates/holon-api/src/link_parser.rs:60` — block/tag/person plus every
sidecar entity) rather than "does this INSTANCE exist", so `nav_focus`
(`:321-332`) builds `navigation.focus{block_id}` from the raw target with no
parse and no existence check — and `navigation.focus` then accepts it and
renders empty rather than refusing. Zero ERROR lines across all three
failing clicks: silent degradation, priority 4 in the project's own
ordering. Each failure was bracketed by a count of
`operation.entity="navigation"` log occurrences, so the dispatch is
attributed to the click; the three passing kinds are the control that
coordinates and the read-mode precondition were right. COVERAGE, not
ENVIRONMENT: link spans are byte ranges inside ONE `StyledText` with no
entity id and no registered bounds (`rendered_text.rs:195-318`), so
`describe_ui` cannot expose them and NO harness rung — headless or windowed
— can address one; the interaction is ungeneratable, which is the COVERAGE
litmus. Secondary ENVIRONMENT (the click closure is GPUI-only). Martin's
interception hypothesis is REFUTED for these three kinds — the navigation
genuinely dispatches; the outer `click_to_focus` (`prelude.rs:47-60`,
mounted `render_entity_view.rs:210-212`) does also fire without the
`stop_propagation` that `selectable.rs:68-70` uses, but it is not what
blanks the panel. Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-link-click-blanks-main-panel.txt`)

## Missing piece

no addressable link span in any harness (no entity id, no bounds), so no
draw can click a link; and no invariant forbids `navigation.focus` to a
non-existent id

## Remedy

FIXED 2026-08-08 (lane LINK-CLICK-ROUTE), three layers, all fail-loud. (1)
ROUTING: the click site now decides with a value, not inline verbs —
`link_click_action` (`frontends/gpui/src/render/builders/rendered_text.rs`)
maps `External{url}` to `LinkClickAction::OpenUrl` (handed to gpui's
`App::open_url`, never a `block_id`), and admits a `Scheme` target to
`Navigate` only when it is BOTH registered AND `block`-schemed — because a
focus root reaches the screen only through `focus_roots JOIN block`, so
`tag:`/`person:`/sidecar schemes were navigating to a panel that
structurally cannot show them. Both refusals degrade to caret placement, the
same benign outcome as clicking ordinary text. (2) PRECONDITION:
`navigation.focus` (`crates/holon/src/navigation/provider.rs`) now refuses a
`block_id` that is not a `block:` URI — `focus_target_is_a_block`, a pure
parse with no read, whose `Err` names the offending target and its actual
scheme and reaches `surface_op_failure`'s ERROR + CommandFailed toast. This
is the deeper fix the row asked for: it protects every caller, not only link
clicks. `focus` with no `block_id` (go-home) stays accepted, and a refusal
WRITES NOTHING so the region keeps its prior root. The EXISTENCE half of
that precondition was built, MEASURED, and ESCALATED rather than landed:
probing `block_raw` before the history write turned the windowed suite from
265/1 into 140/13, every failure `boot … seed_default_layout failed:
navigation.focus: refusing 'block:journals' … no such block` — because in
the LORO arm `ordering.create_in_tree` writes only the Loro tree
(`sql_block_operations.rs:577-601`) while `wiring.rs:446-473` projects the
seeded layout into `block_raw` strictly AFTER `seed_default_layout` returns,
by its own comment. `block:journals` is a real first-class block
(`holon-frontend/src/lib.rs:101-137`), not a synthetic target, so the
invariant is semantically right (`assets/default/index.org:26-45`
inner-joins the focus root to `block`) and only its TIMING is wrong — a
boot-ordering fork touching every Loro-arm boot, with three candidate
remedies recorded in the evidence file. PINNED, not merely noted: the
refusal test's last assertion focuses a nonexistent `block:` URI and
`expect`s SUCCESS under the banner `KNOWN HOLE`, so it flips
`expect`→`expect_err` the day existence is enforced. Disclosed by the same
measurement and NOT triaged here: on a Loro-arm first launch the opening
main-panel focus root is a block SQL cannot yet join. (3) THE OBSERVABLE —
the COVERAGE half, and the row's original "no harness rung can click a link
span" is PARTLY REFUTED: `glyph_center` in
`frontends/gpui/tests/layout_editor.rs` already addresses a CHARACTER INDEX
inside a `rendered_text`, so a link span IS clickable at that rung; what was
missing was only an intent recorder, now added as
`TestServices::recorded_intents` (`frontends/gpui/tests/support/mod.rs`).
GAP CLOSED BY A PAIR at that rung:
`clicking_an_external_link_opens_the_url_instead_of_navigating` asserts zero
`navigation` intents AND `cx.opened_url() == Some("https://example.com")` —
reading gpui's `TestPlatform` recorder, so the assertion observes the open
WITHOUT launching a browser — beside the control
`clicking_an_entity_link_still_navigates`, which pins that the routing fix
did not disarm entity links. Plus
`link_click_action_routes_each_kind_to_its_verb` (the whole five-row routing
table incl. the registered-but-viewless `tag:rust`),
`link_click_action_opens_mailto_rather_than_navigating`, and
`focus_refuses_a_target_that_names_no_block` /
`focus_with_no_block_id_is_still_accepted` driving the REAL
`NavigationProvider` through `execute_operation`
(`crates/holon/tests/turso_storage_repros/navigation_focus_refuses_unresolvable_target.rs`),
which also asserts a refused focus WRITES NOTHING so the region keeps its
prior root — the panel keeps rendering instead of blanking. NOT CLOSED,
named residuals: the COMPOSED windowed/keystone harness still addresses only
whole elements (`SimUserDriver::text_center` /
`GpuiUserDriver::text_center`), so no generated draw can click a link span —
the sub-range driver is the remaining coverage lift;
`crates/holon-frontend/src/reactive.rs` `maybe_mirror_navigation_focus`
still mirrors a focus intent into `UiState` OPTIMISTICALLY at dispatch time,
before the op can refuse;
`frontends/dioxus-web/src/render/builders/rendered_text.rs` carries the same
untouched `External` → `nav_focus` routing; and `tag:`/`person:` links now
place a caret rather than reaching any destination, because no such
destination exists yet. Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-link-click-blanks-main-panel.txt`
(the triage matrix) and
`docs/Testing/fixture-logs-2026-08-08/task17-link-click-red-green.txt` (the
red-for-the-right-reason logs + gates).
