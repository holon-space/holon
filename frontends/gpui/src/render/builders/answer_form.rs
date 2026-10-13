use std::time::Duration;

use gpui::Entity;
use gpui::Window;
use gpui_component::input::Input;
use gpui_component::input::InputState;
use holon_api::Value;
use holon_api::user_transition::AskRow;
use holon_frontend::answer::AnswerForm;
use holon_frontend::answer::Answering;
use holon_frontend::answer::FormPhase;
use holon_frontend::answer::hash_hex;
use holon_frontend::param_collection::CollectStep;

use super::prelude::*;
use crate::geometry::BoundsRegistry;
use crate::geometry::TransparentTracker;

/// The answering port the `answer_form` builder uses. Absent, the form uses
/// [`Answering::unwired`], whose every call fails loud with `NotImplemented`.
pub struct AnswerPort(pub Answering);

impl gpui::Global for AnswerPort {}

const POLL_EVERY: Duration = Duration::from_millis(100);

/// The form for one ask (prop `ask`: the [`AskRow`] as JSON). It stays open
/// until the answer settles, shows a failure or an expiry, and closes only on
/// `Done`.
pub struct AnswerFormView {
    form: AnswerForm,
    answering: Answering,
    input: Option<Entity<InputState>>,
    bounds: BoundsRegistry,
}

impl AnswerFormView {
    fn new(
        ask: AskRow,
        answering: Answering,
        bounds: BoundsRegistry,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let form = AnswerForm::open(ask, &*answering.transitions);
        let input = matches!(form.step(), CollectStep::Enter { .. })
            .then(|| cx.new(|cx| InputState::new(window, cx)));
        Self {
            form,
            answering,
            input,
            bounds,
        }
    }

    fn enter(&mut self, raw: Value, cx: &mut gpui::Context<Self>) {
        self.form
            .enter(&raw, &*self.answering.transitions, &*self.answering.input);
        self.after_step(cx);
    }

    fn submit_text(&mut self, cx: &mut gpui::Context<Self>) {
        let text = self
            .input
            .as_ref()
            .expect("a text ask has an input")
            .read(cx)
            .value()
            .to_string();
        self.enter(Value::String(text), cx);
    }

    /// Starts polling once an answer is in flight; the loop ends when it
    /// settles.
    fn after_step(&mut self, cx: &mut gpui::Context<Self>) {
        if matches!(self.form.phase(), FormPhase::Awaiting(_)) {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL_EVERY).await;
                    let awaiting = this.update(cx, |view, cx| {
                        view.form.poll(&*view.answering.transitions);
                        cx.notify();
                        matches!(view.form.phase(), FormPhase::Awaiting(_))
                    });
                    // Settled, or the view is gone and nobody is left to show the outcome.
                    if !matches!(awaiting, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn tracked(
        &self,
        id: &str,
        widget: &'static str,
        text: String,
        el: impl IntoElement,
    ) -> AnyElement {
        TransparentTracker::new(
            id.to_string(),
            widget,
            self.bounds.clone(),
            el.into_any_element(),
        )
        .with_displayed_text(text)
        .into_any_element()
    }

    fn button(
        &self,
        id: &str,
        label: String,
        cx: &mut gpui::Context<Self>,
        on_click: impl Fn(&mut Self, &mut gpui::Context<Self>) + 'static,
    ) -> AnyElement {
        let el = div()
            .id(hashed_id(id))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .border_1()
            .cursor_pointer()
            .child(label.clone())
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_click(this, cx)),
            );
        self.tracked(id, "answer_button", label, el)
    }

