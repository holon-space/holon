//! The decision block adapter against the org replica: a legal decision
//! written as block operations survives org render and parse and reads back
//! equal; a change written as block operations reads back as the committed
//! decision; an illegal subtree reads as its named error.

use std::path::Path;

use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::decision::AllowedDeciders;
use holon_api::decision::AnswerInput;
use holon_api::decision::Answerer;
use holon_api::decision::Change;
use holon_api::decision::Command;
use holon_api::decision::Decider;
use holon_api::decision::Decision;
use holon_api::decision::DecisionError;
use holon_api::decision::DecisionRef;
use holon_api::decision::DraftQuestion;
use holon_api::decision::OptionKey;
use holon_api::decision::Prose;
use holon_api::decision::Rule;
use holon_api::decision::Status;
use holon_api::decision::TextSite;
use holon_api::decision_block::ANSWER_TITLE;
use holon_api::decision_block::BlockDecisionError;
use holon_api::types::TaskState;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::decision_block;
use holon_org_format::parse_org_file;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/decisions.org";
const SKELETON: &str = "#+ID: decisions\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
const DECISION_ID: &str = "dq-1";

const KEYS: &[&str] = &["a", "b", "c", "rename-2"];
const ANSWERERS: &[&str] = &["person:martin", "person:eve", "agent:orch", "model:jev-1"];
const HAZARD_ANSWERERS: &[&str] = &["person:eve\n* evil", "person: spaced "];
const DECIDERS: &[&str] = &["person:martin", "agent:orch"];

fn allowed() -> AllowedDeciders {
    AllowedDeciders::new(
        DECIDERS
            .iter()
            .map(|d| Decider::try_from(Answerer::parse(d).expect("answerer")).expect("decider")),
    )
}

fn instant(step: u32, millis: u32) -> DateTime<Utc> {
    Utc.timestamp_opt(1_790_000_000 + i64::from(step) * 61, millis * 1_000_000)
        .single()
        .expect("instant")
}

/// The block store as a flat list in sibling order, plus its org document.
struct Vault {
    document: Block,
    blocks: Vec<Block>,
    minted: u32,
}

impl Vault {
    fn new() -> Self {
        let (document, blocks) = parse(SKELETON);
        Vault {
            document,
            blocks,
            minted: 0,
        }
    }

    fn topic(&self) -> EntityUri {
        EntityUri::block("topic")
    }

    fn mint(&mut self) -> EntityUri {
        self.minted += 1;
        EntityUri::block(&format!("{DECISION_ID}-c{}", self.minted))
    }

    fn apply(&mut self, ops: &[Operation]) {
        for op in ops {
            assert_eq!(op.entity_name.as_str(), "block", "{op:?}");
            match op.op_name.as_str() {
                "create" => self.create(op),
                "set_field" => self.set_field(op),
                other => panic!("the block store has no operation {other:?}: {op:?}"),
            }
        }
    }

    fn create(&mut self, op: &Operation) {
        let id = uri_param(op, "id");
        let parent = uri_param(op, "parent_id");
        let mut block = Block::new_text(id, parent.clone(), text_param(op, "content"));
        for (key, value) in &op.params {
            match key.as_str() {
                "id" | "parent_id" | "after_block_id" | "content" => {}
                "tags" => {
                    let Value::Array(tags) = value else {
                        panic!("tags must be an array: {op:?}")
                    };
                    for tag in tags {
                        block.tags.insert(tag.as_string().expect("tag text"));
                    }
                }
                "task_state" => block.set_task_state(Some(TaskState::from_keyword(
                    value.as_string().expect("keyword text"),
                ))),
                _ => block.set_property(key.clone(), value.clone()),
            }
        }
        let at = match op.params.get("after_block_id") {
            Some(_) => {
                let after = uri_param(op, "after_block_id");
                let i = self
                    .blocks
                    .iter()
                    .position(|b| b.id == after)
                    .unwrap_or_else(|| panic!("no block {after} to place after: {op:?}"));
                assert_eq!(
                    self.blocks[i].parent_id, parent,
                    "after-anchor is no sibling"
                );
                i + 1
            }
            None => self
                .blocks
                .iter()
                .position(|b| b.parent_id == parent)
                .unwrap_or(self.blocks.len()),
        };
        self.blocks.insert(at, block);
    }

    fn set_field(&mut self, op: &Operation) {
        let id = uri_param(op, "id");
        let field = text_param(op, "field");
        let value = op.params.get("value").expect("set_field value");
        let block = self
            .blocks
            .iter_mut()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("no block {id}: {op:?}"));
        match (field.as_str(), value) {
            ("task_state", Value::String(k)) => {
                block.set_task_state(Some(TaskState::from_keyword(k)))
            }
            (_, v) if v.is_removed() => {
                block.properties.remove(&field);
            }
            (_, Value::String(_)) => block.set_property(field, value.clone()),
            _ => panic!("unexpected set_field: {op:?}"),
        }
    }

    fn round_trip_org(&mut self) {
        let text = render(&self.document, &self.blocks);
        let (document, blocks) = parse(&text);
        assert_eq!(
            render(&document, &blocks),
            text,
            "org render is not a fixed point"
        );
        self.document = document;
        self.blocks = blocks;
    }

    fn decision_id(&self) -> EntityUri {
        EntityUri::block(DECISION_ID)
    }

    fn decision(&self) -> (Block, Vec<Block>) {
        let id = self.decision_id();
        let block = self.block(&id).clone();
        let children = self
            .blocks
            .iter()
            .filter(|b| b.parent_id == id)
            .cloned()
            .collect();
        (block, children)
    }

    fn block(&self, id: &EntityUri) -> &Block {
        self.blocks
            .iter()
            .find(|b| &b.id == id)
            .unwrap_or_else(|| panic!("no block {id}"))
    }

    fn block_mut(&mut self, id: &EntityUri) -> &mut Block {
        self.blocks
            .iter_mut()
            .find(|b| &b.id == id)
            .unwrap_or_else(|| panic!("no block {id}"))
    }

    fn read(&self) -> Result<Decision, BlockDecisionError> {
        let (block, children) = self.decision();
        holon_api::decision_block::parse(&block, &children)
    }
}

