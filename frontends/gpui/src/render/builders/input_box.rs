use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::Entity;
use gpui::Subscription;
use gpui::Window;
use gpui_component::button::Button;
use gpui_component::input::Enter;
use gpui_component::input::Input;
use gpui_component::input::InputEvent;
use gpui_component::input::InputState;
use holon_api::Value;
use holon_api::render_types::OperationWiring;
use holon_frontend::ReactiveViewModel;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;

use super::prelude::*;

/// The compose box. Its authority on what the user has typed is the
/// ViewModel's `draft` `Mutable`; the `InputState` is the platform text
/// surface mirroring it, reseeded from the draft whenever this view is
/// (re)created. Submit dispatches the node's single `OperationWiring` with the
/// draft bound to `modified_param`, and clears the draft only once that
/// dispatch has come back `Ok`.
pub struct InputBoxView {
    input: Entity<InputState>,
    draft: Mutable<String>,
    wiring: OperationWiring,
    services: Arc<dyn BuilderServices>,
    submit_label: String,
    multiline: bool,
    _subscription: Subscription,
}

impl InputBoxView {
    fn new(
        placeholder: String,
        submit_label: String,
        multiline: bool,
        draft: Mutable<String>,
        wiring: OperationWiring,
        services: Arc<dyn BuilderServices>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let seed = draft.get_cloned();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                // Auto-grow (a multi-line mode) even for a single-line compose
                // box: the box grows with the message, and the single-line
                // shaper cannot represent a wrapped draft at all.
                .auto_grow(1, 8)
                .placeholder(placeholder)
                .default_value(&seed)
        });

        let subscription = cx.subscribe_in(&input, window, {
            let draft = draft.clone();
            move |this, entity, event, window, cx| match event {
                InputEvent::Change => draft.set(entity.read(cx).value().to_string()),
                // Reached only in `multiline` mode — the plain-Enter gesture is
                // captured before `InputState` sees it (see `render`). There,
                // Enter has already opened a new paragraph and only the
                // secondary chord sends.
                InputEvent::PressEnter { secondary } => {
                    if *secondary {
                        this.submit(window, cx);
                    }
                }
                InputEvent::Focus | InputEvent::Blur => {}
            }
        });

        Self {
            input,
            draft,
            wiring,
            services,
            submit_label,
            multiline,
            _subscription: subscription,
        }
    }

    /// Fire the wired operation with the draft as `modified_param`. Empty draft
    /// = nothing to send, so the gesture is inert.
    fn submit(&self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let text = self.draft.get_cloned();
        if text.is_empty() {
            return;
        }

        let mut params = self.wiring.descriptor.bound_params.clone();
        params.insert(self.wiring.modified_param.clone(), Value::String(text));
        let intent = OperationIntent::new(
            self.wiring.descriptor.entity_name.clone(),
            self.wiring.descriptor.name.clone(),
            params,
        );

        // Await the op's result rather than fire-and-forget: whether the draft
        // may be cleared IS the result. The tokio hop mirrors `EditorView`'s
        // awaitable dispatches — the op future needs the session's runtime, not
        // gpui's foreground executor.
        let services = self.services.clone();
        let draft = self.draft.clone();
        let input = self.input.clone();
        let rt = services.runtime_handle();
        let window_handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
            rt.spawn(async move {
                let outcome = services
                    .dispatch_intent_awaitable(intent)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(outcome);
            });
            let detail = match rx.await {
                Ok(Ok(())) => {
                    draft.set(String::new());
                    let _ = cx.update_window(window_handle, |_, window, cx| {
                        input.update(cx, |state, cx| state.set_value("", window, cx));
                    });
                    return;
                }
                // The draft is deliberately left alone on both arms — a user
                // never loses typed text because the backend said no, or
                // because the dispatch task vanished.
                Ok(Err(e)) => e,
                Err(e) => format!("compose dispatch did not report a result: {e}"),
            };
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                crate::share_ui::DegradedToastSink::push(
                    crate::share_ui::DegradedToast {
                        kind: crate::share_ui::DegradedKind::CommandFailed,
                        shared_tree_id: "input_box".into(),
                        detail,
                        condition: None,
                    },
                    cx,
                );
            });
        })
        .detach();
    }
}

impl gpui::Render for InputBoxView {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let mut root = div().w_full().flex().flex_row().items_end().gap(px(6.0));
        // Capture phase: an ancestor of the `InputState` element, so a
        // single-line compose box consumes Enter as SEND before the input can
        // insert a newline into the draft.
        if !self.multiline {
            root = root.capture_action(cx.listener(|this, _: &Enter, window, cx| {
                this.submit(window, cx);
                cx.stop_propagation();
            }));
        }
        root.child(div().flex_1().child(Input::new(&self.input)))
            .child(
                Button::new("input-box-submit")
                    .label(self.submit_label.clone())
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
    }
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let draft = node
        .draft
        .clone()
        .expect("input_box node carries a draft buffer");
    let wiring = node
        .operations
        .first()
        .cloned()
        .expect("input_box node carries its submit wiring");
    let placeholder = node.prop_str("placeholder").unwrap_or_default();
    let submit_label = node
        .prop_str("submit_label")
        .unwrap_or_else(|| "Send".to_string());
    let multiline = node.prop_bool("multiline").unwrap_or(false);

    // Ephemeral: a structural rebuild drops the `InputState`, and the view is
    // rebuilt reseeded from `draft` — which is why the draft, not the input,
    // is the authority.
    let el_id = format!(
        "input-box-{}-{}",
        node.row_id().unwrap_or_default(),
        wiring.descriptor.name
    );
    let services = ctx.services.clone();
    let key = crate::entity_view_registry::CacheKey::Ephemeral(el_id);
    let any = ctx.local.get_or_create(key, || {
        ctx.with_gpui(|window, cx| {
            cx.new(|cx| {
                InputBoxView::new(
                    placeholder,
                    submit_label,
                    multiline,
                    draft,
                    wiring,
                    services,
                    window,
                    cx,
                )
            })
            .into_any()
        })
    });
    let entity: gpui::Entity<InputBoxView> = any.downcast().expect("input_box cache type mismatch");
    div().w_full().child(entity).into_any_element()
}
