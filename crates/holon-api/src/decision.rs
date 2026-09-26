//! The storage-neutral decision core: a question with options, answers from
//! many answerers, and one ruling by an allowed decider.
//!
//! A home (the block tree, an issue tracker) owns each decision and its id.
//! Its adapter reads native data into a [`DecisionDraft`]; [`Decision::parse`]
//! refuses every illegal state with a named DC rule; a [`Command`]
//! yields a [`Change`] for the adapter to write back. Nothing here does I/O.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU8;

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;

use crate::entity_uri::EntityUri;

/// `<source scheme>:<native id>`, for example `block:sharing-12` or
/// `github-issue:owner/repo/7`. The home mints it; the core never does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecisionRef(EntityUri);

impl DecisionRef {
    pub fn parse(raw: &str) -> Result<Self, DecisionError> {
        let malformed = |reason: String| DecisionError::MalformedRef {
            raw: raw.to_string(),
            reason,
        };
        let uri = EntityUri::parse(raw).map_err(|e| malformed(e.to_string()))?;
        if uri.id().is_empty() {
            return Err(malformed("no native id after the scheme".into()));
        }
        if uri.is_sentinel() {
            return Err(malformed("a sentinel names no decision".into()));
        }
        Ok(DecisionRef(uri))
    }

    pub fn uri(&self) -> &EntityUri {
        &self.0
    }
}

impl fmt::Display for DecisionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// `[a-z0-9-]+`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OptionKey(String);

