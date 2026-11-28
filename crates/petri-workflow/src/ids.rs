//! The ids that leave the executor: occurrences, answers and asks.

use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

/// The identity of one ask: a hash over the transition, the subject and the
/// read-trace content at derivation. The executor computes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AskHash(pub [u8; 32]);

/// Names one answer: the occurrence of its `Answer` firing, chosen by the
/// caller.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnswerId(pub String);

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
