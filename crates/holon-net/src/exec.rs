//! The executor of runtime nets at Holon's token value [`Value`]: the
//! generic executor of `petri-workflow` behind type aliases, the host
//! functions, and the Holon ask surface.

use holon_pattern::Value;
use holon_transition_contract::AskHash;
pub use petri_workflow::exec::*;

pub use crate::host::HOST;
pub use crate::host::encode_value;
pub use crate::host::new_net;
pub use crate::uft::answer::AskMeta;
pub use crate::uft::answer::AskMetas;
pub use crate::uft::answer::Asks;
pub use crate::uft::answer::HASH_FIELD;
pub use crate::uft::answer::answer_transition;
pub use crate::uft::answer::answered_place;
pub use crate::uft::answer::ask_places;
pub use crate::uft::answer::asks_place;
pub use crate::uft::answer::expired_place;

pub type Content = Record<Value>;
pub type Read = petri_workflow::exec::Read<Value>;
pub type PureBody = petri_workflow::exec::PureBody<Value>;
pub type PlaceKind = petri_workflow::exec::PlaceKind<Value>;
pub type PlaceDef = petri_workflow::exec::PlaceDef<Value>;
pub type Output = petri_workflow::exec::Output<Value>;
pub type Body = petri_workflow::exec::Body<Value>;
pub type TransitionDef = petri_workflow::exec::TransitionDef<Value>;
pub type NetDef = petri_workflow::exec::NetDef<Value>;
pub type Token = petri_workflow::exec::Token<Value>;
pub type Payload = petri_workflow::exec::Payload<Value>;
pub type Answered = petri_workflow::exec::Answered<Value>;
pub type AnswerProgress<'a> = petri_workflow::exec::AnswerProgress<'a, Value>;
pub type Candidate = petri_workflow::exec::Candidate<Value>;
pub type Tokens = petri_workflow::exec::Tokens<Value>;
pub type Trace = petri_workflow::exec::Trace<Value>;
pub type Reader<'a> = petri_workflow::exec::Reader<'a, Value>;
pub type EffectRequest = petri_workflow::exec::EffectRequest<Value>;
pub type EffectResult = petri_workflow::exec::EffectResult<Value>;
pub type Resolution = petri_workflow::exec::Resolution<Value>;
pub type Quiesced<T> = petri_workflow::exec::Quiesced<T, Value>;
pub type Executor = petri_workflow::exec::Executor<Value>;

/// The ask identity of `transition` for `subject` over `trace`, with
/// Holon's value encoder.
pub fn ask_hash(transition: &str, subject: &Subject, trace: &Trace) -> AskHash {
    ask_hash_with(HOST.encode, transition, subject, trace)
}
