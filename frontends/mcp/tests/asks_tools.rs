//! The agent's side of user-facing transitions (#167 lane U): `asks_list`
//! names each open question and the exact call that answers it;
//! `execute_operation(ask, answer, ..)` parses the value at the boundary and
//! refuses an irreversible ask with the typed `needs_approval` error;
//! `answer_settled` reports the outcome, or `pending` past its timeout.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityName;
use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskHash;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::ExpireReason;
use holon_api::user_transition::InputContext;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::OccurrenceId;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_api::user_transition::UserTransitions;
use holon_core::storage::types::StorageEntity;
use holon_frontend::answer::Answering;
use holon_mcp::tools::AnswerSettledParams;
use holon_mcp::tools::answer_ask;
use holon_mcp::tools::answer_settled_text;
use holon_mcp::tools::asks_list_text;

struct FakePort {
    asks: Vec<AskRow>,
    answered: Mutex<Vec<AnswerCall>>,
    script: Mutex<VecDeque<AnswerSettlement>>,
}

impl UserTransitions for FakePort {
    fn asks(&self) -> Result<Vec<AskRow>, UftError> {
        Ok(self.asks.clone())
    }
    fn preview(&self, _: &AskRow) -> Result<String, UftError> {
        unreachable!("agents read no preview here")
    }
    fn answer(&self, call: AnswerCall) -> Result<AnswerId, UftError> {
        let id = call.answer.clone();
        self.answered.lock().unwrap().push(call);
        Ok(id)
    }
    fn settlement(&self, _: &AnswerId) -> Result<AnswerSettlement, UftError> {
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AnswerSettlement::Waiting))
    }
}

/// Entity `doc` has the one id `d1`; no newtypes, places or row keys.
struct DocIds;

impl InputContext for DocIds {
    fn parse_newtype(&self, type_name: &str, _: &str) -> Result<Value, String> {
        panic!("no newtype '{type_name}' here")
    }
    fn place_holds(&self, _: &str, _: &str) -> Result<bool, NotImplemented> {
        panic!("no place here")
    }
    fn entity_exists(&self, entity: &EntityName, id: &str) -> Result<bool, NotImplemented> {
        assert_eq!(entity.as_str(), "doc");
        Ok(id == "d1")
    }
    fn row_key_exists(&self, _: &str) -> Result<bool, NotImplemented> {
        panic!("no row key here")
    }
}

const HASH_HEX: &str = "0707070707070707070707070707070707070707070707070707070707070707";

fn ask(transition: &str, irreversible: bool) -> AskRow {
    AskRow {
        transition: TransitionName(transition.into()),
        subject: SubjectName("a.org".into()),
        hash: AskHash([7; 32]),
        input: Some(TypeHint::Enum {
            type_name: "Color".into(),
            variants: vec!["red".into(), "blue".into()],
        }),
        irreversible,
        has_preview: irreversible,
    }
}

fn rig(script: Vec<AnswerSettlement>) -> (Arc<FakePort>, Answering) {
    let port = Arc::new(FakePort {
        asks: vec![
            ask("pick", false),
            ask("archive", true),
            AskRow {
                input: Some(TypeHint::EntityId {
                    entity_name: "doc".into(),
                }),
                ..ask("assign", false)
            },
        ],
        answered: Mutex::new(Vec::new()),
        script: Mutex::new(script.into()),
    });
    let answering = Answering {
        transitions: port.clone(),
        input: Arc::new(DocIds),
    };
    (port, answering)
}

fn params(transition: &str, value: Option<&str>) -> StorageEntity {
    let mut p: StorageEntity = [
        ("transition", transition),
        ("subject", "a.org"),
        ("seen_hash", HASH_HEX),
    ]
    .into_iter()
    .map(|(k, v)| (Arc::from(k), Value::String(v.into())))
    .collect();
    if let Some(v) = value {
        p.insert(Arc::from("value"), Value::String(v.into()));
    }
    p
}

