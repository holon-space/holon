//! Structural rebuild through a SLOT: what survives the merge and what must
//! not be carried onto the wrong node.
//!
//! `push_down_slot` recurses into a slot's content so the Mutables below it
//! survive a rebuild. Two properties bound that recursion:
//!
//!   S1 liveness: a slot root's live subscription still drives its props after
//!      a rebuild — the node keeps updating from its row.
//!   S2 identity: reordered same-widget siblings never trade state. Losing an
//!      expand is bad; showing it on the neighbouring section is worse.

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveSlot;

fn row(content: &str) -> Arc<DataRow> {
    let mut r = DataRow::new();
    r.insert("id".to_string(), Value::String("block:1".to_string()));
    r.insert("content".to_string(), Value::String(content.to_string()));
    Arc::new(r)
}

/// `text(col("content"))` bound to `cell` — the shape
/// `shared_live_query_build` puts at a slot root for a bare item template, and
/// the one that spawns a `DropTask` re-deriving `content` from row updates.
fn live_text(services: &StubBuilderServices, cell: &Mutable<Arc<DataRow>>) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl("text(col(\"content\"))")
        .expect("dsl should parse");
    let ctx = RenderContext::default().with_row_mutable(cell.read_only());
    services.interpret(&expr, &ctx)
}

/// A slot-bearing node wrapping `content`, as `view_mode_switcher` /
/// `live_query` build it.
fn slot_wrapper(content: ReactiveViewModel) -> ReactiveViewModel {
    ReactiveViewModel {
        slot: Some(ReactiveSlot::new(content)),
        ..ReactiveViewModel::from_widget("view_mode_switcher", HashMap::new())
    }
}

fn slot_content(node: &ReactiveViewModel) -> Arc<ReactiveViewModel> {
    node.slot
        .as_ref()
        .expect("the wrapper carries a slot")
        .content
        .get_cloned()
}

fn content_prop(node: &ReactiveViewModel) -> Option<String> {
    match node.props.get_cloned().get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Let the runtime drain the subscription's wakeup.
fn settle(services: &StubBuilderServices) {
    let handle = services.runtime_handle().clone();
    std::thread::scope(|s| {
        s.spawn(|| {
            handle.block_on(async {
                for _ in 0..50 {
                    tokio::task::yield_now().await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            })
        });
    });
}

#[test]
fn s1_a_slot_roots_subscription_survives_a_rebuild() {
    let services = StubBuilderServices::new();
    let cell = Mutable::new(row("first"));

    let mounted = slot_wrapper(live_text(&services, &cell));
    assert_eq!(
        slot_content(&mounted).subscriptions.len(),
        1,
        "S1 setup: a `text(col(...))` slot root must own its row subscription"
    );

    let rebuilt = slot_wrapper(live_text(&services, &cell));
    let merged = mounted.with_update(&rebuilt);

    assert_eq!(
        slot_content(&merged).subscriptions.len(),
        1,
        "S1 VIOLATED: the merged slot root owns no subscription. The rebuild \
         dropped the mounted node's DropTask (aborting its task) and discarded \
         the fresh node's, so this node stops updating from its row."
    );
}

#[test]
fn s1_b_a_row_update_still_reaches_the_slot_root_after_a_rebuild() {
    let services = StubBuilderServices::new();
    let cell = Mutable::new(row("first"));

    let mounted = slot_wrapper(live_text(&services, &cell));
    cell.set(row("before-rebuild"));
    settle(&services);
    assert_eq!(
        content_prop(&slot_content(&mounted)).as_deref(),
        Some("before-rebuild"),
        "S1 setup: the subscription must drive props before any rebuild"
    );

    let rebuilt = slot_wrapper(live_text(&services, &cell));
    let merged = mounted.with_update(&rebuilt);

    cell.set(row("after-rebuild"));
    settle(&services);
    assert_eq!(
        content_prop(&slot_content(&merged)).as_deref(),
        Some("after-rebuild"),
        "S1 VIOLATED: after a structural rebuild a row update no longer reaches \
         the slot root's props — the node went silently dead, still painting \
         its pre-rebuild text"
    );
}

/// Two same-widget siblings that carry state, told apart by their title.
fn titled_accordion(title: &str, expanded: bool) -> Arc<ReactiveViewModel> {
    let mut props = HashMap::new();
    props.insert("title".to_string(), Value::String(title.to_string()));
    Arc::new(ReactiveViewModel {
        expanded: Some(Mutable::new(expanded)),
        ..ReactiveViewModel::from_widget("accordion", props)
    })
}

fn expanded_by_title(node: &ReactiveViewModel, title: &str) -> bool {
    let child = node
        .children
        .iter()
        .find(|c| match c.props.get_cloned().get("title") {
            Some(Value::String(t)) => t == title,
            _ => false,
        })
        .unwrap_or_else(|| panic!("a child titled {title}"));
    child
        .expanded
        .as_ref()
        .expect("an accordion carries an `expanded` handle")
        .get()
}

#[test]
fn s2_reordered_siblings_never_trade_their_state() {
    let mounted = ReactiveViewModel {
        children: vec![titled_accordion("A", false), titled_accordion("B", false)],
        ..ReactiveViewModel::from_widget("column", HashMap::new())
    };

    // The reader expands A, then a rebuild emits the two sections swapped.
    mounted.children[0]
        .expanded
        .as_ref()
        .expect("gate")
        .set(true);
    let rebuilt = ReactiveViewModel {
        children: vec![titled_accordion("B", false), titled_accordion("A", false)],
        ..ReactiveViewModel::from_widget("column", HashMap::new())
    };

    let merged = mounted.with_update(&rebuilt);

    assert!(
        !expanded_by_title(&merged, "B"),
        "S2 VIOLATED: after the swap the section titled B is expanded. Positional \
         keying carried A's `expanded` onto B, so the reader sees the WRONG \
         section open — worse than losing the expand"
    );
}
