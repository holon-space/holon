use std::sync::Arc;

use futures::StreamExt;
use holon_frontend::reactive::ReactiveEngine;

use super::prelude::*;
use crate::render::EntityContext;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::LiveBlock { block_id, content } = &node.kind else {
        return rsx! {};
    };
    let block_id = block_id.clone();
    let content = (**content).clone();
    rsx! { LiveBlockNode { block_id, content } }
}

/// Renders a `live_block` and provides its `block_id` to descendants via
/// `Signal<EntityContext>`, so an `editable_text` inside the subtree knows
/// which entity to mutate — and keeps knowing it when dioxus reuses this
/// scope for a different block after a sibling insertion/reorder.
///
/// Each LiveBlockNode owns its own in-process `ReactiveEngine::watch`
/// subscription for the target block and renders whatever `ViewModel`
/// snapshots arrive. The `content` prop from the parent snapshot is only used
/// as an initial render before the cell's own subscription delivers. When the
/// `block_id` prop changes, the watch task re-subscribes to the new block and
/// the provided `EntityContext` is updated in place.
#[component]
fn LiveBlockNode(block_id: String, content: ViewModel) -> Element {
    let mut entity_ctx: Signal<EntityContext> =
        use_context_provider(|| Signal::new(EntityContext(block_id.clone())));
    if entity_ctx.peek().0 != block_id {
        entity_ctx.set(EntityContext(block_id.clone()));
    }

    let engine: Arc<ReactiveEngine> = use_context();
    let rt: tokio::runtime::Handle = use_context();
    let mut inner_vm: Signal<Option<ViewModel>> = use_signal(|| None);

    // Bridge the tokio watch stream (Send) into a dioxus signal (!Send) via a
    // `tokio::sync::watch` channel — the same dance the root `App` uses. The
    // id channel lets a reused scope drop its old subscription and re-watch
    // the new block when the `block_id` prop changes.
    let (id_tx, watch_rx) = use_hook({
        let bid = block_id.clone();
        let rt = rt.clone();
        move || {
            let (id_tx, mut id_rx) = tokio::sync::watch::channel::<String>(bid);
            let (tx, rx) = tokio::sync::watch::channel::<Option<ViewModel>>(None);
            let engine = engine.clone();
            rt.spawn(async move {
                loop {
                    let bid = id_rx.borrow_and_update().clone();
                    let uri = holon_api::EntityUri::parse(&bid)
                        .expect("live_block: invalid entity URI");
                    let mut stream = engine.watch(&uri);
                    loop {
                        tokio::select! {
                            changed = id_rx.changed() => {
                                if changed.is_err() || tx.send(None).is_err() {
                                    return;
                                }
                                break;
                            }
                            rvm = stream.next() => {
                                let Some(rvm) = rvm else { return };
                                if tx.send(Some(rvm.snapshot())).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            });
            (Arc::new(id_tx), rx)
        }
    });

    if *id_tx.borrow() != block_id {
        id_tx
            .send(block_id.clone())
            .expect("live_block: watch task terminated");
    }

    use_future(move || {
        let mut rx = watch_rx.clone();
        async move {
            while rx.changed().await.is_ok() {
                let vm = rx.borrow_and_update().clone();
                inner_vm.set(vm);
            }
        }
    });

    let displayed = inner_vm.read().clone().unwrap_or_else(|| content.clone());

    rsx! {
        div { "data-block-id": "{block_id}",
            RenderNode { node: displayed }
        }
    }
}