fn uri_param(op: &Operation, key: &str) -> EntityUri {
    EntityUri::parse(&text_param(op, key)).unwrap_or_else(|e| panic!("{key}: {e}: {op:?}"))
}

fn text_param(op: &Operation, key: &str) -> String {
    op.params
        .get(key)
        .and_then(Value::as_string)
        .unwrap_or_else(|| panic!("no text param {key}: {op:?}"))
        .to_string()
}

fn parse(source: &str) -> (Block, Vec<Block>) {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parse");
    (parsed.document, parsed.blocks)
}

fn render(document: &Block, blocks: &[Block]) -> String {
    OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
        .expect("org render")
        .text
}

fn words(first: &'static str) -> impl Strategy<Value = String> + Clone {
    (first, "( [a-z]{1,8}){0,3}").prop_map(|(a, b)| format!("{a}{b}"))
}

/// Lines a block body holds verbatim.
const PLAIN_LINES: &[&str] = &[
    "Keep it short",
    "  indented line",
    "\tlead tab",
    ":END:",
    ":PROPERTIES:",
    "- [ ] item",
    "|a|b|",
    "*x marks",
    "*\tx",
    "**",
    "#1 is my choice",
    "#hashtag start",
    "# comment",
    "#+begin_quote",
    "#+:",
    "CLOSED: [2026-01-01 Thu]",
    "x SCHEDULED: <2026-01-01 Thu>",
    "x [y]",
    "x [[",
    "* heading",
    "** heading",
    "*  x",
    "#+TITLE: x",
    "#+NAME: x",
    "#+RESULTS:",
    "#+begin_src\nfn\n#+end_src",
];
/// Lines a body holds verbatim only as its first line.
const PLAIN_FIRST_LINES: &[&str] = &["SCHEDULED: tomorrow", "DEADLINE: soon"];
/// Lines org reads as structure, trims, moves, or rewrites.
const HAZARD_LINES: &[&str] = &[
    "",
    "   ",
    "DEADLINE: <2026-01-01 Thu>",
    "SCHEDULED: <2026-01-01 Thu>",
    "SCHEDULED: tomorrow",
    "  SCHEDULED: tomorrow",
    "DEADLINE: tomorrow",
    "see [[x]]",
    "trailing  ",
    " leading",
];

fn line(hazard: bool) -> BoxedStrategy<String> {
    let plain = prop_oneof![
        words("[A-Z][a-z]{1,8}"),
        prop::sample::select(PLAIN_LINES).prop_map(str::to_string),
    ];
    if hazard {
        prop_oneof![
            plain,
            prop::sample::select(HAZARD_LINES).prop_map(str::to_string)
        ]
        .boxed()
    } else {
        plain.boxed()
    }
}

