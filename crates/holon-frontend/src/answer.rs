//! Answering a `manual` transition's ask: the form state machine the GPUI
//! `answer_form` builder renders, and the agent path MCP calls.
//!
//! An answer is fire-and-await. [`UserTransitions::answer`] only ACCEPTS the
//! call; the form stays open until [`UserTransitions::settlement`] reports the
//! settled outcome, so a failed or expired answer is never a silent close.
//! The answer value goes through the shared [`ParamCollector`], which parses
//! it at the boundary with [`parse_input`].

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use holon_api::Value;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationParam;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskHash;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::ExpireReason;
use holon_api::user_transition::InputContext;
use holon_api::user_transition::InputParseError;
use holon_api::user_transition::Lane;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_api::user_transition::Unwired;
use holon_api::user_transition::UserTransitions;
use holon_api::user_transition::parse_input;

use crate::operations::OperationIntent;
use crate::param_collection::CollectStep;
use crate::param_collection::ParamCollector;

/// The entity of the generic answer operation (ruling 9).
pub const ASK_ENTITY: &str = "ask";
/// The one generic call: `answer(transition, subject, seen_hash, value)`.
pub const ANSWER_OP: &str = "answer";
pub const PARAM_TRANSITION: &str = "transition";
pub const PARAM_SUBJECT: &str = "subject";
pub const PARAM_SEEN_HASH: &str = "seen_hash";
pub const PARAM_VALUE: &str = "value";

/// What a surface needs to answer asks: the port and the boundary parsers.
#[derive(Clone)]
pub struct Answering {
    pub transitions: Arc<dyn UserTransitions>,
    pub input: Arc<dyn InputContext + Send + Sync>,
}

impl Answering {
    /// The stubs until lane I wires the executor: every call fails loud with
    /// `NotImplemented`.
    pub fn unwired() -> Self {
        Self {
            transitions: Arc::new(Unwired),
            input: Arc::new(UnwiredInput),
        }
    }
}

/// The [`InputContext`] stub until lane I registers newtype parsers and place
/// membership.
pub struct UnwiredInput;

impl InputContext for UnwiredInput {
    fn parse_newtype(&self, type_name: &str, _: &str) -> Result<Value, String> {
        let missing = NotImplemented {
            lane: Lane::Integration,
            what: "newtype parsers",
        };
        Err(format!("{missing} (cannot parse '{type_name}')"))
    }

    // ponytail: refuses every place id until lane I wires place membership.
    fn place_holds(&self, _: &str, _: &str) -> bool {
        false
    }
}

pub fn hash_hex(hash: &AskHash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_hash_hex(text: &str) -> Result<AskHash, String> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 || !text.is_ascii() {
        return Err(format!("'{text}' is not a 64-digit hex ask hash"));
    }
    let mut out = [0u8; 32];
    for (i, pair) in bytes.chunks(2).enumerate() {
        let digits = std::str::from_utf8(pair).expect("ascii checked above");
        out[i] = u8::from_str_radix(digits, 16)
            .map_err(|_| format!("'{text}' is not a 64-digit hex ask hash"))?;
    }
    Ok(AskHash(out))
}

/// The answer operation for one ask. `transition`, `subject` and `seen_hash`
/// come bound from the ask; `value` (when the ask takes input) is collected.
pub fn answer_descriptor(ask: &AskRow) -> OperationDescriptor {
    let text = |name: &str, description: &str| OperationParam {
        name: name.to_string(),
        type_hint: TypeHint::String,
        description: description.to_string(),
    };
    let mut required_params = vec![
        text(PARAM_TRANSITION, "The transition that asks"),
        text(PARAM_SUBJECT, "The subject the ask is about"),
        text(
            PARAM_SEEN_HASH,
            "The hash of the ask as listed by asks_list",
        ),
    ];
    if let Some(hint) = &ask.input {
        required_params.push(OperationParam {
            name: PARAM_VALUE.to_string(),
            type_hint: hint.clone(),
            description: "The answer".to_string(),
        });
    }
    OperationDescriptor {
        entity_name: holon_api::EntityName::new(ASK_ENTITY),
        entity_short_name: ASK_ENTITY.into(),
        name: ANSWER_OP.into(),
        display_name: ask.transition.0.clone(),
        required_params,
        optional_params: vec![],
        param_mappings: vec![],
        id_column: "id".to_string(),
        description: format!("Answer '{}' on '{}'", ask.transition.0, ask.subject.0),
        affected_fields: vec![],
        target_scope: holon_api::TargetScope::Global,
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::PointerGesture,
        },
        trigger: None,
        bound_params: bound_params(ask),
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Undeclared,
    }
}

fn bound_params(ask: &AskRow) -> HashMap<String, Value> {
    HashMap::from([
        (
            PARAM_TRANSITION.to_string(),
            Value::String(ask.transition.0.clone()),
        ),
        (
            PARAM_SUBJECT.to_string(),
            Value::String(ask.subject.0.clone()),
        ),
        (
            PARAM_SEEN_HASH.to_string(),
            Value::String(hash_hex(&ask.hash)),
        ),
    ])
}

