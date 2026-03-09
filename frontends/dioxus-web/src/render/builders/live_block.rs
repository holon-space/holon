use super::prelude::*;
use crate::render::EntityContext;

pub fn render(
    block_id: &String,
    content: &Box<ViewModel>,
    _ctx: &DioxusRenderContext,
) -> Element {
    let block_id = block_id.clone();
    let content = (**content).clone();
    rsx! { LiveBlockNode { block_id, content } }
}

/// Renders a `live_block` and provides its `block_id` to descendants via
/// `EntityContext`, so an `editable_text` inside the subtree knows which
/// entity to mutate.
///
/// Each LiveBlockNode owns its own `engineWatchView` subscription for the
/// target block and renders whatever ViewModel snapshots arrive. The
/// `content` prop from the parent snapshot is only used as an
/// initial/fallback render before the cell's own subscription delivers.
#[component]
fn LiveBlockNode(block_id: String, content: ViewModel) -> Element {
    use_context_provider(|| EntityContext(block_id.clone()));

    let mut inner_vm: Signal<Option<ViewModel>> = use_signal(|| None);
    let mut watch_handle: Signal<Option<u32>> = use_signal(|| None);

    let bid_for_future = block_id.clone();
    use_future(move || {
        let bid = bid_for_future.clone();
        async move {
            let bridge = loop {
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
            watch_handle.set(Some(handle));
            bridge.on_snapshot(handle, move |json| {
                match serde_json::from_str::<ViewModel>(&json) {
                    Ok(v) => inner_vm.set(Some(v)),
                    Err(e) => {
                        tracing::error!("[live_block] snapshot parse: {e}")
                    }
                }
            });
        }
    });

    use_drop(move || {
        if let Some(handle) = watch_handle.peek().as_ref().copied() {
            wasm_bindgen_futures::spawn_local(async move {
                let bridge = crate::BRIDGE.with(|b| b.borrow().clone());
                if let Some(bridge) = bridge {
                    if let Err(e) = bridge.drop_subscription(handle).await {
                        tracing::error!("[live_block] drop_subscription({handle}): {e}");
                    }
                }
            });
        }
    });

    let live = inner_vm.read().clone();
    let displayed = live.unwrap_or_else(|| content.clone());

    rsx! {
        div { "data-block-id": "{block_id}",
            RenderNode { node: displayed }
        }
    }
}
