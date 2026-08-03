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
use holon_core::Delivery;
use holon_api::render_types::OperationWiring;
use holon_frontend::ReactiveViewModel;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::SendState;
use holon_frontend::reactive_view_model::SendStripKind;

use super::prelude::*;

/// The compose box. Its authority on what the user has typed is the
/// ViewModel's `draft` `Mutable`; the `InputState` is the platform text
/// surface mirroring it, reseeded from the draft whenever this view is
/// (re)created. Submit dispatches the node's single `OperationWiring` with the
/// draft bound to `modified_param`, and clears the draft only once that
/// dispatch has come back `Ok`.
pub struct InputBoxView {
    input: Entity<InputState>,
    bounds: crate::geometry::BoundsRegistry,
    draft: Mutable<String>,
    send_state: Mutable<SendState>,
    wiring: OperationWiring,
    services: Arc<dyn BuilderServices>,
    submit_label: String,
    multiline: bool,
    _subscription: Subscription,
}

impl InputBoxView {
    fn new(
        bounds: crate::geometry::BoundsRegistry,
        placeholder: String,
        submit_label: String,
        multiline: bool,
        draft: Mutable<String>,
        send_state: Mutable<SendState>,
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
            bounds,
            draft,
            send_state,
            wiring,
            services,
            submit_label,
            multiline,
            _subscription: subscription,
        }
    }

    /// Fire the wired operation with the draft as `modified_param`. Inert on an
    /// empty draft (nothing to send) and while a send is already in flight —
    /// the wired op is an irreversible external effect, so two fast Enters must
    /// not become two messages.
    fn submit(&self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let text = self.draft.get_cloned();
        if text.is_empty() || *self.send_state.lock_ref() == SendState::InFlight {
            return;
        }
        self.send_state.set(SendState::InFlight);

        let mut params = self.wiring.descriptor.bound_params.clone();
        params.insert(
            self.wiring.modified_param.clone(),
            Value::String(text.clone()),
        );
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
        let send_state = self.send_state.clone();
        let input = self.input.clone();
        let rt = services.runtime_handle();
        let window_handle = window.window_handle();
        let sent_text = text;
        cx.spawn(async move |this, cx| {
            let (tx, rx) = tokio::sync::oneshot::channel::<Result<Delivery, String>>();
            rt.spawn(async move {
                let outcome = services
                    .dispatch_intent_awaitable(intent)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(outcome);
            });
            let detail = match rx.await {
                // Proven delivery is the ONLY outcome that clears the box.
                Ok(Ok(Delivery::Proven)) => {
                    send_state.set(SendState::Idle);
                    draft.set(String::new());
                    let _ = cx.update_window(window_handle, |_, window, cx| {
                        input.update(cx, |state, cx| state.set_value("", window, cx));
                    });
                    let _ = this.update(cx, |_, cx| cx.notify());
                    return;
                }
                // Dispatched but unproven: neither success nor failure. The
                // strip says so in the provider's own words and offers no
                // retry — the transport is `retry_safe:false`.
                // Disclosed by the strip, not by a toast: a toast fades, and
                // an unproven send stays unproven until the message shows up in
                // the transcript.
                Ok(Ok(Delivery::Unproven { detail })) => {
                    send_state.set(SendState::Unconfirmed {
                        at: local_clock_time(),
                        message: sent_text,
                        detail,
                    });
                    let _ = this.update(cx, |_, cx| cx.notify());
                    return;
                }
                // The draft is deliberately left alone on both arms — a user
                // never loses typed text because the backend said no, or
                // because the dispatch task vanished.
                Ok(Err(e)) => e,
                Err(e) => format!("compose dispatch did not report a result: {e}"),
            };
            send_state.set(SendState::Failed {
                message: detail.clone(),
            });
            let _ = this.update(cx, |_, cx| cx.notify());
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

impl InputBoxView {
    /// The pending-send strip, styled so it cannot be mistaken for a chat
    /// bubble: it is the compose box's own status line, not a message in the
    /// transcript. Carries no retry affordance in any state — the transport is
    /// `retry_safe:false`, so a second dispatch is a second message in a live
    /// session.
    fn status_strip(&self) -> Option<AnyElement> {
        let strip = self.send_state.lock_ref().strip()?;
        let (border, tint) = match strip.kind {
            SendStripKind::Sending => (gpui::hsla(0.0, 0.0, 0.5, 0.5), gpui::hsla(0.0, 0.0, 0.5, 0.08)),
            SendStripKind::Unconfirmed => {
                (gpui::hsla(0.11, 0.9, 0.5, 0.9), gpui::hsla(0.11, 0.9, 0.5, 0.12))
            }
            SendStripKind::Refused => {
                (gpui::hsla(0.0, 0.75, 0.5, 0.9), gpui::hsla(0.0, 0.75, 0.5, 0.12))
            }
        };
        let painted = format!("{}\n{}", strip.headline, strip.detail);
        let el = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(4.0))
            .border_l(px(3.0))
            .border_color(border)
            .bg(tint)
            .child(div().text_xs().child(strip.headline.clone()))
            .child(div().text_xs().child(strip.detail.clone()))
            .into_any_element();
        let seq = self.bounds.next_seq();
        Some(
            crate::geometry::TransparentTracker::new(
                format!("{}#{seq}", strip.kind.widget_type()),
                strip.kind.widget_type(),
                self.bounds.clone(),
                el,
            )
            .with_displayed_text(painted)
            .into_any_element(),
        )
    }
}

impl gpui::Render for InputBoxView {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let strip = self.status_strip();
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
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(strip)
            .child(
                root.child(div().flex_1().child(Input::new(&self.input)))
                    .child(
                        Button::new("input-box-submit")
                            .label(self.submit_label.clone())
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

/// Wall-clock time of day, for the "sent, not acknowledged" strip. A strip
/// that outlives the send must say HOW stale it is.
fn local_clock_time() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// A node's view-cache identity: the address of the draft buffer it owns.
///
/// The draft is minted once per `input_box` node and carried across structural
/// rebuilds by `ReactiveViewModel::with_update`, so it is stable exactly when
/// the node is. The cached view holds a clone of the same `Mutable`, so while
/// an entry lives its address cannot be reused by another node — two boxes can
/// never collide.
fn draft_identity(draft: &Mutable<String>) -> usize {
    &*draft.lock_ref() as *const String as usize
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let draft = node
        .draft
        .clone()
        .expect("input_box node carries a draft buffer");
    let send_state = node
        .send_state
        .clone()
        .expect("input_box node carries its send state");
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

    // Ephemeral, keyed on the node's OWN draft buffer. A compose box carries no
    // row id, so a (row, operation) key collapses every sibling wired to the
    // same op onto one view — and one view is one buffer, so box A would show
    // box B's text.
    let key = crate::entity_view_registry::CacheKey::Ephemeral(format!(
        "input-box-{:x}",
        draft_identity(&draft)
    ));
    let services = ctx.services.clone();
    let ctx_bounds = ctx.bounds_registry.clone();
    let any = ctx.local.get_or_create(key, || {
        ctx.with_gpui(|window, cx| {
            cx.new(|cx| {
                InputBoxView::new(
                    ctx_bounds,
                    placeholder,
                    submit_label,
                    multiline,
                    draft,
                    send_state,
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