impl OptionKey {
    pub fn parse(raw: &str) -> Result<Self, DecisionError> {
        let well_formed = !raw.is_empty()
            && raw
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !well_formed {
            return Err(DecisionError::MalformedOptionKey(raw.to_string()));
        }
        Ok(OptionKey(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for OptionKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OptionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Free text with at least one non-whitespace character: a ruling's note or
/// an answer's rationale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prose(String);

impl Prose {
    pub fn parse(raw: &str, site: TextSite) -> Result<Self, DecisionError> {
        if raw.trim().is_empty() {
            return Err(DecisionError::BlankText(site));
        }
        Ok(Prose(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Prose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a [`Prose`] sits in a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSite {
    Note,
    Rationale,
}

impl fmt::Display for TextSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TextSite::Note => "note",
            TextSite::Rationale => "rationale",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOption {
    pub key: OptionKey,
    pub label: String,
}

/// Non-empty, unique keys, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options(Vec<DecisionOption>);

impl Options {
    fn parse(raw: &[(String, String)]) -> Result<Self, DecisionError> {
        let mut seen = BTreeSet::new();
        let mut options = Vec::with_capacity(raw.len());
        for (key, label) in raw {
            let key = OptionKey::parse(key)?;
            if !seen.insert(key.clone()) {
                return Err(DecisionError::DuplicateOptionKey(key));
            }
            options.push(DecisionOption {
                key,
                label: label.clone(),
            });
        }
        if options.is_empty() {
            return Err(DecisionError::NoOptions);
        }
        Ok(Options(options))
    }

    pub fn iter(&self) -> impl Iterator<Item = &DecisionOption> {
        self.0.iter()
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, raw: &str) -> Option<&OptionKey> {
        self.0.iter().map(|o| &o.key).find(|k| k.as_str() == raw)
    }
}

/// How many options a selection holds: `min..=max`, `max <= options`.
/// Written `n` (exactly n) or `min..max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cardinality {
    min: u8,
    max: NonZeroU8,
}

impl Cardinality {
    const SINGLE: Cardinality = Cardinality {
        min: 1,
        max: NonZeroU8::MIN,
    };

    fn parse(raw: &str, options: usize) -> Result<Self, DecisionError> {
        let malformed = || DecisionError::MalformedCardinality(raw.to_string());
        let (min, max) = raw.split_once("..").unwrap_or((raw, raw));
        let min: u8 = min.parse().map_err(|_| malformed())?;
        let max: NonZeroU8 = max.parse().map_err(|_| malformed())?;
        if min > max.get() {
            return Err(malformed());
        }
        let c = Cardinality { min, max };
        if usize::from(max.get()) > options {
            return Err(DecisionError::CardinalityExceedsOptions { choose: c, options });
        }
        Ok(c)
    }

    pub fn min(&self) -> u8 {
        self.min
    }

    pub fn max(&self) -> u8 {
        self.max.get()
    }

    fn admits(&self, size: usize) -> bool {
        (usize::from(self.min)..=usize::from(self.max.get())).contains(&size)
    }
}

impl fmt::Display for Cardinality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.min, self.max)
    }
}

/// Option keys of one decision, no repeats, count within its `choose`.
/// Only a decision's own parse and commands construct it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection(BTreeSet<OptionKey>);

impl Selection {
    fn parse<K: AsRef<str>>(
        keys: &[K],
        options: &Options,
        choose: Cardinality,
        site: Site,
    ) -> Result<Self, DecisionError> {
        let keys = distinct_option_keys(keys.iter().map(AsRef::as_ref), options, site)?;
        if !choose.admits(keys.len()) {
            return Err(DecisionError::SelectionSize {
                site,
                size: keys.len(),
                choose,
            });
        }
        Ok(Selection(keys))
    }

    pub fn keys(&self) -> impl Iterator<Item = &OptionKey> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn to_raw(&self) -> Vec<String> {
        self.0.iter().map(|k| k.0.clone()).collect()
    }
}

/// DC2: each key names an option of this decision, once.
fn distinct_option_keys<'a>(
    keys: impl Iterator<Item = &'a str>,
    options: &Options,
    site: Site,
) -> Result<BTreeSet<OptionKey>, DecisionError> {
    let mut out = BTreeSet::new();
    for raw in keys {
        let key = options
            .key(raw)
            .ok_or_else(|| DecisionError::UnknownOptionKey {
                site,
                key: raw.to_string(),
            })?;
        if !out.insert(key.clone()) {
            return Err(DecisionError::RepeatedOptionKey {
                site,
                key: key.clone(),
            });
        }
    }
    Ok(out)
}

/// Written `person:<name>`, `agent:<name>` or `model:<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Answerer {
    Person(String),
    Agent(String),
    Model(String),
}

impl Answerer {
    pub fn parse(raw: &str) -> Result<Self, DecisionError> {
        let malformed = || DecisionError::MalformedAnswerer(raw.to_string());
        let (kind, name) = raw.split_once(':').ok_or_else(malformed)?;
        if name.is_empty() {
            return Err(malformed());
        }
        let name = name.to_string();
        match kind {
            "person" => Ok(Answerer::Person(name)),
            "agent" => Ok(Answerer::Agent(name)),
            "model" => Ok(Answerer::Model(name)),
            _ => Err(malformed()),
        }
    }
}

impl fmt::Display for Answerer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Answerer::Person(n) => write!(f, "person:{n}"),
            Answerer::Agent(n) => write!(f, "agent:{n}"),
            Answerer::Model(n) => write!(f, "model:{n}"),
        }
    }
}

/// An answerer that may rule: a model only suggests (DC8).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Decider {
    Person(String),
    Agent(String),
}

impl Decider {
    pub fn answerer(&self) -> Answerer {
        match self {
            Decider::Person(n) => Answerer::Person(n.clone()),
            Decider::Agent(n) => Answerer::Agent(n.clone()),
        }
    }
}

impl TryFrom<Answerer> for Decider {
    type Error = DecisionError;

    fn try_from(a: Answerer) -> Result<Self, DecisionError> {
        match a {
            Answerer::Person(n) => Ok(Decider::Person(n)),
            Answerer::Agent(n) => Ok(Decider::Agent(n)),
            Answerer::Model(_) => Err(DecisionError::ModelCannotDecide(a)),
        }
    }
}

impl fmt::Display for Decider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.answerer().fmt(f)
    }
}

