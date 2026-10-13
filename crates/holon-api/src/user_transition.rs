//! The contract of user-facing transitions (#167, increment 1).
//!
//! Declarations — the `#[transition]` macro and YAML sidecars — emit one
//! high-level IR, [`TransitionDescriptor`]. It states WHAT a transition asks;
//! a lowering pass in `holon-net` generates the plumbing (`Asks`, `Answer`,
//! `ExpireAnswer`). People and agents see the same [`AskRow`]s, fire the same
//! [`AnswerCall`] and read the same [`AnswerSettlement`] through
//! [`UserTransitions`].
//!
//! The types live here because every consumer (macro, lowering, executor,
//! YAML loader, UI, MCP) already depends on `holon-api`, and the frontends do
//! not depend on `holon-net`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::EntityUri;
use crate::Value;
use crate::render_types::OperationDescriptor;
use crate::render_types::TypeHint;
use crate::types::EntityName;

/// The name of a transition, unique in its net.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransitionName(pub String);

/// The name of a place, unique in its net.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlaceName(pub String);

/// The subject a token, an ask or an answer is about (`a.org`, `b.org#4`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubjectName(pub String);

/// A placeholder for an expression in the #166 IR. Opaque until #166 lands;
/// the YAML loader (lane Y) fills it from sidecar text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExprSlot(String);

impl ExprSlot {
    pub fn new(source: impl Into<String>) -> Self {
        Self(source.into())
    }

    pub fn source(&self) -> &str {
        &self.0
    }
}

/// A Rust function the macro (lane M) registers under this symbol: a work
/// body or a pure `preview` function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeRef(pub String);

/// The high-level transition IR (HL-IR). Filled by the macro (lane M) and the
/// YAML loader (lane Y); read by the lowering pass (lane L).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransitionDescriptor {
    pub name: TransitionName,
    pub arcs: Vec<HlArc>,
    pub body: BodyDecl,
    /// A fact about the effect: the UI confirms it, agents need approval.
    pub irreversible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timers: Vec<TimerDecl>,
    /// A guard over the marking. Never over the answer value (UFT-adopt.a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<ExprSlot>,
    /// `Some` makes the transition `manual`: it waits for an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<AskDecl>,
}

impl TransitionDescriptor {
    pub fn manual(&self) -> bool {
        self.ask.is_some()
    }
}

/// One arc of a [`TransitionDescriptor`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlArc {
    pub kind: ArcKind,
    pub place: PlaceName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcKind {
    Take,
    Read,
    Inhibit,
    Put,
    /// Gets a token when a run settles failed.
    OnFailure,
    /// Gets a token when the run timer ends an unsettled run.
    OnTimeout,
}

/// What the transition runs when it fires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyDecl {
    Native {
        symbol: NativeRef,
        effectful: bool,
    },
    Expr(ExprSlot),
    /// A call of a sidecar tool, arguments by name.
    Tool {
        tool: String,
        args: BTreeMap<String, ToolArg>,
    },
}

/// The value a [`BodyDecl::Tool`] passes for one argument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArg {
    /// The answer value of the ask; the lowered `Content` carries it under
    /// [`ANSWER_INPUT_KEY`].
    Answer,
    Literal(Value),
    /// A field of the token taken from `place`. No declaration binds one
    /// until #166.
    TokenField {
        place: PlaceName,
        field: String,
    },
}

/// What a `manual` transition asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskDecl {
    /// The answer's type; `None` is a click with no value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<TypeHint>,
    /// A pure function computed lazily when the ask is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<NativeRef>,
    /// The ask is a data-class token lowered to a decision block (later).
    pub persist: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerDecl {
    pub kind: TimerKind,
    pub after: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerKind {
    Retry {
        max_attempts: u32,
    },
    Run,
    Abandon,
    /// Fires the transition once it has been enabled this long (a default
    /// answer, E6).
    Delay,
}

/// The identity of one ask: a hash over the transition, the subject and the
/// read-trace content at derivation (UFT-adopt.a). The executor (lane X)
/// computes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AskHash(pub [u8; 32]);

/// One row of the derived place `Asks`. Derived by the executor (lane X).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskRow {
    pub transition: TransitionName,
    pub subject: SubjectName,
    pub hash: AskHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<TypeHint>,
    pub irreversible: bool,
    pub has_preview: bool,
}