fn lines(hazard: bool) -> BoxedStrategy<String> {
    (
        prop::option::of(prop::sample::select(PLAIN_FIRST_LINES)),
        prop::collection::vec(line(hazard), 1..4),
    )
        .prop_map(|(first, rest)| {
            first
                .into_iter()
                .map(str::to_string)
                .chain(rest)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .boxed()
}

#[derive(Debug, Clone, Copy)]
enum Headline {
    Question,
    Label,
}

/// Titles org keeps verbatim in both headlines.
const PLAIN_TITLES: &[&str] = &[
    "COMMENT x",
    "Comment x",
    "x [#A]",
    "[#10] x",
    "x [1/2]",
    "x *bold*",
    "x <2026-01-01 Thu>",
    "x :y:z",
    "x :tag: y",
];
/// Kept verbatim by an option headline, which carries no keyword or tag.
const PLAIN_LABELS: &[&str] = &["Pick ::", "x ::", "x :"];
/// Kept verbatim by the decision headline, which carries its own keyword.
const PLAIN_QUESTIONS: &[&str] = &["? x", "TODO x", "DONE x", "Pick ::", "x:y:"];
/// Titles org reads as a cookie, keyword, tags or link, or refuses to parse.
const HAZARD_TITLES: &[&str] = &[
    "[#A] which one?",
    "[#a] x",
    "[#A]x",
    "x [[y]]",
    "x :tag:",
    ":tag:",
    "Pick ::",
    "x:y:",
    "TODO x",
    "? x",
    "LATER x",
    "x ",
    "",
];

fn title(hazard: bool, headline: Headline) -> BoxedStrategy<String> {
    let own = match headline {
        Headline::Question => PLAIN_QUESTIONS,
        Headline::Label => PLAIN_LABELS,
    };
    let plain = prop_oneof![
        3 => words("[A-Z][a-z]{1,8}"),
        1 => prop::sample::select(PLAIN_TITLES).prop_map(str::to_string),
        1 => prop::sample::select(own).prop_map(str::to_string),
    ];
    if hazard {
        prop_oneof![
            plain,
            prop::sample::select(HAZARD_TITLES).prop_map(str::to_string),
            lines(true),
        ]
        .boxed()
    } else {
        plain.boxed()
    }
}

fn prose(text: BoxedStrategy<String>, site: TextSite) -> impl Strategy<Value = Prose> {
    text.prop_filter_map("blank text is no prose", move |t| {
        Prose::parse(&t, site).ok()
    })
}

#[derive(Debug, Clone)]
struct Shape {
    keys: Vec<&'static str>,
    labels: Vec<String>,
    bounds: (u8, u8),
    written_choose: Option<String>,
    hazard: bool,
}

fn shape(hazard: bool) -> impl Strategy<Value = Shape> {
    prop::sample::subsequence(KEYS, 1..=KEYS.len())
        .prop_shuffle()
        .prop_flat_map(move |keys| {
            let n = keys.len() as u8;
            let labels = prop::collection::vec(title(hazard, Headline::Label), keys.len());
            let bounds = (1..=n).prop_flat_map(|max| (0..=max, Just(max)));
            (Just(keys), labels, bounds, 0..3u8)
        })
        .prop_map(move |(keys, labels, (min, max), form)| {
            let written_choose = match form {
                0 if (min, max) == (1, 1) => None,
                1 if min == max => Some(max.to_string()),
                _ => Some(format!("{min}..{max}")),
            };
            Shape {
                keys,
                labels,
                bounds: (min, max),
                written_choose,
                hazard,
            }
        })
}

fn subset(keys: &[&'static str], (min, max): (u8, u8)) -> impl Strategy<Value = Vec<String>> {
    let owned: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    (usize::from(min)..=usize::from(max))
        .prop_flat_map(move |n| prop::sample::subsequence(owned.clone(), n))
        .prop_shuffle()
}

fn keys_of(raw: Vec<String>) -> Vec<OptionKey> {
    raw.iter()
        .map(|k| OptionKey::parse(k).expect("key"))
        .collect()
}

/// Which commands a decision in some status accepts.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Any,
    Open,
    Decided,
    Withdrawn,
}

impl Phase {
    fn of(status: &Status) -> Self {
        match status {
            Status::Open => Phase::Open,
            Status::Decided(_) => Phase::Decided,
            Status::Withdrawn { .. } => Phase::Withdrawn,
        }
    }
}

fn command(shape: &Shape) -> BoxedStrategy<Command> {
    command_in(shape, Phase::Any)
}

/// A command `apply` accepts for a decision of this shape in `phase`.
/// `Phase::Any` draws commands without regard to status or decider rights.
fn command_in(shape: &Shape, phase: Phase) -> BoxedStrategy<Command> {
    let keys = shape.keys.clone();
    let n = keys.len();
    let bounds = shape.bounds;
    let mut pool = ANSWERERS.to_vec();
    if shape.hazard {
        pool.extend(HAZARD_ANSWERERS);
    }
    let answerer = prop::sample::select(pool).prop_map(|a| Answerer::parse(a).unwrap());
    let decider_pool = match phase {
        Phase::Any => answerer.clone().boxed(),
        _ => prop::sample::select(DECIDERS)
            .prop_map(|a| Answerer::parse(a).unwrap())
            .boxed(),
    };
    let note_text = if shape.hazard {
        lines(true)
    } else {
        words("[A-Z][a-z]{1,8}").boxed()
    };
    let rationale = prop::option::of(prose(lines(shape.hazard), TextSite::Rationale));
    let note = prop::option::of(prose(note_text, TextSite::Note));
    let probabilities = move |cap: u32| {
        prop::sample::subsequence(keys.clone(), 0..=n)
            .prop_flat_map(move |ks| {
                let len = ks.len();
                (Just(ks), prop::collection::vec(0..=cap, len))
            })
            .prop_map(|(ks, ps)| {
                ks.into_iter()
                    .zip(ps)
                    .map(|(k, p)| (OptionKey::parse(k).unwrap(), f64::from(p) / 300.0))
                    .collect::<Vec<_>>()
            })
    };
    let per_key_cap = 300 / n as u32;
    let pick = subset(&shape.keys, bounds).prop_map(|k| AnswerInput::Pick(keys_of(k)));
    let marginals = probabilities(300).prop_map(AnswerInput::Marginals);
    let body = if bounds.1 == 1 {
        let categorical = probabilities(per_key_cap).prop_map(AnswerInput::Categorical);
        prop_oneof![pick, categorical, marginals].boxed()
    } else {
        prop_oneof![pick, marginals].boxed()
    };
    let answer =
        (answerer.clone(), body, rationale).prop_map(|(by, body, rationale)| Command::Answer {
            by,
            body,
            rationale,
        });
    let decide =
        (subset(&shape.keys, bounds), decider_pool, note).prop_map(|(chosen, decider, note)| {
            Command::Decide {
                chosen: keys_of(chosen),
                decider,
                note,
            }
        });
    let withdraw = answerer.prop_map(|by| Command::Withdraw { by });
    match phase {
        Phase::Any | Phase::Open => prop_oneof![3 => answer, 2 => decide, 1 => withdraw].boxed(),
        Phase::Decided => prop_oneof![3 => answer, 2 => decide].boxed(),
        Phase::Withdrawn => answer.boxed(),
    }
}

/// A legal decision: asked, then the legal ones of a few random commands.
/// With `hazard`, its texts include what the block tree cannot hold.
fn decision_with(hazard: bool) -> impl Strategy<Value = (Decision, Shape)> {
    shape(hazard)
        .prop_flat_map(|shape| {
            let recommend = prop::option::of(subset(&shape.keys, shape.bounds));
            let supersedes = prop::option::of(prop::sample::select(vec![
                "block:sharing-7",
                "github-issue:owner/repo/7",
            ]));
            let commands = prop::collection::vec((command(&shape), 0..1000u32), 0..8);
            (
                title(shape.hazard, Headline::Question),
                Just(shape),
                recommend,
                supersedes,
                commands,
            )
        })
        .prop_map(|(question, shape, recommend, supersedes, commands)| {
            let q = DraftQuestion {
                question,
                options: shape
                    .keys
                    .iter()
                    .zip(&shape.labels)
                    .map(|(k, l)| (k.to_string(), l.clone()))
                    .collect(),
                choose: shape.written_choose.clone(),
                recommend,
                supersedes: supersedes.map(str::to_string),
            };
            let id = DecisionRef::parse(&format!("block:{DECISION_ID}")).unwrap();
            let mut d = Decision::ask(id, DECISION_ID.to_string(), q).expect("a legal question");
            for (step, (c, millis)) in commands.into_iter().enumerate() {
                if let Ok(change) = d.apply(c, instant(step as u32, millis), &allowed()) {
                    d = d.commit(change).expect("fresh change");
                }
            }
            (d, shape)
        })
}

fn decision() -> impl Strategy<Value = (Decision, Shape)> {
    any::<bool>().prop_flat_map(decision_with)
}

fn try_stored(d: &Decision) -> Result<Vault, BlockDecisionError> {
    let mut vault = Vault::new();
    let topic = vault.topic();
    let mut minted = 0;
    let ops = decision_block::ask_ops(d, &topic, None, || {
        minted += 1;
        EntityUri::block(&format!("{DECISION_ID}-k{minted}"))
    })?;
    vault.apply(&ops);
    vault.round_trip_org();
    Ok(vault)
}

fn stored(d: &Decision) -> Vault {
    try_stored(d).expect("a decision of plain text encodes")
}

/// Every text a decision holds that the block tree writes verbatim.
fn texts_of(d: &Decision) -> Vec<String> {
    let mut texts = vec![d.question().to_string()];
    texts.extend(d.options().iter().map(|o| o.label.clone()));
    match d.status() {
        Status::Open => {}
        Status::Decided(r) => {
            texts.push(r.decider().to_string());
            texts.extend(r.note().map(Prose::to_string));
        }
        Status::Withdrawn { by, .. } => texts.push(by.to_string()),
    }
    for a in d.answers() {
        texts.push(a.by().to_string());
        texts.extend(a.rationale().map(Prose::to_string));
    }
    texts
}

/// A refusal is legal for a decision with hazardous text, and for one with a
/// rationale org does not give back as written; it names a text of the
/// decision or one line of it.
fn check_refusal(e: &BlockDecisionError, d: &Decision, hazard: bool) -> Result<(), TestCaseError> {
    let BlockDecisionError::Unencodable { text, .. } = e else {
        return Err(TestCaseError::fail(format!(
            "not an Unencodable refusal: {e}"
        )));
    };
    let org_changes_a_rationale = || {
        d.answers()
            .iter()
            .filter_map(|a| a.rationale())
            .any(|r| !org_keeps(r.as_str()))
    };
    prop_assert!(
        hazard || org_changes_a_rationale(),
        "a decision org keeps as written was refused: {}",
        e
    );
    prop_assert!(
        texts_of(d).iter().any(|t| t.contains(text.as_str())),
        "the refusal names {:?}, no text of the decision",
        text
    );
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum OddAnswer {
    NoAnswered,
    TwoBodies,
    NoAnswerer,
}

/// The headline re-reads the question's own first word as a keyword, or its
/// trailing `:y:` as a tag group, so the breach would not read as its name.
fn is_masked(breach: Breach, d: &Decision) -> bool {
    let first_word = d.question().split(' ').next().unwrap_or_default();
    match breach {
        Breach::NoKeyword => PLAIN_QUESTIONS.iter().any(|q| q.starts_with(first_word)),
        Breach::NoTag => holon_api::types::Tags::org_tag_group_start(d.question()).is_some(),
        _ => false,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: ProptestConfig::default().cases.max(512),
        max_global_rejects: 100_000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_rationale_reads_back_equal_or_is_refused_naming_a_changed_line(
        rationale in nested_rationale()
    ) {
        equal_or_refused(&rationale)?;
    }

    #[test]
    fn a_stored_decision_reads_back_equal((d, shape) in decision()) {
        match try_stored(&d) {
            Ok(vault) => prop_assert_eq!(vault.read(), Ok(d)),
            Err(e) => check_refusal(&e, &d, shape.hazard)?,
        }
    }

    #[test]
    fn a_change_reads_back_as_the_committed_decision(
        (d, shape, c) in (decision_with(false), any::<bool>())
            .prop_flat_map(|((d, mut shape), hazard)| {
                shape.hazard = hazard;
                let c = command_in(&shape, Phase::of(d.status()));
                (Just(d), Just(shape), c)
            }),
        millis in 0..1000u32,
    ) {
        let change = d
            .apply(c, instant(99, millis), &allowed())
            .expect("a command drawn for the decision's status applies");
        let mut vault = stored(&d);
        let (_, children) = vault.decision();
        let mint = vault.mint();
        let next = d.commit(change.clone()).unwrap();
        match decision_block::change_ops(&change, &children, || mint) {
            Ok(ops) => {
                vault.apply(&ops);
                vault.round_trip_org();
                prop_assert_eq!(vault.read(), Ok(next));
            }
            Err(e) => check_refusal(&e, &next, shape.hazard)?,
        }
    }

    #[test]
    fn an_illegal_subtree_reads_as_its_named_error(
        (d, _shape) in decision_with(false),
        breach in prop::sample::select(Breach::ALL),
    ) {
        prop_assume!(!is_masked(breach, &d));
        let mut vault = stored(&d);
        breach.apply(&mut vault);
        let got = vault.read();
        prop_assert!(breach.expects(&got), "{:?} read as {:?}", breach, got);
    }

    #[test]
    fn an_unreadable_answer_is_refused_and_disclosed(
        (d, _shape) in decision_with(false),
        odd in prop::sample::select(vec![
            OddAnswer::NoAnswered,
            OddAnswer::TwoBodies,
            OddAnswer::NoAnswerer,
        ]),
    ) {
        let mut vault = stored(&d);
        let (_, children) = vault.decision();
        let id = vault.mint();
        let mut block = Block::new_text(id.clone(), vault.decision_id(), "Odd answer".to_string());
        let mut set = |k: &str, v: &str| block.set_property(k, Value::String(v.into()));
        match odd {
            OddAnswer::NoAnswered => set("answerer", "person:eve"),
            OddAnswer::TwoBodies => {
                set("answerer", "person:eve");
                set("answered", DONE_TIME);
                set("pick", KEYS[0]);
                set("p", &format!("{}=0.5", KEYS[0]));
            }
            OddAnswer::NoAnswerer => {
                set("answered", DONE_TIME);
                set("pick", KEYS[0]);
            }
        }
        insert_last_child(&mut vault, &children, block);
        vault.round_trip_org();
        let read = vault.read();
        prop_assert!(read.is_ok(), "{:?}", read);
        let read = read.unwrap();
        prop_assert_eq!(read.answers(), d.answers());
        prop_assert_eq!(read.refused().len(), 1, "{:?} was not disclosed", odd);
        prop_assert_eq!(&read.refused()[0].item, &id.to_string());
        prop_assert!(!read.refused()[0].reason.is_empty());
    }
}

/// A store may type a property value; org ingest never does, so this skips
/// the org trip.
#[test]
fn a_typed_property_value_is_a_named_error() {
    let d = Decision::ask(
        DecisionRef::parse(&format!("block:{DECISION_ID}")).unwrap(),
        DECISION_ID.to_string(),
        DraftQuestion {
            question: "Which?".into(),
            options: vec![("a".into(), "A".into()), ("b".into(), "B".into())],
            choose: None,
            recommend: None,
            supersedes: None,
        },
    )
    .unwrap();
    let mut vault = stored(&d);
    let id = vault.decision_id();
    vault
        .block_mut(&id)
        .set_property("choose", Value::Integer(2));
    let got = vault.read();
    assert!(
        matches!(got, Err(BlockDecisionError::NotText { key: "choose", .. })),
        "{got:?}"
    );
}

fn insert_last_child(vault: &mut Vault, children: &[Block], block: Block) {
    let at = match children.last() {
        Some(last) => vault.blocks.iter().position(|b| b.id == last.id).unwrap() + 1,
        None => {
            let parent = &block.parent_id;
            vault.blocks.iter().position(|b| &b.id == parent).unwrap() + 1
        }
    };
    vault.blocks.insert(at, block);
}

const DONE_TIME: &str = "2026-09-25T10:12:31Z";

#[derive(Debug, Clone, Copy)]
enum Breach {
    NoTag,
    NoKeyword,
    TodoKeyword,
    OptionAndAnswer,
    AnswerKeysOnOption,
    RulingOnOpen,
    WithdrawalOnDecided,
    NoOptions,
    DuplicateOptionKey,
    ChosenNamesNoOption,
    ChooseExceedsOptions,
    DoneWithoutRuling,
    CancelledWithoutWithdrawal,
    ProbabilityOutOfRange,
    SupersedesItself,
    ModelDecides,
}

impl Breach {
    const ALL: &[Breach] = &[
        Breach::NoTag,
        Breach::NoKeyword,
        Breach::TodoKeyword,
        Breach::OptionAndAnswer,
        Breach::AnswerKeysOnOption,
        Breach::RulingOnOpen,
        Breach::WithdrawalOnDecided,
        Breach::NoOptions,
        Breach::DuplicateOptionKey,
        Breach::ChosenNamesNoOption,
        Breach::ChooseExceedsOptions,
        Breach::DoneWithoutRuling,
        Breach::CancelledWithoutWithdrawal,
        Breach::ProbabilityOutOfRange,
        Breach::SupersedesItself,
        Breach::ModelDecides,
    ];

    fn apply(self, vault: &mut Vault) {
        let id = vault.decision_id();
        let (_, children) = vault.decision();
        let options: Vec<EntityUri> = children
            .iter()
            .filter(|c| c.get_property_str("option").is_some())
            .map(|c| c.id.clone())
            .collect();
        let first_key = vault.block(&options[0]).get_property_str("option").unwrap();
        let set = |vault: &mut Vault, key: &str, value: &str| {
            vault
                .block_mut(&id)
                .set_property(key, Value::String(value.to_string()))
        };
        let clear = |vault: &mut Vault, keys: &[&str]| {
            let block = vault.block_mut(&id);
            for k in keys {
                block.properties.remove(*k);
            }
        };
        let keyword = |vault: &mut Vault, k: Option<&str>| {
            vault
                .block_mut(&id)
                .set_task_state(k.map(TaskState::from_keyword))
        };
        const RULING: &[&str] = &["chosen", "decider", "decided", "note"];
        const WITHDRAWAL: &[&str] = &["withdrawer", "withdrawn"];
        match self {
            Breach::NoTag => {
                vault.block_mut(&id).tags.remove("decision");
            }
            Breach::NoKeyword => keyword(vault, None),
            Breach::TodoKeyword => keyword(vault, Some("TODO")),
            Breach::OptionAndAnswer => vault
                .block_mut(&options[0])
                .set_property("answerer", Value::String("person:eve".into())),
            Breach::AnswerKeysOnOption => vault
                .block_mut(&options[0])
                .set_property("answered", Value::String(DONE_TIME.into())),
            Breach::RulingOnOpen => {
                keyword(vault, Some("?"));
                clear(vault, RULING);
                clear(vault, WITHDRAWAL);
                set(vault, "chosen", &first_key);
            }
            Breach::WithdrawalOnDecided => {
                keyword(vault, Some("DONE"));
                clear(vault, WITHDRAWAL);
                set(vault, "withdrawer", "person:eve");
            }
            Breach::NoOptions => {
                for o in &options {
                    vault.block_mut(o).properties.remove("option");
                }
            }
            Breach::DuplicateOptionKey => {
                let twin_id = vault.mint();
                let mut twin = Block::new_text(twin_id, id.clone(), "Twin".to_string());
                twin.set_property("option", Value::String(first_key.clone()));
                insert_last_child(vault, &children, twin);
            }
            Breach::ChosenNamesNoOption => {
                keyword(vault, Some("DONE"));
                clear(vault, WITHDRAWAL);
                set(vault, "chosen", "zz");
                set(vault, "decider", "person:martin");
                set(vault, "decided", DONE_TIME);
            }
            Breach::ChooseExceedsOptions => set(vault, "choose", "9"),
            Breach::DoneWithoutRuling => {
                keyword(vault, Some("DONE"));
                clear(vault, RULING);
                clear(vault, WITHDRAWAL);
            }
            Breach::CancelledWithoutWithdrawal => {
                keyword(vault, Some("CANCELLED"));
                clear(vault, RULING);
                clear(vault, WITHDRAWAL);
            }
            Breach::ProbabilityOutOfRange => {
                let answer_id = vault.mint();
                let mut answer = Block::new_text(answer_id, id.clone(), "Sure".to_string());
                answer.set_property("answerer", Value::String("model:jev-1".into()));
                answer.set_property("answered", Value::String(DONE_TIME.into()));
                answer.set_property("p", Value::String(format!("{first_key}=1.5")));
                insert_last_child(vault, &children, answer);
            }
            Breach::SupersedesItself => set(vault, "supersedes", DECISION_ID),
            Breach::ModelDecides => {
                keyword(vault, Some("DONE"));
                clear(vault, WITHDRAWAL);
                clear(vault, &["recommend"]);
                set(vault, "choose", "1");
                set(vault, "chosen", &first_key);
                set(vault, "decider", "model:jev-1");
                set(vault, "decided", DONE_TIME);
            }
        }
        vault.round_trip_org();
    }

    fn expects(self, got: &Result<Decision, BlockDecisionError>) -> bool {
        use BlockDecisionError as B;
        use DecisionError as D;
        let Err(e) = got else { return false };
        match self {
            Breach::NoTag => matches!(e, B::NotTagged { .. }),
            Breach::NoKeyword | Breach::TodoKeyword => matches!(e, B::UnknownKeyword { .. }),
            Breach::OptionAndAnswer => matches!(e, B::OptionAndAnswer { .. }),
            Breach::AnswerKeysOnOption => matches!(e, B::AnswerKeysOnOption { .. }),
            Breach::RulingOnOpen | Breach::WithdrawalOnDecided => {
                matches!(e, B::StrayKey { .. })
            }
            Breach::NoOptions => matches!(e, B::Core(D::NoOptions)),
            Breach::DuplicateOptionKey => matches!(e, B::Core(D::DuplicateOptionKey(_))),
            Breach::ChosenNamesNoOption => matches!(e, B::Core(D::UnknownOptionKey { .. })),
            Breach::ChooseExceedsOptions => {
                matches!(e, B::Core(D::CardinalityExceedsOptions { .. }))
            }
            Breach::DoneWithoutRuling => matches!(e, B::Core(D::IncompleteRuling(_))),
            Breach::CancelledWithoutWithdrawal => {
                matches!(e, B::Core(D::IncompleteWithdrawal(_)))
            }
            Breach::ProbabilityOutOfRange => {
                matches!(e, B::Core(core) if core.rule() == Rule::Dc5)
            }
            Breach::SupersedesItself => matches!(e, B::Core(D::SupersedesItself(_))),
            Breach::ModelDecides => matches!(e, B::Core(D::ModelCannotDecide(_))),
        }
    }
}

fn open_decision() -> Decision {
    Decision::ask(
        DecisionRef::parse(&format!("block:{DECISION_ID}")).unwrap(),
        DECISION_ID.to_string(),
        DraftQuestion {
            question: "Which?".into(),
            options: vec![("a".into(), "A".into())],
            choose: None,
            recommend: None,
            supersedes: None,
        },
    )
    .unwrap()
}

fn answer_change(d: &Decision, rationale: &str) -> Change {
    d.apply(
        Command::Answer {
            by: Answerer::parse("person:eve").unwrap(),
            body: AnswerInput::Pick(keys_of(vec!["a".into()])),
            rationale: Some(Prose::parse(rationale, TextSite::Rationale).unwrap()),
        },
        instant(1, 0),
        &allowed(),
    )
    .unwrap()
}

/// The operations that store `rationale` as an answer with no org check in
/// the way: the adapter's operations for a plain rationale, with the answer
/// text swapped in.
fn unchecked_answer_ops(rationale: &str) -> (Vault, Vec<Operation>, Decision) {
    let d = open_decision();
    let mut vault = stored(&d);
    let (_, children) = vault.decision();
    let mint = vault.mint();
    let mut ops = decision_block::change_ops(&answer_change(&d, "x"), &children, || mint)
        .expect("a plain rationale encodes");
    assert_eq!(ops.len(), 1, "an answer is stored by one create: {ops:?}");
    ops[0].params.insert(
        "content".to_string(),
        Value::String(format!("{ANSWER_TITLE}\n{rationale}")),
    );
    let next = d.commit(answer_change(&d, rationale)).unwrap();
    (vault, ops, next)
}

fn answered(rationale: &str) -> Result<Decision, BlockDecisionError> {
    let d = open_decision();
    let change = answer_change(&d, rationale);
    let mut vault = stored(&d);
    let (_, children) = vault.decision();
    let mint = vault.mint();
    let ops = decision_block::change_ops(&change, &children, || mint)?;
    let next = d.commit(change).unwrap();
    vault.apply(&ops);
    vault.round_trip_org();
    assert_eq!(vault.read(), Ok(next.clone()));
    Ok(next)
}

/// Whether org gives the stored answer back unchanged with no refusal in the
/// way: the ground truth the refusal must agree with.
fn org_keeps(rationale: &str) -> bool {
    let (mut vault, ops, next) = unchecked_answer_ops(rationale);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vault.apply(&ops);
        vault.round_trip_org();
        vault.read() == Ok(next)
    }))
    .unwrap_or(false)
}

const CONTAINER_NAMES: &[&str] = &[
    "verse", "comment", "center", "quote", "src", "example", "export", "special", "foo", "QUOTE",
    "Src",
];
const DRAWER_NAMES: &[&str] = &["LOGBOOK", "PROPERTIES", "end", "log1", "a-b"];
const KEYWORD_KEYS: &[&str] = &[
    "begin",
    "end",
    "BEGINX",
    "endx",
    "Begin_",
    "END_",
    "begin_src",
    "end_quote",
    ":",
    "TITLE",
];

fn one_of(items: &'static [&'static str]) -> impl Strategy<Value = &'static str> + Clone {
    prop::sample::select(items)
}