#[test]
fn asks_list_names_each_ask_and_the_call_that_answers_it() {
    let (_, answering) = rig(vec![]);
    let rows: serde_json::Value =
        serde_json::from_str(&asks_list_text(&answering).expect("asks list")).unwrap();
    let pick = &rows[0];
    assert_eq!(pick["transition"], "pick");
    assert_eq!(pick["seen_hash"], HASH_HEX);
    assert_eq!(pick["irreversible"], false);
    assert_eq!(pick["answer"]["entity_name"], "ask");
    assert_eq!(pick["answer"]["operation"], "answer");
    assert_eq!(pick["answer"]["params"]["seen_hash"], HASH_HEX);
    assert_eq!(rows[1]["irreversible"], true);
}

#[test]
fn an_agent_answers_a_reversible_ask_with_a_parsed_value() {
    let (port, answering) = rig(vec![]);
    let text = answer_ask(&answering, params("pick", Some("blue"))).expect("accepted");
    let reply: serde_json::Value = serde_json::from_str(&text).unwrap();
    let answered = port.answered.lock().unwrap().clone();
    assert_eq!(answered.len(), 1);
    assert_eq!(reply["answer_id"], answered[0].answer.0);
    assert_eq!(answered[0].value, Some(Value::String("blue".into())));
    assert_eq!(answered[0].seen, AskHash([7; 32]));
}

#[test]
fn a_value_outside_the_ask_is_refused_before_the_port() {
    let (port, answering) = rig(vec![]);
    let err = answer_ask(&answering, params("pick", Some("green"))).unwrap_err();
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(port.answered.lock().unwrap().is_empty());
}

#[test]
fn an_agent_answer_with_a_nonexistent_id_is_refused_at_the_parse() {
    let (port, answering) = rig(vec![]);
    let err = answer_ask(&answering, params("assign", Some("ghost"))).unwrap_err();
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("ghost") && err.message.contains("doc"),
        "the refusal does not name the input and the entity: {}",
        err.message
    );
    assert!(port.answered.lock().unwrap().is_empty());

    answer_ask(&answering, params("assign", Some("d1"))).expect("an existing id is accepted");
    assert_eq!(port.answered.lock().unwrap().len(), 1);
}

#[test]
fn an_irreversible_ask_needs_approval_in_the_ui() {
    let (port, answering) = rig(vec![]);
    let err = answer_ask(&answering, params("archive", Some("red"))).unwrap_err();
    assert_eq!(
        err.data.as_ref().map(|d| d["code"].clone()),
        Some(serde_json::json!("needs_approval")),
        "not the typed needs-approval error: {err:?}"
    );
    assert!(port.answered.lock().unwrap().is_empty());
}

#[tokio::test]
async fn answer_settled_reports_pending_then_the_outcome() {
    let (_, answering) = rig(vec![
        AnswerSettlement::Waiting,
        AnswerSettlement::Done {
            work: OccurrenceId::caller("w1"),
        },
        AnswerSettlement::Expired {
            reason: ExpireReason::AskGone,
        },
    ]);
    let settled = |text: String| serde_json::from_str::<serde_json::Value>(&text).unwrap();
    let call = |timeout_ms| AnswerSettledParams {
        answer_id: "a1".into(),
        timeout_ms,
    };

    let first = settled(answer_settled_text(&answering, call(0)).await);
    assert_eq!(first["state"], "pending");
    let done = settled(answer_settled_text(&answering, call(1000)).await);
    assert_eq!(done["outcome"], "done");
    let expired = settled(answer_settled_text(&answering, call(1000)).await);
    assert_eq!(expired["outcome"], "expired");
    assert_eq!(expired["text"], "The question no longer applies");
}

#[test]
fn an_unwired_app_says_so() {
    let err = asks_list_text(&Answering::unwired()).unwrap_err();
    assert!(
        err.message.contains("is not implemented yet"),
        "the unwired port is not disclosed: {}",
        err.message
    );
}
