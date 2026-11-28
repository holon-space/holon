//! How an ask and its answer look in a Holon net: the names of an ask's
//! places and transitions, the keys of the `Answered.T` and `Expired.T`
//! tokens, and the ask surface of the UI and MCP.

use std::collections::BTreeMap;
use std::time::Duration;

use holon_api::TypeHint;
use holon_pattern::Value;
use holon_transition_contract::ANSWER_ID_KEY;
use holon_transition_contract::ANSWER_INPUT_KEY;
use holon_transition_contract::AnswerCall;
use holon_transition_contract::AnswerId;
use holon_transition_contract::AnswerSettlement;
use holon_transition_contract::AskRow;
use holon_transition_contract::ExpireReason;
use holon_transition_contract::SubjectName;
use holon_transition_contract::TransitionName;
use holon_transition_contract::UftError;

use crate::exec::AnswerCodec;
use crate::exec::AnswerKeys;
use crate::exec::AnswerProgress;
use crate::exec::Answered;
use crate::exec::Ask;
use crate::exec::ClockOutOfRange;
use crate::exec::ClockWentBack;
use crate::exec::Content;
use crate::exec::DeriveError;
use crate::exec::Executor;
use crate::exec::FireError;
use crate::exec::Place;
use crate::exec::Quiesced;
use crate::exec::Settled;
use crate::exec::WhyNot;
use crate::exec::hash_from_text;
use crate::exec::hash_text;

/// The places and transitions of an ask for the work transition `T`
/// (UFT-adopt.a): the derived `Asks.T`, the manual `Answer.T` that puts the
/// `Answered.T` token, and the `Expired.T` token of an answer that no longer
/// applies, whose subject is the answer id.
pub fn asks_place(work: &str) -> String {
    format!("Asks.{work}")
}

pub fn answered_place(work: &str) -> String {
    format!("Answered.{work}")
}

pub fn answer_transition(work: &str) -> String {
    format!("Answer.{work}")
}

pub fn expired_place(work: &str) -> String {
    format!("Expired.{work}")
}

/// The ask declaration of `work` under these names.
pub fn ask_places(work: &str) -> Ask {
    Ask {
        offer: Place::new(&asks_place(work)),
        answer: answer_transition(work).into(),
        answered: Place::new(&answered_place(work)),
        expired: Place::new(&expired_place(work)),
    }
}

/// `Asks.T`: the [`hash_text`] of the ask.
pub const HASH_FIELD: &str = "hash";

/// What the `Asks` row of an ask shows besides its hash.
#[derive(Clone, Debug, PartialEq)]
pub struct AskMeta {
    pub input: Option<TypeHint>,
    pub irreversible: bool,
    pub has_preview: bool,
}

/// The [`AskMeta`] of each ask of a net, keyed by its work transition.
pub type AskMetas = BTreeMap<String, AskMeta>;

pub const CODEC: AnswerCodec<Value> = AnswerCodec {
    keys: AnswerKeys {
        id: ANSWER_ID_KEY,
        input: ANSWER_INPUT_KEY,
    },
    parse: parse_answered,
    check_expired,
    expired_cause,
};

fn check_expired(content: &Content) -> Result<(), &'static str> {
    parse_expired(content).map(|_| ())
}

fn expired_cause() -> String {
    UftError::NoLongerApplies.to_string()
}

/// `Answered.T`: the [`hash_text`](crate::exec::hash_text) of the ask the user
/// saw. The token also holds [`ANSWER_ID_KEY`], which keys the failure mark of
/// `T` so a new answer is a new mark key, and [`ANSWER_INPUT_KEY`], which no
/// reader sees: the executor adds it to the bound input of `T`.
pub const SEEN_FIELD: &str = "seen";
/// `Expired.T`: the [`expire_reason_text`].
pub const REASON_FIELD: &str = "reason";

pub fn expire_reason_text(reason: ExpireReason) -> &'static str {
    match reason {
        ExpireReason::AskGone => "ask_gone",
        ExpireReason::SituationChanged => "situation_changed",
    }
}

fn parse_expire_reason(text: &str) -> Option<ExpireReason> {
    [ExpireReason::AskGone, ExpireReason::SituationChanged]
        .into_iter()
        .find(|r| expire_reason_text(*r) == text)
}

