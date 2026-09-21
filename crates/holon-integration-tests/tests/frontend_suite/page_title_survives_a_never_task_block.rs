//! A block that NEVER carried a task state renders as a BARE h1 title —
//! h1 text, no task control — wherever it holds the `page_title` role.
//!
//! `task_state` is a PROPERTY, not a declared column
//! (`crates/holon-profiles/src/lib.rs` pins that it must not be declared), so
//! on a never-task block it is structurally UNBOUND. A profile condition that
//! requires an unbound column is a SILENT non-match — `eval_condition`
//! (`crates/holon-api/src/entity_profile.rs`) returns false without invoking
//! Rhai, and warns only for columns that ARE declared. So a title condition
//! that mentions `task_state` stops matching the ordinary case, the row falls
//! through to `embedded_page` (a Page) or `default`/`editing` (a plain
//! block), and the title silently loses its h1.
//!
//! Nothing asserted the h1 before this rung: every existing title guard tests
//! for the ABSENCE of a state_toggle, which a fallthrough to `embedded_page`
//! also satisfies. This one asserts the h1 POSITIVELY, and it guards the
//! opposite direction too — the never-task title must carry NO state_toggle,
//! which is the only test of the `has_task_state` gate on `page_title_task`.
//!
//! It judges the RIGHT SIDEBAR's pinned head, where `assets/default/index.org`
//! stamps the role on every level-0 row. The main panel's context root is NOT
//! asserted here, and deliberately so: measured on main's own profile
//! (`lane-logs/ptr-newtest-on-main.log`), focusing a `Page` resolves
//! `embedded_page` — an `expand_toggle` whose header draws unstyled text — so
//! there is no h1 on that path to pin, and asserting one would pin a
//! behaviour the product does not have.
//!
//! @pbt kind harness
//! @pbt covers never-task-title-loses-its-h1 +
//! never-task-title-grows-a-task-control   — a block with no task_state
//! property, holding the page_title role,   renders the h1 title treatment and
//! no state_toggle @pbt slips-if-removed a title condition that reads
//! `task_state` silently   stops matching every ordinary page and head and they
//! degrade to outline   rows, or the task title variant loses its
//! has_task_state gate and every   page, document and query title grows a task
//! control — with no test   saying so either way

use std::time::Duration;
use std::time::Instant;

use holon_integration_tests::pbt::frontend_slice::components::HeadlessFrontendComponent;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutMutate;
use holon_pbt_core::capabilities::SutNavHistoryDrive;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::types::CycleTarget;

/// Two title shapes that are both "not a task", and that `has_task_state`
/// must separate from a real one by its two clauses:
///
/// - `plain-head` NEVER carried a task state, so its properties bag holds no
///   `task_state` key at all — the column is UNBOUND, which is what `task_state
///   != ()` tests. The ordinary vault shape.
/// - `cleared-head` is authored as a TODO and cycled back to no state by the
///   test, which leaves the key BOUND to `""` — what `task_state != ""` tests.
///   Authoring an empty property cannot produce it: an empty value drops its
///   key on write-back, giving the unbound shape again.
const PLAIN_ORG: &str = "#+ID: plain-doc\n* Plain Page :Page:\n:PROPERTIES:\n:ID: \
                         plain-page\n:END:\n** Plain Head\n:PROPERTIES:\n:ID: \
                         plain-head\n:END:\nBody of the plain head.\n** TODO Cleared \
                         Head\n:PROPERTIES:\n:ID: cleared-head\n:END:\nBody of the cleared \
                         head.\n";

/// The row's OWN scope, truncated at nested `tree_item`s — a nested row is
/// another block's rendering and must not be credited to this one. Same scope
/// rule as `inv-viewmodel-task-rows-have-state-toggle`.
fn row_scope<'a>(node: &'a WidgetSnapshot, out: &mut Vec<&'a WidgetSnapshot>) {
    for child in &node.children {
        if child.kind == "tree_item" {
            continue;
        }
        out.push(child);
        row_scope(child, out);
    }
}

