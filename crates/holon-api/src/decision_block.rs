//! The block home of a decision: a block tagged `decision` whose task
//! keyword is its status (B4) and whose children are options, answers or
//! discussion (B6). The decision block's title is the question, its bare id
//! the label. Option and answer titles are display only; option and answer
//! order is tree order.
//!
//! This module reads blocks into a [`DecisionDraft`]. The operations that
//! store a decision live in `holon_org_format::decision_block`, next to the
//! org parser that decides which text org gives back as written.

use crate::Value;
use crate::block::Block;
use crate::decision::AnswerInput;
use crate::decision::Decision;
use crate::decision::DecisionDraft;
use crate::decision::DecisionError;
use crate::decision::DecisionRef;
use crate::decision::DraftQuestion;
use crate::decision::RawAnswer;
use crate::decision::RawRuling;
use crate::decision::RawWithdrawal;
use crate::decision::RefusedInput;
use crate::decision::Status;
use crate::entity_uri::EntityUri;

pub const DECISION_TAG: &str = "decision";

pub const TASK_STATE: &str = "task_state";

pub const CHOOSE: &str = "choose";
pub const RECOMMEND: &str = "recommend";
pub const SUPERSEDES: &str = "supersedes";
pub const CHOSEN: &str = "chosen";
pub const DECIDER: &str = "decider";
pub const DECIDED: &str = "decided";
pub const NOTE: &str = "note";
pub const WITHDRAWER: &str = "withdrawer";
pub const WITHDRAWN: &str = "withdrawn";
const RULING_KEYS: &[&str] = &[CHOSEN, DECIDER, DECIDED, NOTE];
const WITHDRAWAL_KEYS: &[&str] = &[WITHDRAWER, WITHDRAWN];

pub const OPTION: &str = "option";
pub const ANSWERER: &str = "answerer";
pub const ANSWERED: &str = "answered";
pub const PICK: &str = "pick";
/// A categorical distribution: `a=0.8 b=0.1`.
pub const CATEGORICAL: &str = "p";
/// Independent per-option probabilities, written like [`CATEGORICAL`].
pub const MARGINAL: &str = "marginal";
const ANSWER_BODY_KEYS: &[&str] = &[PICK, CATEGORICAL, MARGINAL];
const ANSWER_KEYS: &[&str] = &[ANSWERED, PICK, CATEGORICAL, MARGINAL];

/// A key set with no members. An empty property value is not stored at all.
pub const EMPTY_SET: &str = "()";

pub const ANSWER_TITLE: &str = "Answer";
/// Held in the answer block's body, below its title.
pub const RATIONALE: &str = "rationale";

/// B4: the task keyword of a decision block is its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Open,
    Decided,
    Withdrawn,
}

impl Keyword {
    fn of(block: &Block) -> Result<Self, BlockDecisionError> {
        let keyword = text(block, TASK_STATE)?;
        match keyword {
            Some("?") => Ok(Keyword::Open),
            Some("DONE") => Ok(Keyword::Decided),
            Some("CANCELLED") => Ok(Keyword::Withdrawn),
            other => Err(BlockDecisionError::UnknownKeyword {
                block: block.id.clone(),
                keyword: other.map(str::to_string),
            }),
        }
    }

    pub fn of_status(status: &Status) -> Self {
        match status {
            Status::Open => Keyword::Open,
            Status::Decided(_) => Keyword::Decided,
            Status::Withdrawn { .. } => Keyword::Withdrawn,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Open => "?",
            Keyword::Decided => "DONE",
            Keyword::Withdrawn => "CANCELLED",
        }
    }