/// The `Answer.T` input of a user answer.
pub fn answer_value(id: &str, seen: String, value: Option<Value>) -> Content {
    Content::from([
        (ANSWER_ID_KEY.to_string(), Value::String(id.to_string())),
        (SEEN_FIELD.to_string(), Value::String(seen)),
        (ANSWER_INPUT_KEY.to_string(), value.unwrap_or(Value::Null)),
    ])
}

/// An `Answered.T` token from net data, or the field it lacks: the work's
/// firing reads the answer id and the value, the lowering the seen hash.
pub fn parse_answered(mut content: Content) -> Result<Answered, &'static str> {
    let id = match content.remove(ANSWER_ID_KEY) {
        Some(Value::String(id)) => id,
        _ => return Err(ANSWER_ID_KEY),
    };
    if !matches!(content.get(SEEN_FIELD), Some(Value::String(_))) {
        return Err(SEEN_FIELD);
    }
    let input = content.remove(ANSWER_INPUT_KEY).ok_or(ANSWER_INPUT_KEY)?;
    Ok(Answered {
        id: id.into(),
        input,
        record: content,
    })
}

/// The net data of an `Answered.T` token.
pub fn answered_content(a: &Answered) -> Content {
    let mut content = a.record.clone();
    content.insert(ANSWER_ID_KEY.to_string(), Value::String(a.id.to_string()));
    content.insert(ANSWER_INPUT_KEY.to_string(), a.input.clone());
    content
}

/// The reason of an `Expired.T` token, or the field it lacks.
pub fn parse_expired(content: &Content) -> Result<ExpireReason, &'static str> {
    match content.get(REASON_FIELD) {
        Some(Value::String(r)) => parse_expire_reason(r).ok_or(REASON_FIELD),
        _ => Err(REASON_FIELD),
    }
}

/// The ask surface of the UI and MCP (UFT-adopt.a) over an executor of a
/// Holon net.
pub trait Asks {
    /// Fires `Answer.T` with the user value; the answer id is the
    /// occurrence. Three panics stay. The first is the `unreachable!` arm; the
    /// load rules out each of its refusals: `NotManual` by `NetDef::check`
    /// (`Answer.T` exists and is manual), `InFlight` and `RetryPending`
    /// because `Answer.T` is pure without retry, `AnswerOccurrence` because
    /// the occurrence is the answer id, `FailedOnThisInput` because `Answer.T`
    /// is not auto, and `BindsAnswerInput` because no ask's work is an
    /// `Answer.T` (`LoadError::AskWorkIsAnswer`). The other two are a firing
    /// that puts, with no compare-and-set, a different token into an occupied
    /// single-token place (`refuse_blind_overwrite`), and a value that holds
    /// `Value::Removed`, which the token hash refuses (`encode_value`). A bind
    /// of `Answer.T` that returns a field the answer sets is a typed
    /// [`UftError::AnswerBindCollides`].
    fn answer(&mut self, call: AnswerCall, now: Duration) -> Result<Quiesced<AnswerId>, UftError>;

    fn answer_settlement(&self, answer: &AnswerId) -> Result<AnswerSettlement, UftError>;

    /// The rows of every `Asks.T`, in work and subject order; `metas` holds
    /// the meta of every ask of the net.
    fn asks(&self, metas: &AskMetas) -> Result<Vec<AskRow>, DeriveError>;
}