/// `#+KEY:` lines with any key spelling, including the ones that start like a
/// block delimiter.
fn keyword_line() -> BoxedStrategy<String> {
    let key = prop_oneof![
        "[A-Za-z0-9_-]{1,6}".prop_map(String::from),
        one_of(KEYWORD_KEYS).prop_map(String::from),
    ];
    (one_of(&["", "  ", "\t"]), key, one_of(&["", " x"]))
        .prop_map(|(indent, key, value)| format!("{indent}#+{key}:{value}"))
        .boxed()
}

/// Lines of text with drawers and blocks of every kind nested in each other,
/// closed by a delimiter that matches in case or not.
fn nested_lines() -> BoxedStrategy<Vec<String>> {
    let leaf = prop_oneof![
        keyword_line().prop_map(|l| vec![l]),
        prop::sample::select(PLAIN_LINES).prop_map(|l| vec![l.to_string()]),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        let inner = prop::collection::vec(inner, 0..4).prop_map(|parts| parts.concat());
        prop_oneof![
            (
                one_of(CONTAINER_NAMES),
                one_of(&["", " x"]),
                prop::bool::ANY,
                inner.clone()
            )
                .prop_map(|(name, arg, upper, inner)| {
                    let closer = if upper {
                        name.to_uppercase()
                    } else {
                        name.to_string()
                    };
                    [
                        vec![format!("#+begin_{name}{arg}")],
                        inner,
                        vec![format!("#+end_{closer}")],
                    ]
                    .concat()
                }),
            inner.clone().prop_map(|inner| {
                [
                    vec!["#+begin: x".to_string()],
                    inner,
                    vec!["#+end:".to_string()],
                ]
                .concat()
            }),
            (one_of(DRAWER_NAMES), inner).prop_map(|(name, inner)| {
                [vec![format!(":{name}:")], inner, vec![":END:".to_string()]].concat()
            }),
        ]
    })
    .boxed()
}