/// Whether `node`'s own row scope renders the BARE title treatment: a `text`
/// carrying the `h1` type-scale keyword, no outline-row text, and no
/// `state_toggle`.
///
/// The keyword, not a pixel size: `text()` carries `#{style: "h1"}` through
/// unresolved and each platform resolves it through
/// `render_eval::text_style_treatment` at render time, so `size` still reads
/// as the 14px builder default in every ViewModel. The keyword→scale mapping
/// is pinned in `holon-api` and the windowed geometry in the GPUI suite; what
/// this headless rung owns is which VARIANT the row resolved to.
///
/// The `state_toggle` clause is the OTHER half of the title split, and the
/// only test of it: `page_title_task` is what may draw a toggle beside the
/// h1, and it is gated on `has_task_state`. Drop that gate and every page,
/// document and query title grows a task control — which nothing else in the
/// suite notices, because a spurious toggle beside a correct h1 satisfies
/// every other title check. It cannot live as a keystone invariant ("a
/// non-task row draws no state_toggle" is FALSE in general: the `default` and
/// `editing` outline variants draw one on EVERY row, `block_profile.yaml`,
/// and an unbound `task_state` just yields the default keyword cycle). The
/// property is specific to the title role, so it is pinned where the title
/// role is.
fn title_verdict(node: &WidgetSnapshot) -> Result<(), String> {
    let mut scope = Vec::new();
    row_scope(node, &mut scope);

    let outline_texts: Vec<&str> = scope
        .iter()
        .map(|n| n.kind.as_str())
        .filter(|k| *k == "rendered_text" || *k == "editable_text")
        .collect();
    let toggles = scope.iter().filter(|n| n.kind == "state_toggle").count();
    let h1s = scope
        .iter()
        .filter(|n| n.kind == "text")
        .filter(|n| n.props.get("style").map(String::as_str) == Some("h1"))
        .count();

    if h1s == 1 && outline_texts.is_empty() && toggles == 0 {
        return Ok(());
    }
    Err(format!(
        "expected exactly one `text` styled h1, no outline-row text and NO state_toggle in the \
         row's own scope, got {h1s} h1(s), {outline_texts:?} and {toggles} state_toggle(s). \
         Scope kinds: {:?}",
        scope.iter().map(|n| n.kind.as_str()).collect::<Vec<_>>(),
    ))
}

/// Poll until `id` renders at least one `tree_item` row, then require that at
/// least ONE of its occurrences is the title treatment. Not every occurrence:
/// the same block is also an ordinary editable row in the main panel it was
/// pinned from, which is correct and is another oracle's business. The
/// property here is that the pinned head renders as a title SOMEWHERE — on a
/// tree whose title condition silently stops matching, NO occurrence does.
/// Waiting on the ROW (not on a fixed sleep) keeps a delivery delay from
/// reading as a title defect.
async fn assert_renders_as_title(comp: &HeadlessFrontendComponent, id: &str, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snap = comp.widget_tree_snapshot().await;
        let rows: Vec<&WidgetSnapshot> = snap
            .walk()
            .filter(|n| n.kind == "tree_item" && n.entity_id.as_deref() == Some(id))
            .collect();
        if !rows.is_empty() {
            let verdicts: Vec<String> =
                rows.iter().filter_map(|r| title_verdict(r).err()).collect();
            assert!(
                verdicts.len() < rows.len(),
                "{what}: NONE of the {} `{id}` row(s) renders the bare h1 title treatment. Either \
                 a title condition requires an UNBOUND column — a silent non-match, so the row \
                 fell through to an outline/embedded variant everywhere — or the task title \
                 variant lost its `has_task_state` gate and a never-task title grew a \
                 state_toggle. {verdicts:?}",
                rows.len(),
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "precondition: {what} must render a tree_item row for `{id}` — it rendered none, \
             which is a delivery defect, not the title defect this rung guards. Rendered \
             entities: {:?}",
            snap.walk()
                .filter_map(|n| n.entity_id.clone())
                .collect::<Vec<_>>(),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_never_task_block_keeps_its_h1_under_the_page_title_role() {
    let comp = HeadlessFrontendComponent::new_with_loro(
        &[("plain.org", PLAIN_ORG)],
        Duration::from_millis(500),
        true,
    )
    .await;

    // Focus the page first, so the head is a descendant of the main panel's
    // focus root and pinnable the way the app pins it.
    comp.apply_navigate_focus(CapRegion::Main, &EntityUri::block("plain-page"))
        .await;

    // `index.org` titles every level-0 row of the right sidebar, and a pinned
    // subtree head IS level 0.
    comp.pin_block(
        holon_api::Region::RightSidebar,
        &holon_api::EntityUri::parse("block:plain-head").expect("plain-head id"),
    )
    .await;
    assert_renders_as_title(&comp, "block:plain-head", "pinned right-sidebar head").await;

    // The other half of `has_task_state`. Cycling the TODO back to no state
    // leaves `task_state` BOUND to the empty string, so a gate that only
    // asks "is the column bound" reads this cleared block as a task and
    // hands its title a state_toggle whose one state is the empty one.
    let cleared = holon_api::EntityUri::parse("block:cleared-head").expect("cleared-head id");
    comp.toggle_state(&cleared, CycleTarget::Clear).await;
    comp.pin_block(holon_api::Region::RightSidebar, &cleared)
        .await;
    assert_renders_as_title(
        &comp,
        "block:cleared-head",
        "pinned right-sidebar head whose task state was cleared",
    )
    .await;
}