impl Asks for Executor {
    fn answer(&mut self, call: AnswerCall, now: Duration) -> Result<Quiesced<AnswerId>, UftError> {
        let work = call.transition.0.as_str();
        let id = call.answer.0.as_str();
        let value = answer_value(id, hash_text(&call.seen), call.value);
        match self.answer_ask(work, &call.subject.0, id, value, now) {
            Ok(fired) => Ok(Quiesced {
                value: call.answer,
                requests: fired.requests,
            }),
            Err(FireError::ClockWentBack(ClockWentBack { clock, now })) => {
                Err(UftError::ClockWentBack { clock, now })
            }
            Err(FireError::ClockOutOfRange(ClockOutOfRange { now, last })) => {
                Err(UftError::ClockOutOfRange { now, last })
            }
            Err(FireError::UnknownTransition(_)) => {
                Err(UftError::UnknownTransition(call.transition))
            }
            Err(
                FireError::DuplicateOccurrence(_)
                | FireError::NotEnabled {
                    why: WhyNot::Inhibited(_),
                    ..
                },
            ) => Err(UftError::AlreadyAnswered),
            Err(FireError::NotEnabled {
                why: WhyNot::Guard | WhyNot::Needs { .. },
                ..
            }) => Err(UftError::NoLongerApplies),
            Err(FireError::NoQuiescence(steps)) => Err(UftError::NoQuiescence {
                transition: call.transition,
                subject: call.subject,
                steps,
            }),
            Err(FireError::NotEnabled {
                why: WhyNot::ReadsUndeclared(place),
                ..
            }) => Err(UftError::ReadsUndeclared {
                transition: call.transition,
                subject: call.subject,
                place: place.to_string(),
            }),
            Err(FireError::NotEnabled {
                why: WhyNot::DerivedCycle(cycle),
                ..
            }) => Err(UftError::DerivedCycle {
                transition: call.transition,
                subject: call.subject,
                cycle: cycle.iter().map(Place::to_string).collect(),
            }),
            Err(FireError::ValueCollides { field, .. }) => Err(UftError::AnswerBindCollides {
                transition: call.transition,
                subject: call.subject,
                field,
            }),
            Err(
                e @ (FireError::NotManual(_)
                | FireError::AnswerOccurrence { .. }
                | FireError::NotEnabled {
                    why:
                        WhyNot::InFlight
                        | WhyNot::RetryPending { .. }
                        | WhyNot::FailedOnThisInput(_)
                        | WhyNot::BindsAnswerInput,
                    ..
                }),
            ) => unreachable!(
                "the load checks that '{work}' has a manual pure Answer without retry: {e}"
            ),
        }
    }

    fn answer_settlement(&self, answer: &AnswerId) -> Result<AnswerSettlement, UftError> {
        let id = answer.0.as_str();
        let progress = self
            .answer_progress(id)
            .ok_or_else(|| UftError::UnknownAnswer(answer.clone()))?;
        Ok(match progress {
            AnswerProgress::Undone => AnswerSettlement::Undone,
            AnswerProgress::Work {
                occurrence,
                settled: None | Some(Settled::OutcomeUnknown(_)),
            } => AnswerSettlement::Working { work: occurrence },
            AnswerProgress::Work {
                occurrence,
                settled: Some(Settled::Done),
            } => AnswerSettlement::Done { work: occurrence },
            AnswerProgress::Work {
                occurrence,
                settled: Some(Settled::Failed(cause) | Settled::OverChangedInputs { cause, .. }),
            } => AnswerSettlement::Failed {
                cause: cause.text,
                work: occurrence,
            },
            AnswerProgress::Expired { place, record } => AnswerSettlement::Expired {
                reason: parse_expired(record)
                    .unwrap_or_else(|field| panic!("the {place} token of '{id}' lacks '{field}'")),
            },
            AnswerProgress::Waiting => AnswerSettlement::Waiting,
        })
    }

    fn asks(&self, metas: &AskMetas) -> Result<Vec<AskRow>, DeriveError> {
        let mut rows = Vec::new();
        for (work, ask) in self.declared_asks() {
            let meta = metas
                .get(&**work)
                .unwrap_or_else(|| panic!("no ask meta for the work '{work}'"));
            for (subject, row) in self.derive_all(&ask.offer)? {
                let hash = match row.get(HASH_FIELD) {
                    Some(Value::String(text)) => hash_from_text(text).map_err(|e| e.to_string()),
                    other => Err(format!("{other:?} is no text")),
                }
                .map_err(|reason| DeriveError::AskRowHash {
                    place: ask.offer.clone(),
                    subject: subject.to_string(),
                    reason,
                })?;
                rows.push(AskRow {
                    transition: TransitionName(work.to_string()),
                    subject: SubjectName(subject.to_string()),
                    hash,
                    input: meta.input.clone(),
                    irreversible: meta.irreversible,
                    has_preview: meta.has_preview,
                });
            }
        }
        Ok(rows)
    }
}
