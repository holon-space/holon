//! Verify that `ReactiveView::children_signal_vec` chains the trailing slot
//! ViewModel after the real `items`.
//!
//! This is the core mechanic of the virtual-child-without-synthetic-row
//! refactor: the placeholder is built directly at the ViewModel layer (not as
//! a fake `DataRow`), and the chain at this layer is what frontends see.

use std::sync::Arc;

use futures::StreamExt;
use futures_signals::signal_vec::{SignalVecExt, VecDiff};
use holon_api::Value;
use holon_frontend::reactive_view::{ReactiveView, TrailingSlot};
use holon_frontend::reactive_view_model::{CollectionVariant, ReactiveViewModel};

fn leaf(label: &str) -> Arc<ReactiveViewModel> {
    Arc::new(ReactiveViewModel::leaf(
        "text",
        Value::String(label.to_string()),
    ))
}

fn variant() -> CollectionVariant {
    CollectionVariant::from_name("list", 0.0).expect("`list` layout is registered as builtin")
}

fn label_of(vm: &Arc<ReactiveViewModel>) -> String {
    vm.props
        .lock_ref()
        .get("content")
        .and_then(|v| v.as_string())
        .map(String::from)
        .unwrap_or_default()
}

/// Drain all VecDiffs that arrive within `budget` and replay them onto a
/// virtual vec, returning the resulting labels.
async fn drain_to_visible<S>(mut stream: S, budget: std::time::Duration) -> Vec<String>
where
    S: futures::Stream<Item = VecDiff<Arc<ReactiveViewModel>>> + Unpin,
{
    let mut visible: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let timeout = tokio::time::sleep_until(deadline);
        tokio::pin!(timeout);
        tokio::select! {
            biased;
            maybe = stream.next() => {
                match maybe {
                    None => break,
                    Some(VecDiff::Replace { values }) => {
                        visible.clear();
                        for v in values { visible.push(label_of(&v)); }
                    }
                    Some(VecDiff::InsertAt { index, value }) => {
                        visible.insert(index, label_of(&value));
                    }
                    Some(VecDiff::UpdateAt { index, value }) => {
                        visible[index] = label_of(&value);
                    }
                    Some(VecDiff::RemoveAt { index }) => {
                        visible.remove(index);
                    }
                    Some(VecDiff::Move { old_index, new_index }) => {
                        let v = visible.remove(old_index);
                        visible.insert(new_index, v);
                    }
                    Some(VecDiff::Push { value }) => {
                        visible.push(label_of(&value));
                    }
                    Some(VecDiff::Pop {}) => {
                        visible.pop();
                    }
                    Some(VecDiff::Clear {}) => {
                        visible.clear();
                    }
                }
            }
            _ = &mut timeout => break,
        }
    }
    visible
}

#[test]
fn snapshot_includes_trailing_slot_after_real_items() {
    let mut view = ReactiveView::new_static_with_layout(
        vec![
            ReactiveViewModel::leaf("text", Value::String("a".into())),
            ReactiveViewModel::leaf("text", Value::String("b".into())),
        ],
        variant(),
    );
    view.set_trailing_slot(TrailingSlot {
        view_model: leaf("slot"),
    });

    let snap = view.children_snapshot();
    assert_eq!(snap.len(), 3, "two items + one slot");
    assert_eq!(label_of(snap.last().unwrap()), "slot");
}

#[test]
fn snapshot_without_slot_matches_items() {
    let view = ReactiveView::new_static_with_layout(
        vec![ReactiveViewModel::leaf(
            "text",
            Value::String("only".into()),
        )],
        variant(),
    );
    assert_eq!(view.children_snapshot().len(), 1);
}

#[tokio::test]
async fn signal_vec_emits_real_items_then_slot() {
    let mut view = ReactiveView::new_static_with_layout(
        vec![
            ReactiveViewModel::leaf("text", Value::String("first".into())),
            ReactiveViewModel::leaf("text", Value::String("second".into())),
        ],
        variant(),
    );
    view.set_trailing_slot(TrailingSlot {
        view_model: leaf("slot"),
    });

    let stream = view.children_signal_vec().to_stream();
    let visible = drain_to_visible(stream, std::time::Duration::from_millis(200)).await;

    assert_eq!(
        visible,
        vec![
            "first".to_string(),
            "second".to_string(),
            "slot".to_string(),
        ],
        "real items first, slot last",
    );
}

#[tokio::test]
async fn signal_vec_omits_slot_when_none() {
    let view = ReactiveView::new_static_with_layout(
        vec![ReactiveViewModel::leaf(
            "text",
            Value::String("only".into()),
        )],
        variant(),
    );
    let stream = view.children_signal_vec().to_stream();
    let visible = drain_to_visible(stream, std::time::Duration::from_millis(200)).await;
    assert_eq!(visible, vec!["only".to_string()]);
}
