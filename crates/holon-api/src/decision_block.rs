//! The block home of a decision: a block tagged `decision` whose task
//! keyword is its status (B4) and whose children are options, answers or
//! discussion (B6). The decision block's title is the question, its bare id
//! the label. Option and answer titles are display only; option and answer
//! order is tree order.
//!
//! Pure translation both ways: blocks → [`DecisionDraft`], and a decision or
//! a [`Change`] → the block `create` / `set_field` operations that store it.

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;

use crate::RemovedTag;
use crate::Value;
use crate::block::Block;
use crate::decision::Answer;
use crate::decision::AnswerBody;
use crate::decision::AnswerInput;
use crate::decision::Change;
use crate::decision::Decision;
use crate::decision::DecisionDraft;
use crate::decision::DecisionError;
use crate::decision::DecisionRef;
use crate::decision::Distribution;
use crate::decision::DraftQuestion;
use crate::decision::Effect;
use crate::decision::OptionKey;
use crate::decision::RawAnswer;
use crate::decision::RawRuling;
use crate::decision::RawWithdrawal;
use crate::decision::RefusedInput;
use crate::decision::Ruling;
use crate::decision::Status;
use crate::entity_uri::EntityUri;
use crate::render_types::Operation;

pub const DECISION_TAG: &str = "decision";

const TASK_STATE: &str = "task_state";

const CHOOSE: &str = "choose";
const RECOMMEND: &str = "recommend";
const SUPERSEDES: &str = "supersedes";
const CHOSEN: &str = "chosen";
const DECIDER: &str = "decider";
const DECIDED: &str = "decided";
const NOTE: &str = "note";
const WITHDRAWER: &str = "withdrawer";
const WITHDRAWN: &str = "withdrawn";
const RULING_KEYS: &[&str] = &[CHOSEN, DECIDER, DECIDED, NOTE];
const WITHDRAWAL_KEYS: &[&str] = &[WITHDRAWER, WITHDRAWN];

const OPTION: &str = "option";
const ANSWERER: &str = "answerer";
const ANSWERED: &str = "answered";
const PICK: &str = "pick";
/// A categorical distribution: `a=0.8 b=0.1`.
const CATEGORICAL: &str = "p";
/// Independent per-option probabilities, written like [`CATEGORICAL`].
const MARGINAL: &str = "marginal";
const ANSWER_BODY_KEYS: &[&str] = &[PICK, CATEGORICAL, MARGINAL];

/// A key set with no members. An empty property value is not stored at all.
const EMPTY_SET: &str = "()";

const ANSWER_TITLE: &str = "Answer";