/// Names one answer: the occurrence of its `Answer` firing, chosen by the
/// caller.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnswerId(pub String);

/// The key under which a lowered work transition's `Content` carries the
/// answer value the user gave.
pub const ANSWER_INPUT_KEY: &str = "@input";

/// The key under which a lowered work transition's `Content` carries the
/// [`AnswerId`]; the work occurrence derives from it
/// (`exec::work_occurrence(transition, answer)`).
pub const ANSWER_ID_KEY: &str = "@answer";

/// Names one occurrence of a firing in a runtime net. The variants never
/// collide, so neither do their displays.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceId {
    /// Supplied by the caller of a manual fire; an answer's id.
    Caller(Arc<str>),
    /// Minted by the executor for an auto firing.
    Minted { transition: Arc<str>, n: u64 },
    /// The work firing that consumed the answer `answer`: derived from the
    /// answer id, so the settled chain and a Keyed receiver find it.
    Work {
        transition: Arc<str>,
        answer: Arc<str>,
    },
}

impl OccurrenceId {
    pub fn caller(id: &str) -> Self {
        Self::Caller(Arc::from(id))
    }
}

impl fmt::Display for OccurrenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Caller(id) => write!(f, "caller:{id}"),
            Self::Minted { transition, n } => write!(f, "minted:{transition}#{n}"),
            Self::Work { transition, answer } => write!(f, "work:{transition}@{answer}"),
        }
    }
}

/// The one user-fired call. Built by the UI/MCP (lane U) from a parsed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerCall {
    pub transition: TransitionName,
    pub subject: SubjectName,
    /// The hash of the ask the user saw; a mismatch at work time expires the
    /// answer with [`ExpireReason::SituationChanged`].
    pub seen: AskHash,
    pub answer: AnswerId,
    /// The value parsed by [`parse_input`]; `None` for an ask without input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// The settled chain: answer → work occurrence → outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSettlement {
    /// The `Answered` token waits for its work transition.
    Waiting,
    /// The work runs, or its outcome is unknown and waits for a resolution.
    Working {
        work: OccurrenceId,
    },
    Done {
        work: OccurrenceId,
    },
    /// The work failed, or its effect happened over changed inputs.
    Failed {
        work: OccurrenceId,
        cause: String,
    },
    /// `ExpireAnswer` consumed the answer (`Failed(NoLongerApplies)`).
    Expired {
        reason: ExpireReason,
    },
    /// The user undid the answer.
    Undone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpireReason {
    /// The ask vanished before the work fired.
    AskGone,
    /// The ask hash changed since the user saw it.
    SituationChanged,
}

/// What the offer path says about one operation for one subject at one
/// marking.
///
/// To HIDE an operation, the producer emits no [`Offered`] entry for that
/// subject: the transition does not apply there. [`Offer::Refused`] means it
/// applies but is blocked now, and the UI shows it disabled with the reason;
/// D151.a's slash-menu vanish is a surface policy over `Refused`. There is no
/// "absent" variant.
///
/// Three-valued where `NetGuard`'s verdict is binary: the dispatcher remains
/// the authority, so an operation the marking cannot classify is offered and
/// decided on dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Offer {
    Enabled,
    Refused { reason: String },
    Unknown { why: String },
}

impl Offer {
    /// Whether the operation can be fired now.
    pub fn is_offered(&self) -> bool {
        !matches!(self, Offer::Refused { .. })
    }
}

