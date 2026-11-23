//! The operations that store a decision in its block home (see
//! [`holon_api::decision_block`]). Every write goes through [`ops`], which
//! refuses text org would not give back as written.

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::RemovedTag;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::decision::Answer;
use holon_api::decision::AnswerBody;
use holon_api::decision::Change;
use holon_api::decision::Decision;
use holon_api::decision::DecisionRef;
use holon_api::decision::Distribution;
use holon_api::decision::Effect;
use holon_api::decision::OptionKey;
use holon_api::decision::Ruling;
use holon_api::decision::Status;
use holon_api::decision_block::ANSWER_TITLE;
use holon_api::decision_block::ANSWERED;
use holon_api::decision_block::ANSWERER;
use holon_api::decision_block::BlockDecisionError;
use holon_api::decision_block::CATEGORICAL;
use holon_api::decision_block::CHOOSE;
use holon_api::decision_block::CHOSEN;
use holon_api::decision_block::DECIDED;
use holon_api::decision_block::DECIDER;
use holon_api::decision_block::DECISION_TAG;
use holon_api::decision_block::EMPTY_SET;
use holon_api::decision_block::Keyword;
use holon_api::decision_block::MARGINAL;
use holon_api::decision_block::NOTE;
use holon_api::decision_block::OPTION;
use holon_api::decision_block::PICK;
use holon_api::decision_block::RATIONALE;
use holon_api::decision_block::RECOMMEND;
use holon_api::decision_block::SUPERSEDES;
use holon_api::decision_block::TASK_STATE;
use holon_api::decision_block::WITHDRAWER;
use holon_api::decision_block::WITHDRAWN;
use holon_api::types::DEFAULT_ACTIVE_KEYWORDS;
use holon_api::types::DEFAULT_DONE_KEYWORDS;

use crate::encodable::refuse_unreadable_content;

/// What a decision write stores.
pub enum Write<'w> {
    /// Creates `decision` under `parent`, after its sibling `after`.
    Ask {
        decision: &'w Decision,
        parent: &'w EntityUri,
        after: Option<&'w EntityUri>,
    },
    /// Stores `change`; `children` are the decision block's direct children
    /// in tree order.
    Change {
        change: &'w Change,
        children: &'w [Block],
    },
}

/// The operations that store `write`, or why org cannot hold it. `mint`
/// names each new option and answer block.
pub fn ops(
    write: Write<'_>,
    mint: &mut dyn FnMut() -> EntityUri,
) -> Result<Vec<Operation>, BlockDecisionError> {
    let ops = match write {
        Write::Ask {
            decision,
            parent,
            after,
        } => ask(decision, parent, after, mint)?,
        Write::Change {
            change: c,
            children,
        } => change(c, children, mint)?,
    };
    refuse_unreadable_content(&ops)?;
    Ok(ops)
}

/// [`ops`] of [`Write::Ask`].
pub fn ask_ops(
    decision: &Decision,
    parent: &EntityUri,
    after: Option<&EntityUri>,
    mut mint: impl FnMut() -> EntityUri,
) -> Result<Vec<Operation>, BlockDecisionError> {
    ops(
        Write::Ask {
            decision,
            parent,
            after,
        },
        &mut mint,
    )
}

/// [`ops`] of [`Write::Change`]; `mint` names a new answer block.
pub fn change_ops(
    change: &Change,
    children: &[Block],
    mint: impl FnOnce() -> EntityUri,
) -> Result<Vec<Operation>, BlockDecisionError> {
    let mut mint = Some(mint);
    ops(Write::Change { change, children }, &mut || {
        mint.take().expect("a change mints at most one block")()
    })
}

fn ask(
    decision: &Decision,
    parent: &EntityUri,
    after: Option<&EntityUri>,
    mint: &mut dyn FnMut() -> EntityUri,
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
        Status::Decided(r) => props.extend(ruling_props(r)?),
        Status::Withdrawn { by, at } => props.extend(withdrawal_props(&by.to_string(), at)?),
    }
    props.push((
        TASK_STATE,
        text_value(Keyword::of_status(decision.status()).as_str()),
    ));
    props.push(("tags", Value::Array(vec![text_value(DECISION_TAG)])));

    let question = headline("question", decision.question(), Headline::Decision)?;
    let mut ops = vec![create(&id, parent, after, question, props)];
    let mut last: Option<EntityUri> = None;
    for option in decision.options().iter() {
        let child = mint();
        ops.push(create(
            &child,
            &id,
            last.as_ref(),
            headline("option label", &option.label, Headline::Option)?,
            vec![(OPTION, text_value(option.key.as_str()))],
        ));
        last = Some(child);
    }
    for answer in decision.answers() {
        let child = mint();
        ops.push(answer_create(&child, &id, last.as_ref(), answer)?);
        last = Some(child);
    }
    Ok(ops)
}

fn change(
    change: &Change,
    children: &[Block],
    mint: &mut dyn FnMut() -> EntityUri,
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
            vec![answer_create(&mint(), &id, after, answer)?]
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
                ruling_props(ruling)?
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
                withdrawal_props(&by.to_string(), at)?
                    .into_iter()
                    .map(|(k, v)| set_field(&id, k, v)),
            );
            ops
        }
    };
    Ok(ops)
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