    /// Keys a decision block in this state must not carry.
    fn foreign_keys(self) -> impl Iterator<Item = &'static str> {
        let (ruling, withdrawal): (&[&str], &[&str]) = match self {
            Keyword::Open => (RULING_KEYS, WITHDRAWAL_KEYS),
            Keyword::Decided => (&[], WITHDRAWAL_KEYS),
            Keyword::Withdrawn => (RULING_KEYS, &[]),
        };
        ruling.iter().chain(withdrawal).copied()
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BlockDecisionError {
    #[error("B4: block {block} is not tagged `decision`")]
    NotTagged { block: EntityUri },
    #[error("B4: decision {block} has keyword {keyword:?}; a decision is `?`, DONE or CANCELLED")]
    UnknownKeyword {
        block: EntityUri,
        keyword: Option<String>,
    },
    #[error("B4: decision {block} is {keyword} but carries `{key}`")]
    StrayKey {
        block: EntityUri,
        keyword: &'static str,
        key: &'static str,
    },
    #[error("B6: child {child} has both `option` and `answerer`")]
    OptionAndAnswer { child: EntityUri },
    #[error("block {block}: `{key}` holds {value}, not text")]
    NotText {
        block: EntityUri,
        key: &'static str,
        value: String,
    },
    #[error("B6: option child {child} carries answer keys {keys:?}")]
    AnswerKeysOnOption {
        child: EntityUri,
        keys: Vec<&'static str>,
    },
    #[error("the block tree cannot hold the {field} {text:?}: {reason}")]
    Unencodable {
        field: &'static str,
        text: String,
        reason: &'static str,
    },
    #[error("decision {0} does not live in the block tree")]
    ForeignHome(DecisionRef),
    #[error(transparent)]
    Core(#[from] DecisionError),
}

impl BlockDecisionError {
    /// The rule this error enforces: a block rule (`B4`, `B6`), a core rule
    /// ([`crate::decision::Rule::code`]), `encoding` or `home`.
    pub fn code(&self) -> &'static str {
        use BlockDecisionError::*;
        match self {
            NotTagged { .. } | UnknownKeyword { .. } | StrayKey { .. } => "B4",
            OptionAndAnswer { .. } | AnswerKeysOnOption { .. } => "B6",
            NotText { .. } => "syntax",
            Unencodable { .. } => "encoding",
            ForeignHome(_) => "home",
            Core(e) => e.rule().code(),
        }
    }
}

/// `children` are the decision block's direct children in tree order.
pub fn read(decision: &Block, children: &[Block]) -> Result<DecisionDraft, BlockDecisionError> {
    if !decision.tags.contains(DECISION_TAG) {
        return Err(BlockDecisionError::NotTagged {
            block: decision.id.clone(),
        });
    }
    let keyword = Keyword::of(decision)?;
    for key in keyword.foreign_keys() {
        if decision.properties.contains_key(key) {
            return Err(BlockDecisionError::StrayKey {
                block: decision.id.clone(),
                keyword: keyword.as_str(),
                key,
            });
        }
    }
    let prop = |key| Ok::<_, BlockDecisionError>(text(decision, key)?.map(str::to_string));
    let ruling = match keyword {
        Keyword::Decided => Some(RawRuling {
            chosen: prop(CHOSEN)?.as_deref().map(key_list),
            decider: prop(DECIDER)?,
            at: prop(DECIDED)?,
            note: prop(NOTE)?,
        }),
        _ => None,
    };
    let withdrawn = match keyword {
        Keyword::Withdrawn => Some(RawWithdrawal {
            by: prop(WITHDRAWER)?,
            at: prop(WITHDRAWN)?,
        }),
        _ => None,
    };

    let mut options = Vec::new();
    let mut answers = Vec::new();
    let mut refused = Vec::new();
    for child in children {
        assert_eq!(
            child.parent_id, decision.id,
            "{} is no child of decision {}",
            child.id, decision.id
        );
        match (text(child, OPTION)?, text(child, ANSWERER)?) {
            (Some(_), Some(_)) => {
                return Err(BlockDecisionError::OptionAndAnswer {
                    child: child.id.clone(),
                });
            }
            (Some(key), None) => {
                let keys = answer_keys(child);
                if !keys.is_empty() {
                    return Err(BlockDecisionError::AnswerKeysOnOption {
                        child: child.id.clone(),
                        keys,
                    });
                }
                options.push((key.to_string(), child.title()))
            }
            (None, Some(by)) => match read_answer(child, by) {
                Ok(answer) => answers.push(answer),
                Err(reason) => refused.push(RefusedInput {
                    item: child.id.to_string(),
                    reason,
                }),
            },
            (None, None) => {
                let keys = answer_keys(child);
                if !keys.is_empty() {
                    refused.push(RefusedInput {
                        item: child.id.to_string(),
                        reason: format!("carries answer keys {keys:?} but no `{ANSWERER}`"),
                    });
                }
            }
        }
    }

    Ok(DecisionDraft {
        id: DecisionRef::parse(decision.id.as_str())?,
        label: decision.id.id().to_string(),
        question: DraftQuestion {
            question: decision.title(),
            options,
            choose: prop(CHOOSE)?,
            recommend: prop(RECOMMEND)?.as_deref().map(key_list),
            supersedes: prop(SUPERSEDES)?.map(|raw| stored_ref(&raw)).transpose()?,
        },
        ruling,
        withdrawn,
        answers,
        refused,
    })
}

pub fn parse(decision: &Block, children: &[Block]) -> Result<Decision, BlockDecisionError> {
    Ok(Decision::parse(read(decision, children)?)?)
}

fn text<'b>(block: &'b Block, key: &'static str) -> Result<Option<&'b str>, BlockDecisionError> {
    match block.properties.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(other) => Err(BlockDecisionError::NotText {
            block: block.id.clone(),
            key,
            value: format!("{other:?}"),
        }),
    }
}

