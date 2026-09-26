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
use holon_api::decision::Command;
use holon_api::decision::Decider;
use holon_api::decision::Decision;
use holon_api::decision::DecisionError;
use holon_api::decision::DecisionRef;
use holon_api::decision::DraftQuestion;
use holon_api::decision::OptionKey;
use holon_api::decision::Rule;
use holon_api::decision_block;
use holon_api::decision_block::BlockDecisionError;
use holon_api::types::TaskState;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;
use proptest::prelude::*;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/decisions.org";
const SKELETON: &str = "#+ID: decisions\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
const DECISION_ID: &str = "dq-1";

const KEYS: &[&str] = &["a", "b", "c", "rename-2"];
const ANSWERERS: &[&str] = &["person:martin", "person:eve", "agent:orch", "model:jev-1"];
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
        decision_block::parse(&block, &children)
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
}

fn words(first: &'static str) -> impl Strategy<Value = String> + Clone {
    (first, "( [a-z]{1,8}){0,3}").prop_map(|(a, b)| format!("{a}{b}"))
}

#[derive(Debug, Clone)]
struct Shape {
    keys: Vec<&'static str>,
    labels: Vec<String>,
    bounds: (u8, u8),
    written_choose: Option<String>,
}

fn shape() -> impl Strategy<Value = Shape> {
    prop::sample::subsequence(KEYS, 1..=KEYS.len())
        .prop_shuffle()
        .prop_flat_map(|keys| {
            let n = keys.len() as u8;
            let labels = prop::collection::vec(words("[A-Z][a-z]{1,8}"), keys.len());
            let bounds = (1..=n).prop_flat_map(|max| (0..=max, Just(max)));
            (Just(keys), labels, bounds, 0..3u8)
        })
        .prop_map(|(keys, labels, (min, max), form)| {
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

fn command(shape: &Shape) -> impl Strategy<Value = Command> {
    let keys = shape.keys.clone();
    let n = keys.len();
    let bounds = shape.bounds;
    let answerer = prop::sample::select(ANSWERERS).prop_map(|a| Answerer::parse(a).unwrap());
    let rationale = prop::option::of(words("[A-Z][a-z]{1,8}"));
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
    let body = prop_oneof![
        subset(&shape.keys, bounds).prop_map(|k| AnswerInput::Pick(keys_of(k))),
        probabilities(per_key_cap).prop_map(AnswerInput::Categorical),
        probabilities(300).prop_map(AnswerInput::Marginals),
    ];
    prop_oneof![
        3 => (answerer.clone(), body, rationale.clone()).prop_map(|(by, body, rationale)| {
            Command::Answer { by, body, rationale }
        }),
        2 => (subset(&shape.keys, bounds), answerer.clone(), rationale).prop_map(
            |(chosen, decider, note)| Command::Decide {
                chosen: keys_of(chosen),
                decider,
                note,
            }
        ),
        1 => answerer.prop_map(|by| Command::Withdraw { by }),
    ]
}

/// A legal decision: asked, then the legal ones of a few random commands.
fn decision() -> impl Strategy<Value = (Decision, Shape)> {
    shape()
        .prop_flat_map(|shape| {
            let recommend = prop::option::of(subset(&shape.keys, shape.bounds));
            let supersedes = prop::option::of(prop::sample::select(vec![
                "block:sharing-7",
                "github-issue:owner/repo/7",
            ]));
            let commands = prop::collection::vec((command(&shape), 0..1000u32), 0..6);
            (
                Just(shape),
                words("[A-Z][a-z]{1,8}"),
                recommend,
                supersedes,
                commands,
            )
        })
        .prop_map(|(shape, question, recommend, supersedes, commands)| {
            let q = DraftQuestion {
                question: format!("{question}?"),
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

fn stored(d: &Decision) -> Vault {
    let mut vault = Vault::new();
    let topic = vault.topic();
    let mut minted = 0;
    let ops = decision_block::ask_ops(d, &topic, None, || {
        minted += 1;
        EntityUri::block(&format!("{DECISION_ID}-k{minted}"))
    })
    .expect("a block decision encodes");
    vault.apply(&ops);
    vault.round_trip_org();
    vault
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn a_stored_decision_reads_back_equal((d, _shape) in decision()) {
        let vault = stored(&d);
        prop_assert_eq!(vault.read(), Ok(d));
    }

    #[test]
    fn a_change_reads_back_as_the_committed_decision(
        (d, c) in decision().prop_flat_map(|(d, shape)| (Just(d), command(&shape))),
        millis in 0..1000u32,
    ) {
        let change = d.apply(c, instant(99, millis), &allowed());
        prop_assume!(change.is_ok());
        let change = change.unwrap();
        let mut vault = stored(&d);
        let (_, children) = vault.decision();
        let mint = vault.mint();
        let ops = decision_block::change_ops(&change, &children, || mint)
            .expect("a block decision's change encodes");
        vault.apply(&ops);
        vault.round_trip_org();
        prop_assert_eq!(vault.read(), Ok(d.commit(change).unwrap()));
    }

    #[test]
    fn an_illegal_subtree_reads_as_its_named_error(
        (d, _shape) in decision(),
        breach in prop::sample::select(Breach::ALL),
    ) {
        let mut vault = stored(&d);
        breach.apply(&mut vault);
        let got = vault.read();
        prop_assert!(breach.expects(&got), "{:?} read as {:?}", breach, got);
    }

    #[test]
    fn an_unreadable_answer_is_refused_and_disclosed(
        (d, _shape) in decision(),
        answered in any::<bool>(),
    ) {
        let mut vault = stored(&d);
        let (_, children) = vault.decision();
        let id = vault.mint();
        let mut odd = Block::new_text(id.clone(), vault.decision_id(), "Odd answer".to_string());
        odd.set_property("answerer", Value::String("person:eve".into()));
        if answered {
            odd.set_property("answered", Value::String("2026-09-25T10:04:00Z".into()));
            odd.set_property("pick", Value::String(KEYS[0].into()));
            odd.set_property("p", Value::String(format!("{}=0.5", KEYS[0])));
        }
        insert_last_child(&mut vault, &children, odd);
        vault.round_trip_org();
        let read = vault.read();
        prop_assert!(read.is_ok(), "{:?}", read);
        let read = read.unwrap();
        prop_assert_eq!(read.answers(), d.answers());
        prop_assert_eq!(read.refused().len(), 1);
        prop_assert_eq!(&read.refused()[0].item, &id.to_string());
    }
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