/// One catalog operation evaluated against one subject. The offer is not
/// catalog data, so it lives here and not on [`OperationDescriptor`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offered {
    pub descriptor: OperationDescriptor,
    pub offer: Offer,
}

/// The lane that fills a stub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Lowering,
    Executor,
    Integration,
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Lane::Lowering => "L (lowering)",
            Lane::Executor => "X (executor)",
            Lane::Integration => "I (integration)",
        })
    }
}

/// What a contract stub returns until its lane fills it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{what} is not implemented yet; lane {lane} fills it (#167)")]
pub struct NotImplemented {
    pub lane: Lane,
    pub what: &'static str,
}

/// Why an ask or answer call did not go through.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UftError {
    #[error("the question no longer applies")]
    NoLongerApplies,
    #[error("already answered")]
    AlreadyAnswered,
    #[error("no answer '{}'", .0.0)]
    UnknownAnswer(AnswerId),
    #[error("no user-facing transition '{}'", .0.0)]
    UnknownTransition(TransitionName),
    /// The answer applied, but the automatic transitions it caused were
    /// still enabled after `steps` rounds; the net discloses a condition.
    #[error(
        "the answer of '{}' for '{}' did not quiesce after {steps} rounds",
        transition.0,
        subject.0
    )]
    NoQuiescence {
        transition: TransitionName,
        subject: SubjectName,
        steps: usize,
    },
    /// An agent answered an `irreversible` ask; a person approves it in the UI.
    #[error("'{}' on '{}' is irreversible; an agent's answer needs approval in the UI", .transition.0, .subject.0)]
    NeedsApproval {
        transition: TransitionName,
        subject: SubjectName,
    },
    #[error(transparent)]
    NotImplemented(#[from] NotImplemented),
}

/// The one port through which the UI and MCP (lane U) list asks, answer them
/// and await the outcome. Implemented by the integration (lane I).
pub trait UserTransitions: Send + Sync {
    fn asks(&self) -> Result<Vec<AskRow>, UftError>;
    fn preview(&self, ask: &AskRow) -> Result<String, UftError>;
    fn answer(&self, call: AnswerCall) -> Result<AnswerId, UftError>;
    fn settlement(&self, answer: &AnswerId) -> Result<AnswerSettlement, UftError>;
}

/// The [`UserTransitions`] stub until lane I wires the executor.
pub struct Unwired;

impl UserTransitions for Unwired {
    fn asks(&self) -> Result<Vec<AskRow>, UftError> {
        Err(unwired("UserTransitions::asks").into())
    }

    fn preview(&self, _: &AskRow) -> Result<String, UftError> {
        Err(unwired("UserTransitions::preview").into())
    }

    fn answer(&self, _: AnswerCall) -> Result<AnswerId, UftError> {
        Err(unwired("UserTransitions::answer").into())
    }

    fn settlement(&self, _: &AnswerId) -> Result<AnswerSettlement, UftError> {
        Err(unwired("UserTransitions::settlement").into())
    }
}

fn unwired(what: &'static str) -> NotImplemented {
    NotImplemented {
        lane: Lane::Integration,
        what,
    }
}

/// A form or MCP input the boundary refused. Names the type and the input.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InputParseError {
    #[error("'{input}' is not a {type_name}: {reason}")]
    Refused {
        type_name: String,
        input: String,
        reason: String,
    },
    #[error(transparent)]
    NotImplemented(#[from] NotImplemented),
}

/// What [`parse_input`] needs from outside the type: newtype parsers (lane M
/// registers them), place membership and id existence (lane I).
pub trait InputContext {
    /// Parses the text of the newtype `type_name`; `Err` is the refusal reason.
    fn parse_newtype(&self, type_name: &str, text: &str) -> Result<Value, String>;
    fn place_holds(&self, place: &str, subject: &str) -> Result<bool, NotImplemented>;
    fn entity_exists(&self, entity: &EntityName, id: &str) -> Result<bool, NotImplemented>;
    /// Whether the asking provider's own table has the row key `key`.
    fn row_key_exists(&self, key: &str) -> Result<bool, NotImplemented>;
}