/// The call for a finished answer intent. The value in `intent` is already
/// parsed by the collector.
pub fn answer_call(ask: &AskRow, intent: &OperationIntent) -> AnswerCall {
    assert_eq!(intent.entity_name.as_str(), ASK_ENTITY);
    assert_eq!(intent.op_name, ANSWER_OP);
    let value = intent.params.get(PARAM_VALUE).cloned();
    assert_eq!(
        value.is_some(),
        ask.input.is_some(),
        "an answer carries a value iff its ask takes input"
    );
    AnswerCall {
        transition: ask.transition.clone(),
        subject: ask.subject.clone(),
        seen: ask.hash,
        answer: new_answer_id(),
        value,
    }
}

/// How one answer ended.
#[derive(Clone, Debug, PartialEq)]
pub enum AnswerOutcome {
    Done,
    Failed {
        cause: String,
    },
    Expired(ExpireReason),
    /// The port refused the call or the settlement lookup.
    Refused(UftError),
}

impl AnswerOutcome {
    /// What the form shows once the answer settled.
    pub fn text(&self) -> String {
        match self {
            AnswerOutcome::Done => "Done".to_string(),
            AnswerOutcome::Failed { cause } => format!("Failed: {cause}"),
            AnswerOutcome::Expired(ExpireReason::AskGone) => {
                "The question no longer applies".to_string()
            }
            AnswerOutcome::Expired(ExpireReason::SituationChanged) => {
                "The question no longer applies: the situation changed".to_string()
            }
            AnswerOutcome::Refused(e) => format!("Not answered: {e}"),
        }
    }
}

/// One step of an answer in flight.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Pending(AnswerId),
    Settled(AnswerOutcome),
}

/// Sends `call`. An accepted call is only `Pending`: the outcome comes from
/// [`poll`].
pub fn fire(port: &dyn UserTransitions, call: AnswerCall) -> Progress {
    match port.answer(call) {
        Ok(id) => Progress::Pending(id),
        Err(e) => Progress::Settled(AnswerOutcome::Refused(e)),
    }
}

pub fn poll(port: &dyn UserTransitions, id: &AnswerId) -> Progress {
    match port.settlement(id) {
        Ok(AnswerSettlement::Waiting | AnswerSettlement::Working { .. }) => {
            Progress::Pending(id.clone())
        }
        Ok(AnswerSettlement::Done { .. }) => Progress::Settled(AnswerOutcome::Done),
        Ok(AnswerSettlement::Failed { cause, .. }) => {
            Progress::Settled(AnswerOutcome::Failed { cause })
        }
        Ok(AnswerSettlement::Expired { reason }) => {
            Progress::Settled(AnswerOutcome::Expired(reason))
        }
        Err(e) => Progress::Settled(AnswerOutcome::Refused(e)),
    }
}

/// Fires `call` and polls until it settles, calling `wait` between polls.
/// Runtime-agnostic: GPUI waits on its executor timer, MCP on tokio.
pub async fn fire_and_await<F, Fut>(
    port: &dyn UserTransitions,
    call: AnswerCall,
    mut wait: F,
) -> AnswerOutcome
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut progress = fire(port, call);
    loop {
        match progress {
            Progress::Settled(outcome) => return outcome,
            Progress::Pending(id) => {
                wait().await;
                progress = poll(port, &id);
            }
        }
    }
}

/// Where an [`AnswerForm`] is.
#[derive(Clone, Debug)]
pub enum FormPhase {
    /// Collecting the value; `error` is the last refused input.
    Editing {
        error: Option<String>,
    },
    /// An `irreversible` ask waits for the person to confirm `call`.
    Confirming {
        call: AnswerCall,
    },
    Awaiting(AnswerId),
    Settled(AnswerOutcome),
}

/// The form for one ask. Closes only on a settled `Done`.
#[derive(Clone, Debug)]
pub struct AnswerForm {
    ask: AskRow,
    collector: ParamCollector,
    /// Loaded when the form opens (ruling 10: an eager preview is fine).
    preview: Option<Result<String, UftError>>,
    phase: FormPhase,
}

impl AnswerForm {
    pub fn open(ask: AskRow, port: &dyn UserTransitions) -> Self {
        let preview = ask.has_preview.then(|| port.preview(&ask));
        Self {
            collector: collector_for(&ask),
            ask,
            preview,
            phase: FormPhase::Editing { error: None },
        }
    }

    pub fn ask(&self) -> &AskRow {
        &self.ask
    }

    pub fn phase(&self) -> &FormPhase {
        &self.phase
    }

    pub fn preview(&self) -> Option<&Result<String, UftError>> {
        self.preview.as_ref()
    }