fn nested_rationale() -> impl Strategy<Value = String> {
    prop::collection::vec(nested_lines(), 1..4).prop_map(|parts| parts.concat().join("\n"))
}

/// A write reads back equal, or is refused naming a line of the text that org
/// changes; it is never accepted and changed, and never refused while org
/// keeps it.
fn equal_or_refused(rationale: &str) -> Result<(), TestCaseError> {
    let kept = org_keeps(rationale);
    match std::panic::catch_unwind(|| answered(rationale)) {
        Ok(Ok(_)) => prop_assert!(kept),
        Ok(Err(BlockDecisionError::Unencodable { text, .. })) => {
            prop_assert!(
                rationale.split('\n').any(|l| l == text),
                "{rationale:?}: the refusal names {text:?}, no line of it"
            );
            prop_assert!(
                !kept,
                "{rationale:?}: refused naming {text:?}, but org keeps it"
            );
        }
        Ok(Err(e)) => prop_assert!(false, "{rationale:?}: {e}"),
        Err(_) => prop_assert!(false, "{rationale:?}: accepted, and org changes it"),
    }
    Ok(())
}

/// Fixed cases a generator found, kept as explicit state.
#[test]
fn fixed_rationales_read_back_equal_or_are_refused() {
    for rationale in [
        ":LOGBOOK:\n#+BEGINX: x\n:END:",
        ":LOGBOOK:\n#+ENDX: x\n:END:",
        "#+begin_src\n:LOGBOOK:\n#+TITLE: x\n:END:\n#+end_src",
        "#+begin_verse\n#+TITLE: x\n#+end_verse",
        "#+begin_comment\n#+TITLE: x\n#+end_comment",
        ":PROPERTIES:\n#+TITLE: x\n:END:",
        "#+begin_quote\n#+TITLE: x\n#+end_quote",
        "#+begin_example\n#+TITLE: x\n#+end_example",
    ] {
        equal_or_refused(rationale).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn a_rationale_keeps_drawer_lines_indentation_and_inner_blank_lines() {
    answered("Because\n:PROPERTIES:\n:ID: x\n:END:\n\n  indented").expect("encodable");
}

/// Fixed case: org lifts a keyword line out of a drawer or block, so the
/// rationale is refused and the refusal names a line of it.
#[test]
fn a_keyword_line_inside_a_drawer_or_block_is_refused_naming_a_line() {
    for rationale in [
        ":PROPERTIES:\n#+TITLE: x\n:END:",
        ":LOGBOOK:\n  #+TITLE: x\n:END:",
        "#+begin_quote\n#+TITLE: x\n#+end_quote",
    ] {
        match answered(rationale) {
            Err(BlockDecisionError::Unencodable { text, .. }) => {
                assert!(
                    rationale.split('\n').any(|l| l == text),
                    "{rationale:?}: {text:?}"
                )
            }
            other => panic!("{rationale:?}: {other:?}"),
        }
    }
}

/// Outside a drawer or block, and inside a source block, such a line is text.
#[test]
fn a_keyword_line_outside_a_drawer_or_in_a_source_block_reads_back_equal() {
    for rationale in [
        "#+TITLE: x",
        "a\n#+TITLE: x\n:END:",
        ":PROPERTIES:\n#+TITLE: x",
        "#+begin_src\n#+TITLE: x\n#+end_src",
    ] {
        answered(rationale).expect("encodable");
    }
}

#[test]
fn a_rationale_with_heading_and_keyword_lines_round_trips() {
    answered("Because\n* not a heading\n#+TITLE: nor a title\n#+begin_src\nfn\n#+end_src")
        .expect("encodable");
}

#[test]
fn a_multi_line_note_is_refused() {
    let d = open_decision();
    let change = d
        .apply(
            Command::Decide {
                chosen: keys_of(vec!["a".into()]),
                decider: Answerer::parse("person:martin").unwrap(),
                note: Some(Prose::parse("line one\n* Evil heading", TextSite::Note).unwrap()),
            },
            instant(1, 0),
            &allowed(),
        )
        .unwrap();
    let vault = stored(&d);
    let (_, children) = vault.decision();
    let got = decision_block::change_ops(&change, &children, || unreachable!());
    assert!(
        matches!(
            got,
            Err(BlockDecisionError::Unencodable { field: "note", .. })
        ),
        "{got:?}"
    );
}

fn asking(question: &str) -> Decision {
    Decision::ask(
        DecisionRef::parse(&format!("block:{DECISION_ID}")).unwrap(),
        DECISION_ID.to_string(),
        DraftQuestion {
            question: question.into(),
            options: vec![("a".into(), "Pick ::".into())],
            choose: None,
            recommend: None,
            supersedes: None,
        },
    )
    .unwrap()
}

#[test]
fn a_question_with_a_priority_cookie_is_refused() {
    let got = try_stored(&asking("[#A] which one?"));
    assert!(
        matches!(
            &got,
            Err(BlockDecisionError::Unencodable {
                field: "question",
                ..
            })
        ),
        "{:?}",
        got.map(|v| v.read())
    );
}

/// The decision headline ends in its `:decision:` tag, which is the tag group
/// org reads, so a question that ends in colons keeps them.
#[test]
fn a_question_ending_in_colons_survives_org() {
    let mut vault = stored(&asking("Pick ::"));
    let id = vault.decision_id();
    vault.block_mut(&id).content = "Pick ::".into();
    let text = render(&vault.document, &vault.blocks);
    let (_, blocks) = parse(&text);
    let question = blocks
        .iter()
        .find(|b| b.id == id)
        .map(|b| b.content.clone());
    assert_eq!(question.as_deref(), Some("Pick ::"), "{text}");
}

#[test]
fn an_option_label_ending_in_a_tab_separated_tag_group_is_refused() {
    let d = Decision::ask(
        DecisionRef::parse(&format!("block:{DECISION_ID}")).unwrap(),
        DECISION_ID.to_string(),
        DraftQuestion {
            question: "Which one?".into(),
            options: vec![("a".into(), "x\t:a:".into())],
            choose: None,
            recommend: None,
            supersedes: None,
        },
    )
    .unwrap();
    let got = try_stored(&d);
    assert!(
        matches!(
            &got,
            Err(BlockDecisionError::Unencodable {
                field: "option label",
                ..
            })
        ),
        "{:?}",
        got.map(|v| v.read())
    );
}

/// `is_masked` is the one remaining filter. Its rejections stay below this
/// share of draws, so at least 95% of the cases still run.
const MAX_MASKED_SHARE: f64 = 0.05;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1))]

    #[test]
    fn few_breach_draws_are_masked(seed in any::<u64>()) {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
        let mut rng_seed = [0u8; 32];
        rng_seed[..8].copy_from_slice(&seed.to_le_bytes());
        let mut runner = TestRunner::new_with_rng(
            Config::default(),
            TestRng::from_seed(RngAlgorithm::ChaCha, &rng_seed),
        );
        let strategy = (decision_with(false), prop::sample::select(Breach::ALL));
        let n = 5_000;
        let masked = (0..n)
            .filter(|_| {
                let ((d, _), breach) = strategy.new_tree(&mut runner).unwrap().current();
                is_masked(breach, &d)
            })
            .count();
        prop_assert!(
            (masked as f64) < MAX_MASKED_SHARE * f64::from(n),
            "{masked} of {n} breach draws are masked"
        );
    }
}

