//! Sidecar `transitions:` load into the same HL-IR the `#[transition]` macro
//! emits (#167), and a bad sidecar fails the load with a named error.

use std::collections::BTreeMap;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::Value;
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

fn call(tool: &str, args: impl IntoIterator<Item = (String, ToolArg)>) -> BodyDecl {
    BodyDecl::Tool {
        tool: tool.into(),
        args: args.into_iter().collect(),
    }
}

/// Example 1 and 2 of the lane brief, built by hand as the macro would emit
/// them for a Rust declaration of the same shape.
#[test]
fn sidecar_transitions_equal_the_hand_built_ir() {
    let expected = vec![
        TransitionDescriptor {
            name: TransitionName("assign_project".into()),
            arcs: vec![arc(ArcKind::Take, "unfiled")],
            body: call(
                "update_task",
                [
                    ("project_id".into(), ToolArg::Answer),
                    (
                        "source".into(),
                        ToolArg::Literal(Value::String("sidecar".into())),
                    ),
                ],
            ),
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
            body: call("delete_task", []),
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
fn a_duplicate_transition_key_is_a_load_error() {
    let yaml = format!(
        "{FIXTURE}  assign_project:\n    take: [inbox]\n    effect: {{ tool: update_task }}\n"
    );
    let error = format!(
        "{:#}",
        McpSidecar::from_yaml(&yaml).expect_err("a duplicate transition must not load")
    );
    assert!(
        error.contains("transition `assign_project` is declared twice"),
        "{error}"
    );
}

fn answer_to_project() -> serde_json::Value {
    json!({ "tool": "update_task", "map": { "project_id": "input" } })
}

/// A foreign entity type is refused, typed, until a system-wide entity
/// registry is reachable at load time.
#[test]
fn a_foreign_entity_type_is_not_implemented_yet() {
    let yaml = sidecar_with(json!({ "t": {
        "manual": true,
        "input": { "type_hint": "entity_id:todoist_project" },
        "effect": { "tool": "update_task" },
    }}));
    let error = McpSidecar::from_yaml(&yaml).expect_err("a foreign entity type must not load");
    assert_eq!(
        error.downcast_ref::<NotImplemented>(),
        Some(&NotImplemented {
            lane: Lane::Integration,
            what: "resolving an entity type of another integration (a system-wide entity registry)",
        }),
        "{error:#}"
    );
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
            with("input", json!({ "type_hint": "entity_id:todoist_project" })),
            "transition `t`: input type `entity_id:todoist_project` names no entity of this \
             sidecar: resolving an entity type of another integration",
        ),
        (
            with("effect", answer_to_project()),
            "transition `t`: effect argument `project_id` binds the answer value, but the \
             transition asks for no `input:`",
        ),
        (
            {
                let mut t = with("manual", json!(false));
                t["effect"] = answer_to_project();
                t
            },
            "transition `t`: effect argument `project_id` binds the answer value, but the \
             transition asks for no `input:`",
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
            {
                let mut t = with("input", json!({ "type_hint": "bool" }));
                t["effect"] = json!({ "tool": "update_task", "map": { "project_id": "subject" } });
                t
            },
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
        place().prop_map(|e| (
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

fn literal() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        "[a-z ]{0,6}".prop_map(|s| json!(s)),
        any::<i64>().prop_map(|n| json!(n)),
        any::<bool>().prop_map(|b| json!(b)),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    let arcs = (
        prop::collection::vec(place(), 0..3),
        prop::collection::vec(place(), 0..3),
        prop::collection::vec(place(), 0..2),
    );
    let effect = (
        prop_oneof![Just(("update_task", false)), Just(("delete_task", true))],
        prop::collection::btree_map("[a-z_]{1,8}", prop::option::of(literal()), 0..3),
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
            |(
                name,
                (take, read, inhibit),
                ((tool, irreversible_tool), args, said),
                timers,
                ask,
            )| {
                let irreversible = irreversible_tool || said;
                let answers = matches!(ask, Some((Some(_), _)));
                // `None` binds the answer value; without one it is a literal too.
                let args: BTreeMap<String, serde_json::Value> = args
                    .into_iter()
                    .map(|(arg, literal)| {
                        let value = match literal {
                            None if answers => json!("input"),
                            None => json!({ "literal": arg }),
                            Some(literal) => json!({ "literal": literal }),
                        };
                        (arg, value)
                    })
                    .collect();
                let mut yaml = json!({
                    "irreversible": irreversible,
                    "take": take,
                    "read": read,
                    "inhibit": inhibit,
                    "effect": {
                        "tool": tool,
                        "map": args,
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
                let tool_args = args.iter().map(|(arg, value)| {
                    let value = match value.get("literal") {
                        Some(literal) => ToolArg::Literal(Value::from_json_value(literal.clone())),
                        None => ToolArg::Answer,
                    };
                    (arg.clone(), value)
                });
                let ir = TransitionDescriptor {
                    name: TransitionName(name.clone()),
                    arcs,
                    body: call(tool, tool_args),
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