    /// The collector's step while editing: the value's choices or text entry,
    /// or `Ready` for an ask without input.
    pub fn step(&self) -> CollectStep {
        self.collector.current()
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.phase, FormPhase::Settled(AnswerOutcome::Done))
    }

    /// Parses `raw` as the answer and submits it. A refusal stays in
    /// `Editing` with the error shown; nothing reaches the port.
    pub fn enter(&mut self, raw: &Value, port: &dyn UserTransitions, cx: &dyn InputContext) {
        self.assert_editing();
        match self.collector.enter(PARAM_VALUE, raw, cx) {
            Ok(()) => self.submit(port),
            Err(e) => {
                self.phase = FormPhase::Editing {
                    error: Some(e.to_string()),
                }
            }
        }
    }

    /// Submits the collected answer: an `irreversible` ask asks to confirm
    /// first, every other ask fires.
    pub fn submit(&mut self, port: &dyn UserTransitions) {
        self.assert_editing();
        let CollectStep::Ready(intent) = self.collector.current() else {
            panic!("submit before the answer value was collected");
        };
        let call = answer_call(&self.ask, &intent);
        if self.ask.irreversible {
            self.phase = FormPhase::Confirming { call };
        } else {
            self.fire(port, call);
        }
    }

    pub fn confirm(&mut self, port: &dyn UserTransitions) {
        let FormPhase::Confirming { call } = &self.phase else {
            panic!("confirm outside the confirm step: {:?}", self.phase);
        };
        let call = call.clone();
        self.fire(port, call);
    }

    /// Back from the confirm step to an empty form.
    pub fn cancel(&mut self) {
        assert!(matches!(self.phase, FormPhase::Confirming { .. }));
        self.collector = collector_for(&self.ask);
        self.phase = FormPhase::Editing { error: None };
    }

    /// Advances an answer in flight; a no-op in every other phase.
    pub fn poll(&mut self, port: &dyn UserTransitions) {
        if let FormPhase::Awaiting(id) = &self.phase {
            self.phase = match poll(port, id) {
                Progress::Pending(id) => FormPhase::Awaiting(id),
                Progress::Settled(outcome) => FormPhase::Settled(outcome),
            };
        }
    }

    fn fire(&mut self, port: &dyn UserTransitions, call: AnswerCall) {
        self.phase = match fire(port, call) {
            Progress::Pending(id) => FormPhase::Awaiting(id),
            Progress::Settled(outcome) => FormPhase::Settled(outcome),
        };
    }

    fn assert_editing(&self) {
        assert!(
            matches!(self.phase, FormPhase::Editing { .. }),
            "the answer is no longer editable: {:?}",
            self.phase
        );
    }
}

fn collector_for(ask: &AskRow) -> ParamCollector {
    let descriptor = answer_descriptor(ask);
    ParamCollector::for_op(&descriptor, &descriptor.bound_params)
}

/// Why an agent's answer did not go through.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentAnswerError {
    /// A required param is missing or malformed.
    Param(String),
    Input(InputParseError),
    Ask(UftError),
}

impl fmt::Display for AgentAnswerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentAnswerError::Param(msg) => write!(f, "{msg}"),
            AgentAnswerError::Input(e) => write!(f, "{e}"),
            AgentAnswerError::Ask(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AgentAnswerError {}

/// An agent's `answer(transition, subject, seen_hash, value)`. The ask must
/// still be listed; an `irreversible` ask is refused with
/// [`UftError::NeedsApproval`] (UQ2.a) so a person approves it in the UI.
pub fn agent_answer(
    answering: &Answering,
    params: &HashMap<String, Value>,
) -> Result<AnswerId, AgentAnswerError> {
    let text = |name: &str| -> Result<&str, AgentAnswerError> {
        params
            .get(name)
            .and_then(Value::as_string)
            .ok_or_else(|| AgentAnswerError::Param(format!("'{name}' (text) is required")))
    };
    let transition = TransitionName(text(PARAM_TRANSITION)?.to_string());
    let subject = SubjectName(text(PARAM_SUBJECT)?.to_string());
    let seen = parse_hash_hex(text(PARAM_SEEN_HASH)?).map_err(AgentAnswerError::Param)?;
    let ask = answering
        .transitions
        .asks()
        .map_err(AgentAnswerError::Ask)?
        .into_iter()
        .find(|a| a.transition == transition && a.subject == subject)
        .ok_or(AgentAnswerError::Ask(UftError::NoLongerApplies))?;
    if ask.irreversible {
        return Err(AgentAnswerError::Ask(UftError::NeedsApproval {
            transition,
            subject,
        }));
    }
    let value = match (&ask.input, params.get(PARAM_VALUE)) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(AgentAnswerError::Param(format!(
                "'{}' takes no value",
                transition.0
            )));
        }
        (Some(_), None) => {
            return Err(AgentAnswerError::Param(format!(
                "'{}' needs a '{PARAM_VALUE}'",
                transition.0
            )));
        }
        (Some(hint), Some(raw)) => Some(
            parse_input(hint, raw, answering.input.as_ref()).map_err(AgentAnswerError::Input)?,
        ),
    };
    answering
        .transitions
        .answer(AnswerCall {
            transition,
            subject,
            seen,
            answer: new_answer_id(),
            value,
        })
        .map_err(AgentAnswerError::Ask)
}

fn new_answer_id() -> AnswerId {
    // ALLOW(identity_minting): names one answer occurrence, not an entity.
    AnswerId(uuid::Uuid::new_v4().to_string())
}