    fn editing(&self, error: Option<&String>, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        match self.form.step() {
            CollectStep::Collect { choices, .. } => {
                for choice in choices {
                    let id = format!("answer-choice-{}", choice.slug);
                    let value = choice.value.clone();
                    let row = div()
                        .id(hashed_id(&id))
                        .flex()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .child("○")
                        .child(choice.label.clone())
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.enter(value.clone(), cx)),
                        );
                    out.push(self.tracked(&id, "answer_choice", choice.label, row));
                }
            }
            CollectStep::Enter { .. } => {
                let input = self.input.as_ref().expect("a text ask has an input");
                out.push(self.tracked(
                    "answer-input",
                    "answer_input",
                    input.read(cx).value().to_string(),
                    div().w_full().child(Input::new(input)),
                ));
                out.push(
                    self.button("answer-submit", "Answer".into(), cx, |this, cx| {
                        this.submit_text(cx)
                    }),
                );
            }
            CollectStep::Ready(_) => {
                out.push(
                    self.button("answer-submit", "Answer".into(), cx, |this, cx| {
                        this.form.submit(&*this.answering.transitions);
                        this.after_step(cx);
                    }),
                );
            }
            CollectStep::Unsupported { reason, .. } => {
                out.push(self.tracked(
                    "answer-error",
                    "answer_error",
                    reason.clone(),
                    div().child(reason),
                ));
            }
        }
        if let Some(error) = error {
            out.push(self.tracked(
                "answer-error",
                "answer_error",
                error.clone(),
                div().text_color(gpui::red()).child(error.clone()),
            ));
        }
        out
    }
}

impl gpui::Render for AnswerFormView {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let mut root = div().w_full().flex().flex_col().gap(px(4.0));
        if self.form.is_closed() {
            return root;
        }
        root = root.child(div().child(self.form.ask().transition.0.clone()));
        if let Some(preview) = self.form.preview() {
            let text = match preview {
                Ok(text) => text.clone(),
                Err(e) => format!("No preview: {e}"),
            };
            root = root.child(self.tracked(
                "answer-preview",
                "answer_preview",
                text.clone(),
                div().child(text),
            ));
        }
        let phase = self.form.phase().clone();
        match phase {
            FormPhase::Editing { error } => root = root.children(self.editing(error.as_ref(), cx)),
            FormPhase::Confirming { .. } => {
                let question = "This cannot be undone. Answer anyway?".to_string();
                root = root
                    .child(div().child(question))
                    .child(
                        self.button("answer-confirm", "Confirm".into(), cx, |this, cx| {
                            this.form.confirm(&*this.answering.transitions);
                            this.after_step(cx);
                        }),
                    )
                    .child(
                        self.button("answer-cancel", "Cancel".into(), cx, |this, cx| {
                            this.form.cancel();
                            cx.notify();
                        }),
                    );
            }
            FormPhase::Awaiting(_) => {
                let text = "Working…".to_string();
                root = root.child(self.tracked(
                    "answer-status",
                    "answer_status",
                    text.clone(),
                    div().child(text),
                ));
            }
            FormPhase::Settled(outcome) => {
                let text = outcome.text();
                root = root.child(self.tracked(
                    "answer-outcome",
                    "answer_outcome",
                    text.clone(),
                    div().child(text),
                ));
            }
        }
        root
    }
}

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let json = node
        .prop_str("ask")
        .expect("answer_form node carries its `ask` prop");
    let ask: AskRow = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("answer_form `ask` prop is not an AskRow ({e}): {json}"));
    let key = crate::entity_view_registry::CacheKey::Ephemeral(format!(
        "answer-form:{}:{}:{}",
        ask.transition.0,
        ask.subject.0,
        hash_hex(&ask.hash)
    ));
    let bounds = ctx.bounds_registry.clone();
    let any = ctx.local.get_or_create(key, || {
        ctx.with_gpui(|window, cx| {
            let answering = cx
                .try_global::<AnswerPort>()
                .map(|port| port.0.clone())
                .unwrap_or_else(Answering::unwired);
            cx.new(|cx| AnswerFormView::new(ask, answering, bounds, window, cx))
                .into_any()
        })
    });
    let entity: Entity<AnswerFormView> = any.downcast().expect("answer_form cache type mismatch");
    div().w_full().child(entity).into_any_element()
}