/// The deciders a home accepts a ruling from (DC8), resolved from its
/// configuration by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedDeciders(BTreeSet<Decider>);

impl AllowedDeciders {
    pub fn new(deciders: impl IntoIterator<Item = Decider>) -> Self {
        AllowedDeciders(deciders.into_iter().collect())
    }

    pub fn contains(&self, d: &Decider) -> bool {
        self.0.contains(d)
    }
}

/// A finite value in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability(f64);

impl Probability {
    pub fn get(&self) -> f64 {
        self.0
    }
}

pub type Distribution = BTreeMap<OptionKey, Probability>;

#[derive(Debug, Clone, PartialEq)]
pub enum AnswerBody {
    Pick(Selection),
    /// One distribution over the options; `1 - sum` is "none of these".
    /// Only when `choose.max == 1`, sum `<= 1` (DC5).
    Categorical(Distribution),
    /// One independent yes/no probability per option.
    Marginals(Distribution),
}

impl AnswerBody {
    fn parse<K: AsRef<str>>(
        input: &AnswerInput<K>,
        options: &Options,
        choose: Cardinality,
    ) -> Result<Self, DecisionError> {
        match input {
            AnswerInput::Pick(keys) => {
                Selection::parse(keys, options, choose, Site::Answer).map(AnswerBody::Pick)
            }
            AnswerInput::Categorical(pairs) => {
                if choose.max() != 1 {
                    return Err(DecisionError::CategoricalNeedsSingleSelect(choose));
                }
                let d = distribution(pairs, options)?;
                let sum: f64 = d.values().map(Probability::get).sum();
                if sum > 1.0 {
                    return Err(DecisionError::ProbabilitySumAboveOne(sum));
                }
                Ok(AnswerBody::Categorical(d))
            }
            AnswerInput::Marginals(pairs) => {
                distribution(pairs, options).map(AnswerBody::Marginals)
            }
        }
    }

    fn to_raw(&self) -> AnswerInput<String> {
        let pairs = |d: &Distribution| d.iter().map(|(k, p)| (k.0.clone(), p.0)).collect();
        match self {
            AnswerBody::Pick(s) => AnswerInput::Pick(s.to_raw()),
            AnswerBody::Categorical(d) => AnswerInput::Categorical(pairs(d)),
            AnswerBody::Marginals(d) => AnswerInput::Marginals(pairs(d)),
        }
    }
}

fn distribution<K: AsRef<str>>(
    pairs: &[(K, f64)],
    options: &Options,
) -> Result<Distribution, DecisionError> {
    let keys = pairs.iter().map(|(k, _)| k.as_ref());
    distinct_option_keys(keys, options, Site::Answer)?;
    pairs
        .iter()
        .map(|(k, p)| {
            let key = options.key(k.as_ref()).expect("checked above").clone();
            if !(0.0..=1.0).contains(p) {
                return Err(DecisionError::ProbabilityOutOfRange { key, p: *p });
            }
            Ok((key, Probability(*p)))
        })
        .collect()
}

/// An answer's body before it is checked against a decision's options.
#[derive(Debug, Clone, PartialEq)]
pub enum AnswerInput<K> {
    Pick(Vec<K>),
    Categorical(Vec<(K, f64)>),
    Marginals(Vec<(K, f64)>),
}

/// A suggestion: it never changes the status.
///
/// ```compile_fail,E0451
/// use holon_api::decision::{Answer, AnswerBody, Distribution};
/// fn forge(of: &Answer, sums_to_two: Distribution) -> Answer {
///     Answer { by: of.by().clone(), at: of.at(), body: AnswerBody::Categorical(sums_to_two), rationale: None }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    by: Answerer,
    at: DateTime<Utc>,
    body: AnswerBody,
    rationale: Option<Prose>,
}

impl Answer {
    pub fn by(&self) -> &Answerer {
        &self.by
    }

    pub fn at(&self) -> DateTime<Utc> {
        self.at
    }

