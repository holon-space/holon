//! How an answer and its expiry look as net data: the keys of the `Answered.T`
//! and `Expired.T` tokens. The executor holds an answer typed and asks this
//! module for every key.

use holon_pattern::Value;
use holon_transition_contract::ANSWER_ID_KEY;
use holon_transition_contract::ANSWER_INPUT_KEY;
use holon_transition_contract::ExpireReason;

use crate::exec::Answered;
use crate::exec::Content;

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

/// The answer id an `Answer.T` input names.
pub fn answer_id(value: &Content) -> Option<&str> {
    match value.get(ANSWER_ID_KEY) {
        Some(Value::String(id)) => Some(id),
        _ => None,
    }
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

/// A field of an `Answered.T` token as a reader sees it: the value only when
/// `show_input`.
pub fn answered_field(a: &Answered, field: &str, show_input: bool) -> Option<Value> {
    match field {
        ANSWER_ID_KEY => Some(Value::String(a.id.to_string())),
        ANSWER_INPUT_KEY => show_input.then(|| a.input.clone()),
        _ => a.record.get(field).cloned(),
    }
}

/// The answer id as the read-trace field that keys a work's failure mark.
pub fn traced_id(a: &Answered) -> (String, Value) {
    (ANSWER_ID_KEY.to_string(), Value::String(a.id.to_string()))
}

/// Adds the answer value to the bound input of the work; `false` when the
/// bind already produced that field.
pub fn work_input(input: &mut Content, a: &Answered) -> bool {
    if input.contains_key(ANSWER_INPUT_KEY) {
        return false;
    }
    input.insert(ANSWER_INPUT_KEY.to_string(), a.input.clone());
    true
}

/// The reason of an `Expired.T` token, or the field it lacks.
pub fn parse_expired(content: &Content) -> Result<ExpireReason, &'static str> {
    match content.get(REASON_FIELD) {
        Some(Value::String(r)) => parse_expire_reason(r).ok_or(REASON_FIELD),
        _ => Err(REASON_FIELD),
    }
}