/// Fixed case of the state the random draws reach only sometimes: a withdrawn
/// decision, whose headline keyword is CANCELLED and whose withdrawer and
/// withdrawn drawer keys carry an agent and a timestamp. `stored` asserts
/// that its org render is a fixed point.
#[test]
fn a_withdrawn_decision_stores_as_a_fixed_point_and_reads_back_equal() {
    for question in ["Jansg", "Eansg", "Cansg"] {
        for millis in [0, 7, 120, 999] {
            let d = asking(question);
            let change = d
                .apply(
                    Command::Withdraw {
                        by: Answerer::parse("agent:orch").unwrap(),
                    },
                    instant(3, millis),
                    &allowed(),
                )
                .unwrap();
            let withdrawn = d.commit(change).unwrap();
            assert!(matches!(withdrawn.status(), Status::Withdrawn { .. }));
            assert_eq!(stored(&withdrawn).read(), Ok(withdrawn));
        }
    }
}

/// The generators must keep drawing the withdrawn state, or a green run says
/// nothing about it. Fixed seed: this measures the strategy, not the code.
#[test]
fn the_generator_draws_withdrawn_decisions() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::Config;
    use proptest::test_runner::RngAlgorithm;
    use proptest::test_runner::TestRng;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7u8; 32]),
    );
    let strategy = decision_with(false);
    let withdrawn = (0..2_000)
        .filter(|_| {
            let (d, _) = strategy.new_tree(&mut runner).unwrap().current();
            matches!(d.status(), Status::Withdrawn { .. })
        })
        .count();
    assert!(
        withdrawn >= 100,
        "only {withdrawn} of 2000 draws are withdrawn"
    );
}