    pub fn body(&self) -> &AnswerBody {
        &self.body
    }

    pub fn rationale(&self) -> Option<&Prose> {
        self.rationale.as_ref()
    }
}

/// ```compile_fail,E0451
/// use holon_api::decision::{Decider, Ruling};
/// fn forge(of_another_decision: &Ruling) -> Ruling {
///     let chosen = of_another_decision.chosen().clone();
///     Ruling { chosen, decider: Decider::Person("eve".into()), at: of_another_decision.at(), note: None }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    chosen: Selection,
    decider: Decider,
    at: DateTime<Utc>,
    note: Option<Prose>,
}

impl Ruling {
    pub fn chosen(&self) -> &Selection {
        &self.chosen
    }

    pub fn decider(&self) -> &Decider {
        &self.decider
    }

    pub fn at(&self) -> DateTime<Utc> {
        self.at
    }

    pub fn note(&self) -> Option<&Prose> {
        self.note.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Open,
    Decided(Ruling),
    Withdrawn { by: Answerer, at: DateTime<Utc> },
}

/// A foreign input item the adapter could not read, kept for disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedInput {
    pub item: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRuling {
    pub chosen: Option<Vec<String>>,
    pub decider: Option<String>,
    pub at: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawWithdrawal {
    pub by: Option<String>,
    pub at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawAnswer {
    pub by: String,
    pub at: String,
    pub body: AnswerInput<String>,
    pub rationale: Option<String>,
}

/// What a caller asks: the new decision's content, without an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftQuestion {
    pub question: String,
    /// `(key, label)` in source order.
    pub options: Vec<(String, String)>,
    /// Absent means `1`.
    pub choose: Option<String>,
    pub recommend: Option<Vec<String>>,
    pub supersedes: Option<String>,
}

/// What an adapter reads from its home. Raw: strings, not keys.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionDraft {
    pub id: DecisionRef,
    pub label: String,
    pub question: DraftQuestion,
    pub ruling: Option<RawRuling>,
    pub withdrawn: Option<RawWithdrawal>,
    pub answers: Vec<RawAnswer>,
    pub refused: Vec<RefusedInput>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    id: DecisionRef,
    label: String,
    question: String,
    options: Options,
    choose: Cardinality,
    recommend: Option<Selection>,
    status: Status,
    answers: Vec<Answer>,
    supersedes: Option<DecisionRef>,
    refused: Vec<RefusedInput>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Answer {
        by: Answerer,
        body: AnswerInput<OptionKey>,
        rationale: Option<Prose>,
    },
    /// Legal on `Open` and `Decided`.
    Decide {
        chosen: Vec<OptionKey>,
        decider: Answerer,
        note: Option<Prose>,
    },
    /// Legal on `Open` only; a ruling is reversed by deciding again.
    Withdraw { by: Answerer },
}

/// What one command does to one state of one decision. Only
/// [`Decision::apply`] makes it, and [`Decision::commit`] takes it only on
/// that same state.
///
/// ```compile_fail,E0451
/// use holon_api::decision::{Change, Decision, Effect};
/// fn forge(basis: Decision, effect: Effect) -> Change {
///     Change { basis, effect }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    basis: Decision,
    effect: Effect,
}

impl Change {
    pub fn decision(&self) -> &DecisionRef {
        &self.basis.id
    }

    pub fn effect(&self) -> &Effect {
        &self.effect
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Answered(Answer),
    /// `replaces` is the ruling this one overwrites in place (DC9). Every
    /// reader's read mark on the decision is stale after it.
    Decided {
        ruling: Ruling,
        replaces: Option<Ruling>,
    },
    Withdrawn {
        by: Answerer,
        at: DateTime<Utc>,
    },
}

/// The rule a [`DecisionError`] enforces. `Syntax` is a malformed value that
/// no DC rule names; `Stale` is a [`Change`] committed to a state it was not
/// made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    Dc1,
    Dc2,
    Dc3,
    Dc4,
    Dc5,
    Dc7,
    Dc8,
    Syntax,
    Stale,
}