type Props = Vec<(&'static str, Value)>;

fn ruling_props(r: &Ruling) -> Result<Props, BlockDecisionError> {
    let mut props = vec![
        (CHOSEN, key_set(r.chosen().keys())),
        (DECIDER, one_line(DECIDER, &r.decider().to_string())?),
        (DECIDED, time(&r.at())),
    ];
    if let Some(note) = r.note() {
        props.push((NOTE, one_line(NOTE, note.as_str())?));
    }
    Ok(props)
}

fn withdrawal_props(by: &str, at: &DateTime<Utc>) -> Result<Props, BlockDecisionError> {
    Ok(vec![
        (WITHDRAWER, one_line(WITHDRAWER, by)?),
        (WITHDRAWN, time(at)),
    ])
}

fn unencodable(field: &'static str, text: &str, reason: &'static str) -> BlockDecisionError {
    BlockDecisionError::Unencodable {
        field,
        text: text.to_string(),
        reason,
    }
}

/// A property value: org keeps one trimmed, non-empty line.
fn one_line(field: &'static str, text: &str) -> Result<Value, BlockDecisionError> {
    if text.contains(['\n', '\r']) {
        return Err(unencodable(field, text, "a property value is one line"));
    }
    if text.trim().is_empty() {
        return Err(unencodable(
            field,
            text,
            "an empty property value is not stored",
        ));
    }
    if text.trim() != text {
        return Err(unencodable(field, text, "org trims a property value"));
    }
    Ok(text_value(text))
}

/// Which headline a title goes into.
#[derive(Debug, Clone, Copy)]
enum Headline {
    /// Carries the decision's own keyword and ends in its `:decision:` tag.
    Decision,
    /// Carries no keyword and no tag.
    Option,
}

/// A headline title org gives back as the same text.
fn headline<'t>(
    field: &'static str,
    text: &'t str,
    headline: Headline,
) -> Result<&'t str, BlockDecisionError> {
    let refuse = |reason| Err(unencodable(field, text, reason));
    one_line(field, text)?;
    if has_link(text) {
        return refuse("org stores a link as marks");
    }
    if starts_with_priority_cookie(text) {
        return refuse("org reads a leading [#x] as the priority cookie");
    }
    match headline {
        Headline::Decision => Ok(text),
        Headline::Option => {
            let first = text.split(' ').next().unwrap_or(text);
            if DEFAULT_ACTIVE_KEYWORDS.contains(&first) || DEFAULT_DONE_KEYWORDS.contains(&first) {
                return refuse("org reads a leading task keyword as the block's state");
            }
            if holon_api::types::Tags::org_tag_group_start(text).is_some() {
                return refuse("org reads a trailing :tag: group as tags");
            }
            Ok(text)
        }
    }
}

fn has_link(text: &str) -> bool {
    text.find("[[")
        .is_some_and(|open| text[open + 2..].contains("]]"))
}

/// `[#` + one character + `]`, as in `[#A]`.
fn starts_with_priority_cookie(text: &str) -> bool {
    let mut rest = match text.strip_prefix("[#") {
        Some(rest) => rest.chars(),
        None => return false,
    };
    rest.next().is_some() && rest.as_str().starts_with(']')
}

/// Text org keeps verbatim as the lines of a block body.
fn body(field: &'static str, text: &str) -> Result<(), BlockDecisionError> {
    let refuse = |reason| Err(unencodable(field, text, reason));
    if text.contains('\r') {
        return refuse("org has no carriage return");
    }
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.first().is_some_and(|l| l.trim().is_empty())
        || lines.last().is_some_and(|l| l.trim().is_empty())
    {
        return refuse("org drops blank lines at the edges of a body");
    }
    if has_link(text) {
        return refuse("org stores a link as marks");
    }
    for (i, line) in lines.iter().enumerate() {
        if !line.trim().is_empty() && line.trim_end() != *line {
            return refuse("org trims the end of a body line");
        }
        if is_planning(i, line, lines.get(i + 1).copied()) {
            return refuse("org reads a SCHEDULED: or DEADLINE: line as the heading's planning");
        }
    }
    Ok(())
}

/// Org keeps a `SCHEDULED:` / `DEADLINE:` line as text only as the first,
/// unindented body line, with no active timestamp after it and no blank line
/// below it.
fn is_planning(index: usize, line: &str, next: Option<&str>) -> bool {
    let unindented = line.trim_start();
    let Some(rest) = ["SCHEDULED:", "DEADLINE:"]
        .iter()
        .find_map(|k| unindented.strip_prefix(k))
    else {
        return false;
    };
    let kept_as_text = index == 0
        && unindented.len() == line.len()
        && rest.starts_with(' ')
        && !rest.trim_start().is_empty()
        && !rest.trim_start().starts_with('<')
        && next.is_none_or(|n| !n.trim().is_empty());
    !kept_as_text
}

fn answer_create(
    id: &EntityUri,
    decision: &EntityUri,
    after: Option<&EntityUri>,
    answer: &Answer,
) -> Result<Operation, BlockDecisionError> {
    let answer_body = match answer.body() {
        AnswerBody::Pick(s) => (PICK, key_set(s.keys())),
        AnswerBody::Categorical(d) => (CATEGORICAL, distribution(d)),
        AnswerBody::Marginals(d) => (MARGINAL, distribution(d)),
    };
    let content = match answer.rationale() {
        Some(r) => {
            body(RATIONALE, r.as_str())?;
            format!("{ANSWER_TITLE}\n{r}")
        }
        None => ANSWER_TITLE.to_string(),
    };
    Ok(create(
        id,
        decision,
        after,
        &content,
        vec![
            (ANSWERER, one_line(ANSWERER, &answer.by().to_string())?),
            (ANSWERED, time(&answer.at())),
            answer_body,
        ],
    ))
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
            holon_api::entity::POSITION_AFTER_BLOCK_ID_PARAM.to_string(),
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