/// The adapter's own write path, as the stored-decision and change properties
/// drive it: a write reads back equal, or is refused naming a line.
fn change_path_equal_or_refused(rationale: &str) -> Result<(), TestCaseError> {
    let d = open_decision();
    let change = answer_change(&d, rationale);
    let mut vault = stored(&d);
    let (_, children) = vault.decision();
    let mint = vault.mint();
    let next = d.commit(change.clone()).unwrap();
    match decision_block::change_ops(&change, &children, || mint) {
        Ok(ops) => {
            let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                vault.apply(&ops);
                vault.round_trip_org();
                vault.read()
            }));
            prop_assert!(
                matches!(&read, Ok(Ok(r)) if *r == next),
                "{rationale:?}: change_ops accepted it, and org changes it"
            );
        }
        Err(BlockDecisionError::Unencodable { text, .. }) => prop_assert!(
            rationale.split('\n').any(|l| l == text),
            "{rationale:?}: the refusal names {text:?}, no line of it"
        ),
        Err(e) => prop_assert!(false, "{rationale:?}: {e}"),
    }
    Ok(())
}

proptest! {
    #[test]
    fn a_change_of_any_rationale_reads_back_or_is_refused(rationale in nested_rationale()) {
        for fixed in [":PROPERTIES:\n#+TITLE: x\n:END:", ":LOGBOOK:\n#+BEGINX: x\n:END:"] {
            change_path_equal_or_refused(fixed)?;
        }
        change_path_equal_or_refused(&rationale)?;
    }
}