/// Where a key set sits in a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    Recommend,
    Ruling,
    Answer,
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Site::Recommend => "recommend",
            Site::Ruling => "chosen",
            Site::Answer => "answer",
        })
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecisionError {
    #[error("DC1: a decision needs at least one option")]
    NoOptions,
    #[error("DC1: option key {0:?} is not [a-z0-9-]+")]
    MalformedOptionKey(String),
    #[error("DC1: option key {0} appears twice")]
    DuplicateOptionKey(OptionKey),
    #[error("DC2: {site} names {key:?}, which is no option of this decision")]
    UnknownOptionKey { site: Site, key: String },
    #[error("DC2: {site} names option {key} twice")]
    RepeatedOptionKey { site: Site, key: OptionKey },
    #[error("DC3: choose {0:?} is not `n` or `min..max` with 0 <= min <= max, 1 <= max <= 255")]
    MalformedCardinality(String),
    #[error("DC3: choose {choose} exceeds the {options} options")]
    CardinalityExceedsOptions { choose: Cardinality, options: usize },
    #[error("DC3: {site} holds {size} options; choose is {choose}")]
    SelectionSize {
        site: Site,
        size: usize,
        choose: Cardinality,
    },
    #[error("DC4: a ruling needs {0}")]
    IncompleteRuling(&'static str),
    #[error("DC4: a withdrawal needs {0}")]
    IncompleteWithdrawal(&'static str),
    #[error("DC4: a decision is decided or withdrawn, not both")]
    RulingAndWithdrawal,
    #[error("DC4: a withdrawn decision takes no ruling")]
    DecideWithdrawn,
    #[error("DC4: only an open decision can be withdrawn")]
    WithdrawClosed,
    #[error("DC5: a categorical answer needs choose max 1; choose is {0}")]
    CategoricalNeedsSingleSelect(Cardinality),
    #[error("DC5: probability {p} for option {key} is outside [0, 1]")]
    ProbabilityOutOfRange { key: OptionKey, p: f64 },
    #[error("DC5: categorical probabilities sum to {0}, above 1")]
    ProbabilitySumAboveOne(f64),
    #[error("DC7: {raw:?} is not a decision reference: {reason}")]
    MalformedRef { raw: String, reason: String },
    #[error("DC7: decision {0} supersedes itself")]
    SupersedesItself(DecisionRef),
    #[error("DC8: {0} is a model; its answers are suggestions and never rule")]
    ModelCannotDecide(Answerer),
    #[error("DC8: {0} is not an allowed decider")]
    DeciderNotAllowed(Decider),
    #[error("answerer {0:?} is not person:<name>, agent:<name> or model:<name>")]
    MalformedAnswerer(String),
    #[error("a {0} holds no text")]
    BlankText(TextSite),
    #[error("time {raw:?} is not RFC 3339: {reason}")]
    MalformedTime { raw: String, reason: String },
    #[error("a change made for a state of {0} other than the one it is committed to")]
    StaleChange(DecisionRef),
}

impl DecisionError {
    pub fn rule(&self) -> Rule {
        use DecisionError::*;
        match self {
            NoOptions | MalformedOptionKey(_) | DuplicateOptionKey(_) => Rule::Dc1,
            UnknownOptionKey { .. } | RepeatedOptionKey { .. } => Rule::Dc2,
            MalformedCardinality(_) | CardinalityExceedsOptions { .. } | SelectionSize { .. } => {
                Rule::Dc3
            }
            IncompleteRuling(_)
            | IncompleteWithdrawal(_)
            | RulingAndWithdrawal
            | DecideWithdrawn
            | WithdrawClosed => Rule::Dc4,
            CategoricalNeedsSingleSelect(_)
            | ProbabilityOutOfRange { .. }
            | ProbabilitySumAboveOne(_) => Rule::Dc5,
            MalformedRef { .. } | SupersedesItself(_) => Rule::Dc7,
            ModelCannotDecide(_) | DeciderNotAllowed(_) => Rule::Dc8,
            MalformedAnswerer(_) | MalformedTime { .. } | BlankText(_) => Rule::Syntax,
            StaleChange(_) => Rule::Stale,
        }
    }
}

fn parse_time(raw: &str) -> Result<DateTime<Utc>, DecisionError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| DecisionError::MalformedTime {
            raw: raw.to_string(),
            reason: e.to_string(),
        })
}