fn answer_keys(child: &Block) -> Vec<&'static str> {
    ANSWER_KEYS
        .iter()
        .copied()
        .filter(|k| child.properties.contains_key(*k))
        .collect()
}

/// An answer child the core can read, or why it cannot.
fn read_answer(child: &Block, by: &str) -> Result<RawAnswer, String> {
    let text = |key| text(child, key).map_err(|e| e.to_string());
    let at = text(ANSWERED)?.ok_or(format!("an answer needs `{ANSWERED}`"))?;
    let mut bodies = Vec::new();
    for key in ANSWER_BODY_KEYS {
        if let Some(value) = text(key)? {
            bodies.push((*key, value));
        }
    }
    let body = match bodies.as_slice() {
        [(PICK, v)] => AnswerInput::Pick(key_list(v)),
        [(CATEGORICAL, v)] => AnswerInput::Categorical(pairs(v)?),
        [(MARGINAL, v)] => AnswerInput::Marginals(pairs(v)?),
        _ => {
            let found: Vec<&str> = bodies.iter().map(|(k, _)| *k).collect();
            return Err(format!(
                "an answer holds exactly one of {ANSWER_BODY_KEYS:?}; it holds {found:?}"
            ));
        }
    };
    Ok(RawAnswer {
        by: by.to_string(),
        at: at.to_string(),
        body,
        // A blank body is no rationale: org keeps no difference between an
        // empty body and none.
        rationale: child
            .content
            .split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .filter(|rest| !rest.trim().is_empty()),
    })
}

fn key_list(raw: &str) -> Vec<String> {
    if raw == EMPTY_SET {
        return Vec::new();
    }
    raw.split_whitespace().map(str::to_string).collect()
}

fn pairs(raw: &str) -> Result<Vec<(String, f64)>, String> {
    key_list(raw)
        .into_iter()
        .map(|pair| {
            let (key, p) = pair
                .split_once('=')
                .ok_or_else(|| format!("{pair:?} is not <option>=<probability>"))?;
            let p: f64 = p
                .parse()
                .map_err(|e| format!("{pair:?}: {p:?} is no number: {e}"))?;
            Ok((key.to_string(), p))
        })
        .collect()
}

/// A stored reference is a bare block id or a schemed foreign one.
fn stored_ref(raw: &str) -> Result<String, BlockDecisionError> {
    EntityUri::try_from_raw(raw)
        .map(|uri| uri.to_string())
        .map_err(|e| {
            DecisionError::MalformedRef {
                raw: raw.to_string(),
                reason: e.to_string(),
            }
            .into()
        })
}
