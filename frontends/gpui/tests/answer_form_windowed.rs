//! Windowed rung for the `answer_form` widget (#167 lane U): an `Enum` ask
//! renders one radio row per variant and answers with the clicked one; a text
//! ask that fails to parse shows the refusal inline and fires nothing.
//!
//! Run: cargo nextest run -p holon-gpui --test main answer_form_windowed::

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use gpui::px;
use holon_api::EntityName;
use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskHash;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::InputContext;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_api::user_transition::UserTransitions;
use holon_frontend::RenderContext;
use holon_frontend::answer::Answering;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::entity_view_registry::LocalEntityScope;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::render::builders::AnswerPort;
use holon_gpui::render::builders::GpuiRenderContext;
use holon_gpui::render::builders::{self};

use crate::support;

struct FakePort {
    answered: Mutex<Vec<AnswerCall>>,
    script: Mutex<VecDeque<AnswerSettlement>>,
}

impl UserTransitions for FakePort {
    fn asks(&self) -> Result<Vec<AskRow>, UftError> {
        unreachable!("the form never lists asks")
    }
    fn preview(&self, _: &AskRow) -> Result<String, UftError> {
        unreachable!("these asks have no preview")
    }
    fn answer(&self, call: AnswerCall) -> Result<AnswerId, UftError> {
        let id = call.answer.clone();
        self.answered.lock().unwrap().push(call);
        Ok(id)
    }
    fn settlement(&self, _: &AnswerId) -> Result<AnswerSettlement, UftError> {
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AnswerSettlement::Waiting))
    }
}

struct NoNewtypes;

impl InputContext for NoNewtypes {
    fn parse_newtype(&self, type_name: &str, _: &str) -> Result<Value, String> {
        panic!("no newtype '{type_name}' in this rung")
    }
    fn place_holds(&self, _: &str, _: &str) -> Result<bool, NotImplemented> {
        panic!("no place in this rung")
    }
    fn entity_exists(&self, _: &EntityName, _: &str) -> Result<bool, NotImplemented> {
        panic!("no entity id in this rung")
    }
    fn row_key_exists(&self, _: &str) -> Result<bool, NotImplemented> {
        panic!("no row key in this rung")
    }
}

struct Fixture {
    vm: Arc<ReactiveViewModel>,
    services: Arc<dyn BuilderServices>,
    bounds: BoundsRegistry,
    cache: holon_gpui::entity_view_registry::EntityCache,
}

impl gpui::Render for Fixture {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        self.bounds.begin_pass();
        let gctx = GpuiRenderContext::new(
            RenderContext::default(),
            self.services.clone(),
            self.bounds.clone(),
            LocalEntityScope::new().with_cache(self.cache.clone()),
            NavigationState::new(),
            window,
            cx,
        );
        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .child(builders::render(&self.vm, &gctx))
    }
}

fn ask(input: TypeHint) -> AskRow {
    AskRow {
        transition: TransitionName("pick_colour".into()),
        subject: SubjectName("a.org".into()),
        hash: AskHash([3; 32]),
        input: Some(input),
        irreversible: false,
        has_preview: false,
    }
}

fn mount(
    cx: &mut TestAppContext,
    ask: AskRow,
) -> (Arc<FakePort>, BoundsRegistry, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let port = Arc::new(FakePort {
        answered: Mutex::new(Vec::new()),
        script: Mutex::new(VecDeque::new()),
    });
    let answering = Answering {
        transitions: port.clone(),
        input: Arc::new(NoNewtypes),
    };
    cx.update(|cx| cx.set_global(AnswerPort(answering)));

    let props = HashMap::from([(
        "ask".to_string(),
        Value::String(serde_json::to_string(&ask).unwrap()),
    )]);
    let vm = Arc::new(ReactiveViewModel::from_widget("answer_form", props));
    let services: Arc<dyn BuilderServices> = support::TestServices::new();
    let bounds = BoundsRegistry::new();
    let (_root, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |window, cx| {
            let fixture = cx.new(|_| Fixture {
                vm,
                services,
                bounds,
                cache: Default::default(),
            });
            gpui_component::Root::new(fixture, window, cx)
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    (port, bounds, vcx)
}

/// Clicks the element tracked as `id`. Tolerant of a missing element, so the
/// assertions (not the gesture) report what is missing.
fn click(vcx: &mut VisualTestContext, bounds: &BoundsRegistry, id: &str) {
    if let Some(info) = bounds.element_info(id) {
        let at = gpui::point(
            px(info.x + info.width / 2.0),
            px(info.y + info.height / 2.0),
        );
        vcx.simulate_mouse_move(at, None, Default::default());
        vcx.simulate_click(at, Default::default());
        vcx.run_until_parked();
        bounds.flush();
    }
}

#[gpui::test]
fn enum_ask_renders_radio_rows_and_answers_the_clicked_one(cx: &mut TestAppContext) {
    let (port, bounds, vcx) = mount(
        cx,
        ask(TypeHint::Enum {
            type_name: "Color".into(),
            variants: vec!["red".into(), "blue".into()],
        }),
    );
    for slug in ["red", "blue"] {
        let id = format!("answer-choice-{slug}");
        let info = bounds.element_info(&id);
        assert!(
            info.is_some(),
            "no radio row '{id}'; tracked: {:?}",
            bounds
                .all_elements()
                .into_iter()
                .map(|(k, _)| k)
                .collect::<Vec<_>>()
        );
        assert_eq!(&*info.unwrap().widget_type, "answer_choice");
    }

    click(vcx, &bounds, "answer-choice-blue");
    let answered = port.answered.lock().unwrap().clone();
    assert_eq!(answered.len(), 1, "the clicked radio did not answer");
    assert_eq!(answered[0].value, Some(Value::String("blue".into())));
    let status = bounds
        .element_info("answer-status")
        .and_then(|i| i.displayed_text.map(|t| t.to_string()));
    assert_eq!(
        status.as_deref(),
        Some("Working…"),
        "an accepted answer must wait for its settlement"
    );
}

#[gpui::test]
fn a_parse_error_renders_inline_and_fires_nothing(cx: &mut TestAppContext) {
    let (port, bounds, vcx) = mount(cx, ask(TypeHint::Number));
    click(vcx, &bounds, "answer-input");
    vcx.simulate_keystrokes("x");
    vcx.run_until_parked();
    click(vcx, &bounds, "answer-submit");

    let error = bounds
        .element_info("answer-error")
        .and_then(|i| i.displayed_text.map(|t| t.to_string()));
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("expected an integer")),
        "the parse error is not shown inline: {error:?}"
    );
    assert!(
        port.answered.lock().unwrap().is_empty(),
        "a parse error fired an answer"
    );
}