/// Parses a raw answer value against the ask's input type at the UI/MCP
/// boundary (parse, don't validate).
pub fn parse_input(
    hint: &TypeHint,
    raw: &Value,
    cx: &dyn InputContext,
) -> Result<Value, InputParseError> {
    let refused = |type_name: &str, reason: &str| InputParseError::Refused {
        type_name: type_name.to_owned(),
        input: render(raw),
        reason: reason.to_owned(),
    };
    let unanswerable =
        |type_name: &str| refused(type_name, "an ask cannot take this type as its answer");
    match (hint, raw) {
        (
            TypeHint::Enum {
                type_name,
                variants,
            },
            Value::String(label),
        ) => {
            if variants.contains(label) {
                Ok(raw.clone())
            } else {
                Err(refused(
                    type_name,
                    &format!("expected one of {}", variants.join(", ")),
                ))
            }
        }
        (TypeHint::Newtype { type_name }, Value::String(text)) => cx
            .parse_newtype(type_name, text)
            .map_err(|reason| refused(type_name, &reason)),
        (TypeHint::PlaceId { place }, Value::String(id)) => {
            if cx.place_holds(&place.0, id)? {
                Ok(raw.clone())
            } else {
                Err(refused(
                    &format!("id of {}", place.0),
                    "no such token in the place",
                ))
            }
        }
        (TypeHint::Enum { type_name, .. } | TypeHint::Newtype { type_name }, _) => {
            Err(refused(type_name, "expected text"))
        }
        (TypeHint::PlaceId { place }, _) => {
            Err(refused(&format!("id of {}", place.0), "expected text"))
        }
        (TypeHint::Number, Value::String(text)) => text
            .trim()
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| refused("number", "expected an integer")),
        (TypeHint::EntityIdOrRoot { .. }, Value::String(id))
            if id == EntityUri::no_parent().as_str() =>
        {
            Ok(raw.clone())
        }
        (
            TypeHint::EntityId { entity_name } | TypeHint::EntityIdOrRoot { entity_name },
            Value::String(id),
        ) => {
            if cx.entity_exists(entity_name, id)? {
                Ok(raw.clone())
            } else {
                Err(refused(
                    &format!("id of {}", entity_name.as_str()),
                    "no such id",
                ))
            }
        }
        (TypeHint::RowKey, Value::String(key)) => {
            if cx.row_key_exists(key)? {
                Ok(raw.clone())
            } else {
                Err(refused("row key", "no such row"))
            }
        }
        (TypeHint::Bool, Value::Boolean(_))
        | (TypeHint::String, Value::String(_))
        | (TypeHint::Number, Value::Integer(_)) => Ok(raw.clone()),
        (TypeHint::Bool, _) => Err(refused("bool", "expected true or false")),
        (TypeHint::String, _) => Err(refused("string", "expected text")),
        (TypeHint::Number, _) => Err(refused("number", "expected an integer")),
        (TypeHint::EntityId { entity_name } | TypeHint::EntityIdOrRoot { entity_name }, _) => Err(
            refused(&format!("id of {}", entity_name.as_str()), "expected text"),
        ),
        (TypeHint::RowKey, _) => Err(refused("row key", "expected text")),
        (TypeHint::FieldValue { .. }, _) => Err(unanswerable("field value")),
        (TypeHint::Object { .. }, _) => Err(unanswerable("object")),
        (TypeHint::Expr, _) => Err(unanswerable("expression")),
        (TypeHint::Collection, _) => Err(unanswerable("collection")),
    }
}

fn render(raw: &Value) -> String {
    match raw {
        Value::String(s) => s.clone(),
        other => format!("{other:?}"),
    }
}
