//! Sidecar `transitions:` load into the same HL-IR the `#[transition]` macro
//! emits (#167), and a bad sidecar fails the load with a named error.

use std::time::Duration;

use holon_api::EntityName;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::*;
use holon_mcp_client::McpSidecar;
use holon_mcp_client::transition_sidecar::transitions_from_sidecar;
use proptest::prelude::*;
use serde_json::json;

const FIXTURE: &str = include_str!("fixtures/transition_sidecar.yaml");

/// The fixture's entities and tools with `transitions` replaced.
fn sidecar_with(transitions: serde_json::Value) -> String {
    let mut doc: serde_json::Value = serde_yaml::from_str(FIXTURE).unwrap();
    doc["transitions"] = transitions;
    serde_yaml::to_string(&doc).unwrap()
}

fn load_error(transition: serde_json::Value) -> String {
    let yaml = sidecar_with(json!({ "t": transition }));
    match McpSidecar::from_yaml(&yaml) {
        Ok(sidecar) => panic!(
            "the sidecar loaded as {:?}, expected a load error",
            sidecar.transition_descriptors()
        ),
        Err(e) => format!("{e:#}"),
    }
}

fn arc(kind: ArcKind, place: &str) -> HlArc {
    HlArc {
        kind,
        place: PlaceName(place.into()),
    }
}

fn call(text: &str) -> BodyDecl {
    BodyDecl::Expr(ExprSlot::new(text))
}

/// Example 1 and 2 of the lane brief, built by hand as the macro would emit
/// them for a Rust declaration of the same shape.
#[test]
fn sidecar_transitions_equal_the_hand_built_ir() {
    let expected = vec![
        TransitionDescriptor {
            name: TransitionName("assign_project".into()),
            arcs: vec![arc(ArcKind::Take, "unfiled")],
            body: call(&format!("update_task(project_id: {ANSWER_INPUT_KEY})")),
            irreversible: false,
            timers: vec![],
            guard: None,
            ask: Some(AskDecl {
                input: Some(TypeHint::EntityId {
                    entity_name: EntityName::new("project"),
                }),
                preview: None,
                persist: false,
            }),
        },
        TransitionDescriptor {
            name: TransitionName("drop_task".into()),
            arcs: vec![arc(ArcKind::Take, "inbox"), arc(ArcKind::Read, "project")],
            body: call("delete_task()"),
            irreversible: true,
            timers: vec![
                TimerDecl {
                    kind: TimerKind::Retry { max_attempts: 3 },
                    after: Duration::from_secs(30),
                },
                TimerDecl {
                    kind: TimerKind::Delay,
                    after: Duration::from_secs(3600),
                },
            ],
            guard: None,
            ask: Some(AskDecl {
                input: None,
                preview: None,
                persist: false,
            }),
        },
    ];
    assert_eq!(transitions_from_sidecar(FIXTURE).unwrap(), expected);
}

#[test]
fn a_bad_sidecar_transition_is_a_named_load_error() {
    let ok = || json!({ "manual": true, "take": ["unfiled"], "effect": { "tool": "update_task" } });
    let with = |key: &str, value: serde_json::Value| {
        let mut t = ok();
        t[key] = value;
        t
    };
    let cases = [
        (
            with("input", json!({ "type_hint": "entity_id:" })),
            "transition `t`: type hint `entity_id:` names no entity",
        ),
        (
            with("input", json!({ "type_hint": "colour" })),
            "transition `t`: unknown type hint `colour`",
        ),
        (
            with("guard", json!("project == null")),
            "transition `t`: guards come with #166; not supported yet",
        ),
        (
            with("preview", json!("find_tasks")),
            "transition `t`: a sidecar transition has no `preview` in increment 1",
        ),
        (with("colour", json!("red")), "unknown field `colour`"),
        (
            with("take", json!(["nowhere"])),
            "transition `t`: place `nowhere` is not an entity of this sidecar",
        ),
        (
            with("input", json!({ "place": "nowhere" })),
            "transition `t`: place `nowhere` is not an entity of this sidecar",
        ),
        (
            with("effect", json!({ "tool": "rename_task" })),
            "transition `t`: effect tool `rename_task` is not declared in `tools:`",
        ),
        (
            with("effect", json!({ "tool": "find_tasks" })),
            "transition `t`: effect tool `find_tasks` is `effect: read`",
        ),
        (
            with("effect", json!({ "tool": "delete_task" })),
            "transition `t`: effect tool `delete_task` is irreversible",
        ),
        (
            with(
                "effect",
                json!({ "tool": "update_task", "map": { "project_id": "subject" } }),
            ),
            "transition `t`: effect argument `project_id: subject`",
        ),
        (
            {
                let mut t = with("manual", json!(false));
                t["persist"] = json!(true);
                t
            },
            "transition `t`: `input` and `persist` need `manual: true`",
        ),
        (
            with("timers", json!([{ "kind": "retry", "after": "5s" }])),
            "transition `t`: a `retry` timer needs `max_attempts`",
        ),
        (
            with(
                "timers",
                json!([{ "kind": "run", "after": "5s", "max_attempts": 2 }]),
            ),
            "transition `t`: only a `retry` timer takes `max_attempts`",
        ),
        (
            with(
                "input",
                json!({ "enum": { "name": "Pick", "variants": [] } }),
            ),
            "transition `t`: enum `Pick` has no variants",
        ),
        (
            with("input", json!({ "newtype": "Pick", "place": "unfiled" })),
            "transition `t`: `input` takes exactly one of",
        ),
    ];
    for (transition, expected) in cases {
        let error = load_error(transition.clone());
        assert!(
            error.contains(expected),
            "{transition}: expected an error containing {expected:?}, got {error:?}"
        );
    }
}

