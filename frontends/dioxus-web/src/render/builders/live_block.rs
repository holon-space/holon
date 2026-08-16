use std::cell::RefCell;
use std::rc::Rc;

use holon_frontend::live_block_ancestors::LiveBlockAncestors;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::render::EntityContext;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::LiveBlock { block_id, content } = &node.kind else {
        return rsx! {};
    };
    // Each LiveBlockNode owns a worker-side `engineWatchView` subscription, so
    // an A→B→A embed without this guard costs a round trip and a live watch
    // per level, forever. The chain arrives through the Dioxus context an
    // enclosing LiveBlockNode provides; the outermost one sees none.
    let ancestors = try_consume_context::<LiveBlockAncestors>().unwrap_or_default(); // ALLOW(ok): the root live_block has no enclosing node, so an absent context IS the empty chain
    if ancestors.would_cycle(block_id) {
        tracing::warn!(
            "[live_block] '{block_id}' would create a cycle (ancestors={:?}) — rendering empty",
            ancestors.as_slice()
        );
        return rsx! {};
    }
    // Keyed single-item list: when a position-reused scope is handed a
    // different block_id, Dioxus drops the old LiveBlockNode (releasing its
    // subscription + EntityContext via use_drop) and mounts a fresh one,
    // instead of leaving the once-only hooks bound to the previous block.
    let entries = [(
        block_id.clone(),
        (**content).clone(),
        ancestors.pushed(block_id),
    )];
    rsx! {
        for (bid , c , chain) in entries {
            LiveBlockNode {
                key: "{bid}",
                block_id: bid.clone(),
                content: c.clone(),
                ancestors: chain.clone(),
            }
        }
    }
}

/// Shared between the subscribe task and `use_drop`. Deliberately NOT a
/// Dioxus signal: it must stay usable after scope teardown so an
/// `engineWatchView` RPC that is still in flight at unmount can learn its
/// handle and release the worker-side subscription instead of leaking it.
struct WatchState {
    handle: Option<u32>,
    unmounted: bool,
}

/// Renders a `live_block` and provides its `block_id` to descendants via
/// `EntityContext`, so an `editable_text` inside the subtree knows which
/// entity to mutate.
///
/// Each LiveBlockNode owns its own `engineWatchView` subscription for the
/// target block and renders whatever ViewModel snapshots arrive. The
/// `content` prop from the parent snapshot is the initial render, shown until
/// the cell's own subscription delivers its first snapshot.
#[component]
fn LiveBlockNode(block_id: String, content: ViewModel, ancestors: LiveBlockAncestors) -> Element {
    use_context_provider(|| EntityContext(block_id.clone()));
    use_context_provider(|| ancestors.clone());

    let mut inner_vm: Signal<Option<ViewModel>> = use_signal(|| None);

    let watch: Rc<RefCell<WatchState>> = use_hook(|| {
        let state = Rc::new(RefCell::new(WatchState {
            handle: None,
            unmounted: false,
        }));
        let state_task = state.clone();
        let bid = block_id.clone();
        // Detached task, not a use_future: a scope-cancelled task dies at
        // the `call` await and never learns the watch handle, so an unmount
        // while the RPC is in flight would leak the worker subscription
        // forever. This task always receives the handle and drops the
        // subscription itself when the scope is already gone.
        wasm_bindgen_futures::spawn_local(async move {
            let bridge = loop {
                if state_task.borrow().unmounted {
                    return;
                }
                if let Some(b) = crate::BRIDGE.with(|b| b.borrow().clone()) {
                    break b;
                }
                gloo_timers::future::TimeoutFuture::new(16).await;
            };

            let handle_val = match bridge.call("engineWatchView", [bid.clone().into()]).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[live_block] watch {bid} failed: {e}");
                    return;
                }
            };
            let handle = match handle_val.as_f64() {
                Some(h) if h >= 1.0 => h as u32,
                other => {
                    tracing::error!("[live_block] bogus watch handle for {bid}: {other:?}");
                    return;
                }
            };

            if state_task.borrow().unmounted {
                if let Err(e) = bridge.drop_subscription(handle).await {
                    tracing::error!(
                        "[live_block] drop_subscription({handle}) after unmount race: {e}"
                    );
                }
                return;
            }
            state_task.borrow_mut().handle = Some(handle);
            bridge.on_snapshot(handle, move |json| {
                match serde_json::from_str::<holon_frontend::view_model::WatchEnvelope>(&json) {
                    Ok(env) => {
                        inner_vm.set(Some(env.view_model));
                        // Idempotent (no-ops when the root watcher already
                        // applied the same state); a sub-block emission can
                        // carry a focus change the coalesced root emission
                        // hasn't delivered yet.
                        crate::editor::worker_focus::apply(env.focused_block, env.caret_offset);
                    }
                    Err(e) => {
                        tracing::error!("[live_block] snapshot parse: {e}")
                    }
                }
            });
        });
        state
    });

    use_drop(move || {
        let handle = {
            let mut st = watch.borrow_mut();
            st.unmounted = true;
            st.handle.take()
        };
        let Some(handle) = handle else { return };
        let Some(bridge) = crate::BRIDGE.with(|b| b.borrow().clone()) else {
            tracing::error!("[live_block] BRIDGE gone before drop_subscription({handle})");
            return;
        };
        // Synchronously, before this scope's signals are dropped, so a
        // queued snapshot can never fire into a dead `inner_vm`.
        bridge.remove_snapshot_listener(handle);
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = bridge.drop_subscription(handle).await {
                tracing::error!("[live_block] drop_subscription({handle}): {e}");
            }
        });
    });

    let live = inner_vm.read().clone();
    let displayed = live.unwrap_or_else(|| content.clone());

    rsx! {
        div { "data-block-id": "{block_id}",
            RenderNode { node: displayed }
        }
    }
}