/// B4: the task keyword of a decision block is its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keyword {
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

    fn of_status(status: &Status) -> Self {
        match status {
            Status::Open => Keyword::Open,
            Status::Decided(_) => Keyword::Decided,
            Status::Withdrawn { .. } => Keyword::Withdrawn,
        }
    }

    fn as_str(self) -> &'static str {
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
    #[error("decision {0} does not live in the block tree")]
    ForeignHome(DecisionRef),
    #[error(transparent)]
    Core(#[from] DecisionError),
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
            (Some(key), None) => options.push((key.to_string(), child.title())),
            (None, Some(by)) => match read_answer(child, by) {
                Ok(answer) => answers.push(answer),
                Err(reason) => refused.push(RefusedInput {
                    item: child.id.to_string(),
                    reason,
                }),
            },
            (None, None) => {}
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

/// The operations that create `decision` under `parent`, after its sibling
/// `after`. `mint` names each option and answer block.
pub fn ask_ops(
    decision: &Decision,
    parent: &EntityUri,
    after: Option<&EntityUri>,
    mut mint: impl FnMut() -> EntityUri,
) -> Result<Vec<Operation>, BlockDecisionError> {
    let id = block_of(decision.id())?;
    assert_eq!(
        decision.label(),
        id.id(),
        "a block decision's label is its bare id"
    );
    assert!(
        decision.refused().is_empty(),
        "refused input of {id} stays where it was read"
    );

    let mut props = vec![(CHOOSE, text_value(decision.choose().to_string()))];
    if let Some(r) = decision.recommend() {
        props.push((RECOMMEND, key_set(r.keys())));
    }
    if let Some(s) = decision.supersedes() {
        props.push((SUPERSEDES, text_value(ref_text(s))));
    }
    match decision.status() {
        Status::Open => {}
        Status::Decided(r) => props.extend(ruling_props(r)),
        Status::Withdrawn { by, at } => props.extend(withdrawal_props(&by.to_string(), at)),
    }
    props.push((
        TASK_STATE,
        text_value(Keyword::of_status(decision.status()).as_str()),
    ));
    props.push(("tags", Value::Array(vec![text_value(DECISION_TAG)])));

    let mut ops = vec![create(&id, parent, after, decision.question(), props)];
    let mut last: Option<EntityUri> = None;
    for option in decision.options().iter() {
        let child = mint();
        ops.push(create(
            &child,
            &id,
            last.as_ref(),
            &option.label,
            vec![(OPTION, text_value(option.key.as_str()))],
        ));
        last = Some(child);
    }
    for answer in decision.answers() {
        let child = mint();
        ops.push(answer_create(&child, &id, last.as_ref(), answer));
        last = Some(child);
    }
    Ok(ops)
}

/// The operations that store `change`. `children` are the decision block's
/// direct children in tree order; `mint` names a new answer block.
pub fn change_ops(
    change: &Change,
    children: &[Block],
    mint: impl FnOnce() -> EntityUri,
) -> Result<Vec<Operation>, BlockDecisionError> {
    let id = block_of(change.decision())?;
    for child in children {
        assert_eq!(
            child.parent_id, id,
            "{} is no child of decision {id}",
            child.id
        );
    }
    let ops = match change.effect() {
        Effect::Answered(answer) => {
            let after = children.last().map(|c| &c.id);
            vec![answer_create(&mint(), &id, after, answer)]
        }
        Effect::Decided { ruling, replaces } => {
            let mut ops = Vec::new();
            if replaces.is_none() {
                ops.push(set_field(
                    &id,
                    TASK_STATE,
                    text_value(Keyword::Decided.as_str()),
                ));
            }
            ops.extend(
                ruling_props(ruling)
                    .into_iter()
                    .map(|(k, v)| set_field(&id, k, v)),
            );
            if ruling.note().is_none() && replaces.as_ref().is_some_and(|r| r.note().is_some()) {
                ops.push(set_field(&id, NOTE, Value::Removed(RemovedTag)));
            }
            ops
        }
        Effect::Withdrawn { by, at } => {
            let mut ops = vec![set_field(
                &id,
                TASK_STATE,
                text_value(Keyword::Withdrawn.as_str()),
            )];
            ops.extend(
                withdrawal_props(&by.to_string(), at)
                    .into_iter()
                    .map(|(k, v)| set_field(&id, k, v)),
            );
            ops
        }
    };
    Ok(ops)
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
        rationale: child
            .content
            .split_once('\n')
            .map(|(_, rest)| rest.to_string()),
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

fn ref_text(r: &DecisionRef) -> String {
    match r.uri().as_block_id() {
        Some(bare) => bare.to_string(),
        None => r.to_string(),
    }
}

fn block_of(r: &DecisionRef) -> Result<EntityUri, BlockDecisionError> {
    if !r.uri().is_block() {
        return Err(BlockDecisionError::ForeignHome(r.clone()));
    }
    Ok(r.uri().clone())
}

fn key_set<'k>(keys: impl Iterator<Item = &'k OptionKey>) -> Value {
    let joined = keys.map(OptionKey::as_str).collect::<Vec<_>>().join(" ");
    text_value(if joined.is_empty() {
        EMPTY_SET.to_string()
    } else {
        joined
    })
}

fn distribution(d: &Distribution) -> Value {
    if d.is_empty() {
        return text_value(EMPTY_SET);
    }
    let pairs: Vec<String> = d.iter().map(|(k, p)| format!("{k}={}", p.get())).collect();
    text_value(pairs.join(" "))
}

fn time(t: &DateTime<Utc>) -> Value {
    text_value(t.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn text_value(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

fn ruling_props(r: &Ruling) -> Vec<(&'static str, Value)> {
    let mut props = vec![
        (CHOSEN, key_set(r.chosen().keys())),
        (DECIDER, text_value(r.decider().to_string())),
        (DECIDED, time(&r.at())),
    ];
    if let Some(note) = r.note() {
        props.push((NOTE, text_value(note)));
    }
    props
}

fn withdrawal_props(by: &str, at: &DateTime<Utc>) -> Vec<(&'static str, Value)> {
    vec![(WITHDRAWER, text_value(by)), (WITHDRAWN, time(at))]
}

fn answer_create(
    id: &EntityUri,
    decision: &EntityUri,
    after: Option<&EntityUri>,
    answer: &Answer,
) -> Operation {
    let body = match answer.body() {
        AnswerBody::Pick(s) => (PICK, key_set(s.keys())),
        AnswerBody::Categorical(d) => (CATEGORICAL, distribution(d)),
        AnswerBody::Marginals(d) => (MARGINAL, distribution(d)),
    };
    let content = match answer.rationale() {
        Some(r) => format!("{ANSWER_TITLE}\n{r}"),
        None => ANSWER_TITLE.to_string(),
    };
    create(
        id,
        decision,
        after,
        &content,
        vec![
            (ANSWERER, text_value(answer.by().to_string())),
            (ANSWERED, time(&answer.at())),
            body,
        ],
    )
}

fn create(
    id: &EntityUri,
    parent: &EntityUri,
    after: Option<&EntityUri>,
    content: &str,
    props: Vec<(&'static str, Value)>,
) -> Operation {
    let mut params = vec![
        ("id".to_string(), text_value(id.as_str())),
        ("parent_id".to_string(), text_value(parent.as_str())),
        ("content".to_string(), text_value(content)),
    ];
    if let Some(after) = after {
        params.push((
            crate::entity::POSITION_AFTER_BLOCK_ID_PARAM.to_string(),
            text_value(after.as_str()),
        ));
    }
    params.extend(props.into_iter().map(|(k, v)| (k.to_string(), v)));
    Operation::from_params("block", "create", "Create", params)
}

fn set_field(id: &EntityUri, field: &str, value: Value) -> Operation {
    Operation::from_params(
        "block",
        "set_field",
        "Set field",
        [
            ("id".to_string(), text_value(id.as_str())),
            ("field".to_string(), text_value(field)),
            ("value".to_string(), value),
        ],
    )
}