/// What a generated transition looks like as a sidecar entry, and the IR it
/// must load as.
#[derive(Debug, Clone)]
struct Case {
    yaml: serde_json::Value,
    ir: TransitionDescriptor,
}

const PLACES: [&str; 3] = ["unfiled", "project", "inbox"];

fn place() -> impl Strategy<Value = &'static str> {
    prop::sample::select(PLACES.to_vec())
}

fn input() -> impl Strategy<Value = (serde_json::Value, TypeHint)> {
    prop_oneof![
        Just((json!({ "type_hint": "bool" }), TypeHint::Bool)),
        Just((json!({ "type_hint": "string" }), TypeHint::String)),
        Just((json!({ "type_hint": "number" }), TypeHint::Number)),
        "[a-z]{1,6}".prop_map(|e| (
            json!({ "type_hint": format!("entity_id:{e}") }),
            TypeHint::EntityId {
                entity_name: EntityName::new(e)
            }
        )),
        (
            "[A-Z][a-z]{0,5}",
            prop::collection::vec("[A-Za-z]{1,6}", 1..4)
        )
            .prop_map(|(name, variants)| (
                json!({ "enum": { "name": name, "variants": variants } }),
                TypeHint::Enum {
                    type_name: name,
                    variants
                }
            )),
        "[A-Z][a-z]{0,5}"
            .prop_map(|n| (json!({ "newtype": n }), TypeHint::Newtype { type_name: n })),
        place().prop_map(|p| (
            json!({ "place": p }),
            TypeHint::PlaceId {
                place: PlaceName(p.into())
            }
        )),
    ]
}

fn timer() -> impl Strategy<Value = (serde_json::Value, TimerDecl)> {
    let kind = prop_oneof![
        (1u32..5).prop_map(|n| (Some(n), "retry", TimerKind::Retry { max_attempts: n })),
        Just((None, "run", TimerKind::Run)),
        Just((None, "abandon", TimerKind::Abandon)),
        Just((None, "delay", TimerKind::Delay)),
    ];
    (kind, 1u64..100_000).prop_map(|((max_attempts, name, kind), secs)| {
        let mut yaml = json!({ "kind": name, "after": secs });
        if let Some(n) = max_attempts {
            yaml["max_attempts"] = json!(n);
        }
        (
            yaml,
            TimerDecl {
                kind,
                after: Duration::from_secs(secs),
            },
        )
    })
}

fn case() -> impl Strategy<Value = Case> {
    let arcs = (
        prop::collection::vec(place(), 0..3),
        prop::collection::vec(place(), 0..3),
        prop::collection::vec(place(), 0..2),
    );
    let effect = (
        prop_oneof![Just(("update_task", false)), Just(("delete_task", true))],
        prop::collection::btree_set("[a-z_]{1,8}", 0..3),
        any::<bool>(),
    );
    let ask = prop::option::of((prop::option::of(input()), any::<bool>()));
    (
        "[a-z][a-z0-9_]{0,10}",
        arcs,
        effect,
        prop::collection::vec(timer(), 0..3),
        ask,
    )
        .prop_map(
            |(name, (take, read, inhibit), ((tool, irreversible_tool), args, said), timers, ask)| {
                let irreversible = irreversible_tool || said;
                let mut yaml = json!({
                    "irreversible": irreversible,
                    "take": take,
                    "read": read,
                    "inhibit": inhibit,
                    "effect": {
                        "tool": tool,
                        "map": args.iter().map(|a| (a.clone(), json!("input"))).collect::<serde_json::Map<_, _>>(),
                    },
                    "timers": timers.iter().map(|(y, _)| y.clone()).collect::<Vec<_>>(),
                });
                if let Some((input, persist)) = &ask {
                    yaml["manual"] = json!(true);
                    yaml["persist"] = json!(persist);
                    if let Some((input, _)) = input {
                        yaml["input"] = input.clone();
                    }
                }
                let arcs = [
                    (ArcKind::Take, &take),
                    (ArcKind::Read, &read),
                    (ArcKind::Inhibit, &inhibit),
                ]
                .into_iter()
                .flat_map(|(kind, places)| places.iter().map(move |p| arc(kind, p)))
                .collect();
                let args: Vec<String> = args
                    .iter()
                    .map(|a| format!("{a}: {ANSWER_INPUT_KEY}"))
                    .collect();
                let ir = TransitionDescriptor {
                    name: TransitionName(name.clone()),
                    arcs,
                    body: call(&format!("{tool}({})", args.join(", "))),
                    irreversible,
                    timers: timers.into_iter().map(|(_, t)| t).collect(),
                    guard: None,
                    ask: ask.map(|(input, persist)| AskDecl {
                        input: input.map(|(_, hint)| hint),
                        preview: None,
                        persist,
                    }),
                };
                Case {
                    yaml: json!({ name: yaml }),
                    ir,
                }
            },
        )
}

proptest! {
    #[test]
    fn a_generated_transition_round_trips_through_sidecar_yaml(case in case()) {
        let yaml = sidecar_with(case.yaml.clone());
        prop_assert_eq!(transitions_from_sidecar(&yaml).unwrap(), vec![case.ir]);
    }

    /// The sidecar re-serialized (as `IntegrationFileConfig` does) loads the
    /// same IR.
    #[test]
    fn a_reserialized_sidecar_loads_the_same_transitions(case in case()) {
        let sidecar = McpSidecar::from_yaml(&sidecar_with(case.yaml)).unwrap();
        let again = McpSidecar::from_yaml(&serde_yaml::to_string(&sidecar).unwrap()).unwrap();
        prop_assert_eq!(
            again.transition_descriptors().unwrap(),
            sidecar.transition_descriptors().unwrap()
        );
    }
}