fn format_time(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn prose(raw: Option<&str>, site: TextSite) -> Result<Option<Prose>, DecisionError> {
    raw.map(|r| Prose::parse(r, site)).transpose()
}

fn parse_status(
    ruling: Option<&RawRuling>,
    withdrawn: Option<&RawWithdrawal>,
    options: &Options,
    choose: Cardinality,
) -> Result<Status, DecisionError> {
    match (ruling, withdrawn) {
        (None, None) => Ok(Status::Open),
        (Some(_), Some(_)) => Err(DecisionError::RulingAndWithdrawal),
        (Some(r), None) => {
            let missing = DecisionError::IncompleteRuling;
            let chosen = r.chosen.as_ref().ok_or(missing("chosen"))?;
            let decider = r.decider.as_ref().ok_or(missing("decider"))?;
            let at = r.at.as_ref().ok_or(missing("a time"))?;
            let decider = Decider::try_from(Answerer::parse(decider)?)?;
            let at = parse_time(at)?;
            let chosen = Selection::parse(chosen, options, choose, Site::Ruling)?;
            Ok(Status::Decided(Ruling {
                chosen,
                decider,
                at,
                note: prose(r.note.as_deref(), TextSite::Note)?,
            }))
        }
        (None, Some(w)) => {
            let missing = DecisionError::IncompleteWithdrawal;
            let by = w.by.as_ref().ok_or(missing("by"))?;
            let at = w.at.as_ref().ok_or(missing("a time"))?;
            Ok(Status::Withdrawn {
                by: Answerer::parse(by)?,
                at: parse_time(at)?,
            })
        }
    }
}

impl Decision {
    pub fn parse(d: DecisionDraft) -> Result<Decision, DecisionError> {
        let q = d.question;
        let options = Options::parse(&q.options)?;
        let choose = match &q.choose {
            None => Cardinality::SINGLE,
            Some(raw) => Cardinality::parse(raw, options.len())?,
        };
        let recommend = q
            .recommend
            .map(|r| Selection::parse(&r, &options, choose, Site::Recommend))
            .transpose()?;
        let supersedes = q
            .supersedes
            .map(|raw| DecisionRef::parse(&raw))
            .transpose()?;
        if supersedes.as_ref() == Some(&d.id) {
            return Err(DecisionError::SupersedesItself(d.id));
        }
        let status = parse_status(d.ruling.as_ref(), d.withdrawn.as_ref(), &options, choose)?;
        let answers = d
            .answers
            .iter()
            .map(|a| {
                Ok(Answer {
                    by: Answerer::parse(&a.by)?,
                    at: parse_time(&a.at)?,
                    body: AnswerBody::parse(&a.body, &options, choose)?,
                    rationale: prose(a.rationale.as_deref(), TextSite::Rationale)?,
                })
            })
            .collect::<Result<_, DecisionError>>()?;
        Ok(Decision {
            id: d.id,
            label: d.label,
            question: q.question,
            options,
            choose,
            recommend,
            status,
            answers,
            supersedes,
            refused: d.refused,
        })
    }

    /// A new, open decision in the home that minted `id`.
    pub fn ask(
        id: DecisionRef,
        label: String,
        q: DraftQuestion,
    ) -> Result<Decision, DecisionError> {
        Decision::parse(DecisionDraft {
            id,
            label,
            question: q,
            ruling: None,
            withdrawn: None,
            answers: Vec::new(),
            refused: Vec::new(),
        })
    }

    pub fn apply(
        &self,
        c: Command,
        now: DateTime<Utc>,
        deciders: &AllowedDeciders,
    ) -> Result<Change, DecisionError> {
        let effect = match c {
            Command::Answer {
                by,
                body,
                rationale,
            } => Effect::Answered(Answer {
                by,
                at: now,
                body: AnswerBody::parse(&body, &self.options, self.choose)?,
                rationale,
            }),
            Command::Decide {
                chosen,
                decider,
                note,
            } => {
                let replaces = match &self.status {
                    Status::Open => None,
                    Status::Decided(r) => Some(r.clone()),
                    Status::Withdrawn { .. } => return Err(DecisionError::DecideWithdrawn),
                };
                let decider = Decider::try_from(decider)?;
                if !deciders.contains(&decider) {
                    return Err(DecisionError::DeciderNotAllowed(decider));
                }
                let chosen = Selection::parse(&chosen, &self.options, self.choose, Site::Ruling)?;
                Effect::Decided {
                    ruling: Ruling {
                        chosen,
                        decider,
                        at: now,
                        note,
                    },
                    replaces,
                }
            }
            Command::Withdraw { by } => match self.status {
                Status::Open => Effect::Withdrawn { by, at: now },
                _ => return Err(DecisionError::WithdrawClosed),
            },
        };
        Ok(Change {
            basis: self.clone(),
            effect,
        })
    }

    /// The decision after `change`, which [`Self::apply`] made from `self`.
    pub fn commit(&self, change: Change) -> Result<Decision, DecisionError> {
        if change.basis != *self {
            return Err(DecisionError::StaleChange(change.basis.id));
        }
        let mut next = change.basis;
        match change.effect {
            Effect::Answered(a) => next.answers.push(a),
            Effect::Decided { ruling, .. } => next.status = Status::Decided(ruling),
            Effect::Withdrawn { by, at } => next.status = Status::Withdrawn { by, at },
        }
        Ok(next)
    }

    /// The draft that parses back to `self`.
    pub fn to_draft(&self) -> DecisionDraft {
        let (ruling, withdrawn) = match &self.status {
            Status::Open => (None, None),
            Status::Decided(r) => (
                Some(RawRuling {
                    chosen: Some(r.chosen.to_raw()),
                    decider: Some(r.decider.to_string()),
                    at: Some(format_time(&r.at)),
                    note: r.note.as_ref().map(ToString::to_string),
                }),
                None,
            ),
            Status::Withdrawn { by, at } => (
                None,
                Some(RawWithdrawal {
                    by: Some(by.to_string()),
                    at: Some(format_time(at)),
                }),
            ),
        };
        DecisionDraft {
            id: self.id.clone(),
            label: self.label.clone(),
            question: DraftQuestion {
                question: self.question.clone(),
                options: self
                    .options
                    .iter()
                    .map(|o| (o.key.0.clone(), o.label.clone()))
                    .collect(),
                choose: Some(self.choose.to_string()),
                recommend: self.recommend.as_ref().map(Selection::to_raw),
                supersedes: self.supersedes.as_ref().map(ToString::to_string),
            },
            ruling,
            withdrawn,
            answers: self
                .answers
                .iter()
                .map(|a| RawAnswer {
                    by: a.by.to_string(),
                    at: format_time(&a.at),
                    body: a.body.to_raw(),
                    rationale: a.rationale.as_ref().map(ToString::to_string),
                })
                .collect(),
            refused: self.refused.clone(),
        }
    }

    pub fn id(&self) -> &DecisionRef {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    pub fn choose(&self) -> Cardinality {
        self.choose
    }

    pub fn recommend(&self) -> Option<&Selection> {
        self.recommend.as_ref()
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn answers(&self) -> &[Answer] {
        &self.answers
    }

    pub fn supersedes(&self) -> Option<&DecisionRef> {
        self.supersedes.as_ref()
    }

    pub fn refused(&self) -> &[RefusedInput] {
        &self.refused
    }
}
