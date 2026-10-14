//! Sidecar `transitions:` load into the same HL-IR the `#[transition]` macro
//! emits (#167), and a bad sidecar fails the load with a named error.

use std::collections::BTreeMap;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::*;
use holon_mcp_client::IntegrationFileConfig;
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
            undoes: None,
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
            undoes: None,
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
    // Binds the answer, so a row that loads reds on "loaded as" rather than
    // on the unbound-`input:` rule.
    let with_answer = |input: serde_json::Value| {
        let mut t = with("input", input);
        t["effect"] = answer_to_project();
        t
    };
    let cases = [
        (
            with_answer(json!({ "type_hint": "entity_id:" })),
            "transition `t`: type hint `entity_id:` names no entity",
        ),
        (
            with_answer(json!({ "type_hint": "entity_id:todoist_project" })),
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
            with_answer(json!({ "type_hint": "colour" })),
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
            "transition `t`: effect argument `project_id`: write `input` (the answer value) or \
             `{ literal: <value> }`, not `\"subject\"`",
        ),
        (
            {
                let mut t = with("input", json!({ "type_hint": "bool" }));
                t["effect"] = json!({ "tool": "update_task",
                    "map": { "project_id": { "place": "unfiled", "field": "id" } } });
                t
            },
            "transition `t`: effect argument `project_id`: write `input` (the answer value) or \
             `{ literal: <value> }`, not `{",
        ),
        (
            with(
                "effect",
                json!({ "tool": "update_task", "map": { "project_id": { "literal": 1, "extra": 2 } } }),
            ),
            "transition `t`: effect argument `project_id`: write `input` (the answer value) or \
             `{ literal: <value> }`, not `{",
        ),
        (
            with(
                "effect",
                json!({ "tool": "update_task", "map": { "project_id": { "literall": 1 } } }),
            ),
            "transition `t`: effect argument `project_id`: write `input` (the answer value) or \
             `{ literal: <value> }`, not `{\"literall\":1}`",
        ),
        (
            with(
                "effect",
                json!({ "tool": "update_task", "map": { "project_id": 5 } }),
            ),
            "transition `t`: effect argument `project_id`: write `input` (the answer value) or \
             `{ literal: <value> }`, not `5`",
        ),
        (
            with("input", json!({ "type_hint": "bool" })),
            "transition `t`: the transition asks for an `input:`, but no `effect.map` argument \
             binds it",
        ),
        (
            with("persist", json!(true)),
            "transition `t`: `persist: true` needs an `input:`",
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
            with("timers", json!([{ "kind": "run", "after": [5] }])),
            "invalid sync interval `[5]`",
        ),
        (
            with("timers", json!([{ "kind": "run", "after": -5 }])),
            "invalid sync interval `-5`",
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

#[test]
fn an_empty_transition_name_is_a_load_error() {
    let yaml = sidecar_with(json!({ "": { "effect": { "tool": "update_task" } } }));
    let error = format!(
        "{:#}",
        McpSidecar::from_yaml(&yaml).expect_err("an empty transition name must not load")
    );
    assert!(
        error.contains("a transition name must not be empty"),
        "{error}"
    );
}

/// A key declared twice in any map section of a sidecar: a plain map keeps the
/// last one silently, so the load must refuse it. Proptest cannot draw this —
/// the generated YAML goes through `serde_json::Value`, which holds one entry
/// per key.
#[test]
fn a_duplicate_key_in_a_map_section_is_a_named_load_error() {
    let cases = [
        (
            FIXTURE.replace("  inbox: {}", "  inbox: {}\n  unfiled: {}"),
            "entity `unfiled` is declared twice",
        ),
        (
            FIXTURE.replace(
                "  find_tasks:\n    effect: read",
                "  find_tasks:\n    effect: read\n  update_task:\n    effect: read",
            ),
            "tool `update_task` is declared twice",
        ),
        (
            FIXTURE.replace(
                "map: { project_id: input, source: { literal: sidecar } }",
                "map: { project_id: input, project_id: { literal: x } }",
            ),
            "effect argument `project_id` is declared twice",
        ),
        (
            format!("{FIXTURE}\nentities:\n  nowhere: {{}}\n"),
            "duplicate field `entities`",
        ),
    ];
    for (yaml, expected) in cases {
        let error = format!(
            "{:#}",
            McpSidecar::from_yaml(&yaml).expect_err("a duplicate key must not load")
        );
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error:?}"
        );
    }
}
#[test]
fn a_malformed_undo_is_a_named_load_error() {
    let undo = "undo: { reversible: false }";
    let cases = [
        "undo: { reversible: false, tool: undo_delete }",
        "undo: { tool: undo_delete }",
        "undo: 5",
    ];
    for bad in cases {
        let yaml = FIXTURE.replace(undo, bad);
        let error = format!(
            "{:#}",
            McpSidecar::from_yaml(&yaml).expect_err("a malformed undo must not load")
        );
        assert!(
            error.contains("tools.delete_task: `undo` takes"),
            "{bad}: got {error:?}"
        );
    }
}

/// A sidecar file is deserialized on three paths — the bundled one parses the
/// file text straight into [`IntegrationFileConfig`] — and a duplicate key
/// must be refused on each. `sidecar()` re-serializes the already-parsed maps,
/// so a duplicate that survives the first parse is laundered and invisible to
/// every later check.
#[test]
fn a_duplicate_key_is_refused_on_every_sidecar_parse_path() {
    const BUNDLED: &str = include_str!("../../../assets/integrations/todoist.yaml");
    let cases = [
        (
            FIXTURE.replace("  inbox: {}", "  inbox: {}\n  unfiled: {}"),
            "entity `unfiled` is declared twice",
        ),
        (
            FIXTURE.replace(
                "  find_tasks:\n    effect: read",
                "  find_tasks:\n    effect: read\n  update_task:\n    effect: read",
            ),
            "tool `update_task` is declared twice",
        ),
        (
            BUNDLED.replace(
                "entities:\n  todoist_tasks:",
                "entities:\n  todoist_tasks: {}\n  todoist_tasks:",
            ),
            "entity `todoist_tasks` is declared twice",
        ),
    ];
    /// A named way to parse a sidecar file's text.
    type ParsePath = (&'static str, fn(&str) -> anyhow::Result<()>);
    let paths: [ParsePath; 3] = [
        ("serde_yaml::from_str::<IntegrationFileConfig>", |yaml| {
            serde_yaml::from_str::<IntegrationFileConfig>(yaml)?;
            Ok(())
        }),
        ("IntegrationFileConfig::sidecar", |yaml| {
            serde_yaml::from_str::<IntegrationFileConfig>(yaml)?.sidecar()?;
            Ok(())
        }),
        ("McpSidecar::from_yaml", |yaml| {
            McpSidecar::from_yaml(yaml)?;
            Ok(())
        }),
    ];
    for (yaml, expected) in cases {
        for (path, parse) in paths {
            let error = format!(
                "{:#}",
                parse(&yaml)
                    .expect_err(&format!("{path} must refuse a duplicate key ({expected})"))
            );
            assert!(
                error.contains(expected),
                "{path}: expected {expected:?}, got {error:?}"
            );
        }
    }
}

/// Everything the generator drew for one transition: the YAML fields, with no
/// legality rule applied. [`legal`] alone decides what the load must do.
#[derive(Debug, Clone)]
struct Draw {
    name: String,
    take: Vec<&'static str>,
    read: Vec<&'static str>,
    inhibit: Vec<&'static str>,
    tool: &'static str,
    irreversible: bool,
    args: BTreeMap<String, Arg>,
    timers: Vec<(serde_json::Value, TimerDecl)>,
    /// `Some((input, persist))` = `manual: true`; the input is `Err(reason)`
    /// when the answer type is one the loader must refuse.
    ask: Option<(Option<DrawnInput>, bool)>,
}

/// A drawn `input:` section with the answer type it must load as, or the reason
/// the load must name.
type DrawnInput = (serde_json::Value, Result<TypeHint, String>);

/// How a generated `effect.map` argument is written.
#[derive(Debug, Clone)]
enum Arg {
    /// `input`: binds the answer value, legal only when the transition asks.
    Answer,
    Literal(serde_json::Value),
    /// A shape the loader must refuse by name.
    Malformed(serde_json::Value),
}

fn arg() -> impl Strategy<Value = Arg> {
    prop_oneof![
        Just(Arg::Answer),
        literal().prop_map(Arg::Literal),
        prop_oneof![
            literal().prop_filter("`input` binds", |v| v != "input"),
            Just(json!(null)),
            Just(json!([1])),
            place().prop_map(|p| json!({ "place": p, "field": "id" })),
            literal().prop_map(|v| json!({ "literal": v, "extra": 1 })),
            literal().prop_map(|v| json!({ "literall": v })),
        ]
        .prop_map(Arg::Malformed),
    ]
}

const PLACES: [&str; 3] = ["unfiled", "project", "inbox"];

fn place() -> impl Strategy<Value = &'static str> {
    prop::sample::select(PLACES.to_vec())
}

/// A place an arc points at: a declared entity, or — one draw in ten — the
/// empty name the load must refuse.
fn arc_place() -> impl Strategy<Value = &'static str> {
    prop_oneof![9 => place(), 1 => Just("")]
}

fn input() -> impl Strategy<Value = DrawnInput> {
    prop_oneof![
        7 => legal_input(),
        // An entity type of another integration, or none at all: both are
        // unresolvable until a system-wide registry is reachable at load.
        1 => "[a-z]{1,6}"
            .prop_filter("not this sidecar's", |e| !PLACES.contains(&e.as_str()))
            .prop_map(|e| (
                json!({ "type_hint": format!("entity_id:{e}") }),
                Err(format!(
                    "input type `entity_id:{e}` names no entity of this sidecar"
                ))
            )),
    ]
}

fn legal_input() -> impl Strategy<Value = DrawnInput> {
    prop_oneof![
        Just((json!({ "type_hint": "bool" }), Ok(TypeHint::Bool))),
        Just((json!({ "type_hint": "string" }), Ok(TypeHint::String))),
        Just((json!({ "type_hint": "number" }), Ok(TypeHint::Number))),
        place().prop_map(|e| (
            json!({ "type_hint": format!("entity_id:{e}") }),
            Ok(TypeHint::EntityId {
                entity_name: EntityName::new(e)
            })
        )),
        (
            "[A-Z][a-z]{0,5}",
            prop::collection::vec("[A-Za-z]{1,6}", 1..4)
        )
            .prop_map(|(name, variants)| (
                json!({ "enum": { "name": name, "variants": variants } }),
                Ok(TypeHint::Enum {
                    type_name: name,
                    variants
                })
            )),
        "[A-Z][a-z]{0,5}".prop_map(|n| (
            json!({ "newtype": n }),
            Ok(TypeHint::Newtype { type_name: n })
        )),
        place().prop_map(|p| (
            json!({ "place": p }),
            Ok(TypeHint::PlaceId {
                place: PlaceName(p.into())
            })
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

/// Every combination is drawn, legal or not; [`legal`] says which.
fn draw() -> impl Strategy<Value = Draw> {
    let arcs = (
        prop::collection::vec(arc_place(), 0..3),
        prop::collection::vec(arc_place(), 0..3),
        prop::collection::vec(arc_place(), 0..2),
    );
    let effect = (
        prop_oneof![Just(("update_task", false)), Just(("delete_task", true))],
        prop::collection::btree_map("[a-z_]{1,8}", arg(), 0..3),
        any::<bool>(),
    );
    let ask = prop::option::of((prop::option::of(input()), any::<bool>()));
    (
        prop_oneof![9 => "[a-z][a-z0-9_]{0,10}", 1 => Just(String::new())],
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
            )| Draw {
                name,
                take,
                read,
                inhibit,
                tool,
                irreversible: irreversible_tool || said,
                args,
                timers,
                ask,
            },
        )
}

/// Only draws [`legal`] accepts: each place carries at most one arc, `persist`
/// only with an `input:`, and an argument binds the answer iff one is asked.
fn legal_draw() -> impl Strategy<Value = Draw> {
    let arcs = (
        Just(PLACES.to_vec()).prop_shuffle(),
        prop::collection::vec(0..4u8, PLACES.len()),
    )
        .prop_map(|(places, kinds)| {
            let of = |kind| {
                places
                    .iter()
                    .zip(&kinds)
                    .filter(|(_, k)| **k == kind)
                    .map(|(p, _)| *p)
                    .collect::<Vec<_>>()
            };
            (of(1), of(2), of(3))
        });
    let ask = prop::option::of(prop_oneof![
        Just((None, false)),
        (legal_input(), any::<bool>()).prop_map(|(input, persist)| (Some(input), persist)),
    ]);
    let args = ask.prop_flat_map(|ask| {
        let asked = matches!(ask, Some((Some(_), _)));
        let arg = if asked {
            prop_oneof![Just(Arg::Answer), literal().prop_map(Arg::Literal)].boxed()
        } else {
            literal().prop_map(Arg::Literal).boxed()
        };
        (
            Just(ask),
            prop::collection::btree_map("[a-z_]{1,8}", arg, 0..3),
            "[a-z_]{1,8}",
        )
            .prop_map(move |(ask, mut args, answer)| {
                if asked {
                    args.insert(answer, Arg::Answer);
                }
                (ask, args)
            })
    });
    (
        "[a-z][a-z0-9_]{0,10}",
        arcs,
        prop_oneof![Just(("update_task", false)), Just(("delete_task", true))],
        any::<bool>(),
        prop::collection::vec(timer(), 0..3),
        args,
    )
        .prop_map(
            |(
                name,
                (take, read, inhibit),
                (tool, irreversible_tool),
                said,
                timers,
                (ask, args),
            )| {
                Draw {
                    name,
                    take,
                    read,
                    inhibit,
                    tool,
                    irreversible: irreversible_tool || said,
                    args,
                    timers,
                    ask,
                }
            },
        )
}

/// The drawn transition as a sidecar entry.
fn yaml_of(d: &Draw) -> serde_json::Value {
    let mut yaml = json!({
        "irreversible": d.irreversible,
        "take": d.take,
        "read": d.read,
        "inhibit": d.inhibit,
        "effect": {
            "tool": d.tool,
            "map": d.args.iter().map(|(name, arg)| (name.clone(), match arg {
                Arg::Answer => json!("input"),
                Arg::Literal(v) => json!({ "literal": v }),
                Arg::Malformed(v) => v.clone(),
            })).collect::<serde_json::Map<_, _>>(),
        },
        "timers": d.timers.iter().map(|(y, _)| y.clone()).collect::<Vec<_>>(),
    });
    if let Some((input, persist)) = &d.ask {
        yaml["manual"] = json!(true);
        yaml["persist"] = json!(persist);
        if let Some((input, _)) = input {
            yaml["input"] = input.clone();
        }
    }
    let name = d.name.clone();
    json!({ name: yaml })
}

/// The legality rules of a sidecar transition, each stated once and
/// independent of the loader's code path: the IR a legal draw must load as,
/// or the reason a load must name. A draw can break several rules at once, so
/// the rules are in the order the load reports them: the first one it meets
/// wins.
fn legal(d: &Draw) -> Result<TransitionDescriptor, String> {
    let refuse = |reason: String| Err(format!("transition `{}`: {reason}", d.name));
    if d.name.is_empty() {
        return refuse("a transition name must not be empty".to_owned());
    }
    let (input, persist) = match &d.ask {
        Some((input, persist)) => (input.as_ref(), *persist),
        None => (None, false),
    };
    if persist && input.is_none() {
        return refuse("`persist: true` needs an `input:`".to_owned());
    }
    // Every place is named before any arc is laid, so an empty name anywhere
    // outranks a duplicate arc.
    if d.take
        .iter()
        .chain(&d.read)
        .chain(&d.inhibit)
        .any(|p| p.is_empty())
    {
        return refuse("a place name must not be empty".to_owned());
    }
    let mut arced = std::collections::BTreeSet::new();
    for place in d.take.iter().chain(&d.read).chain(&d.inhibit) {
        if !arced.insert(*place) {
            return refuse(format!("place `{place}` carries more than one arc"));
        }
    }
    if let Some((_, Err(reason))) = input {
        return refuse(reason.clone());
    }
    for (arg, value) in &d.args {
        match value {
            Arg::Malformed(_) => return refuse(format!("effect argument `{arg}`: write `input`")),
            Arg::Answer if input.is_none() => {
                return refuse(format!(
                    "effect argument `{arg}` binds the answer value, but the transition asks for \
                     no `input:`"
                ));
            }
            _ => {}
        }
    }
    if input.is_some() && !d.args.values().any(|arg| matches!(arg, Arg::Answer)) {
        return refuse(
            "the transition asks for an `input:`, but no `effect.map` argument binds it".to_owned(),
        );
    }
    Ok(TransitionDescriptor {
        name: TransitionName(d.name.clone()),
        arcs: [
            (ArcKind::Take, &d.take),
            (ArcKind::Read, &d.read),
            (ArcKind::Inhibit, &d.inhibit),
        ]
        .into_iter()
        .flat_map(|(kind, places)| places.iter().map(move |p| arc(kind, p)))
        .collect(),
        body: call(
            d.tool,
            d.args.iter().map(|(name, arg)| {
                let value = match arg {
                    Arg::Answer => ToolArg::Answer,
                    Arg::Literal(v) => ToolArg::Literal(Value::from_json_value(v.clone())),
                    Arg::Malformed(_) => unreachable!("a malformed argument is refused"),
                };
                (name.clone(), value)
            }),
        ),
        irreversible: d.irreversible,
        timers: d.timers.iter().map(|(_, t)| t.clone()).collect(),
        guard: None,
        ask: d.ask.as_ref().map(|(input, persist)| AskDecl {
            input: input.as_ref().map(|(_, hint)| {
                hint.as_ref()
                    .expect("an unresolvable answer type is refused")
                    .clone()
            }),
            preview: None,
            persist: *persist,
        }),
        undoes: None,
    })
}

proptest! {
    #[test]
    fn a_generated_transition_loads_iff_it_is_legal(d in draw()) {
        let loaded = transitions_from_sidecar(&sidecar_with(yaml_of(&d)));
        match legal(&d) {
            Ok(ir) => prop_assert_eq!(loaded.unwrap(), vec![ir]),
            Err(refusal) => {
                let error = format!("{:#}", loaded.expect_err("an illegal transition must not load"));
                prop_assert!(error.contains(&refusal), "expected {:?}, got {:?}", refusal, error);
            }
        }
    }

}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// The sidecar re-serialized (as `IntegrationFileConfig` does) loads the
    /// same IR.
    #[test]
    fn a_reserialized_sidecar_loads_the_same_transitions(d in legal_draw()) {
        prop_assert!(legal(&d).is_ok(), "legal_draw drew an illegal transition: {:?}", legal(&d));
        let sidecar = McpSidecar::from_yaml(&sidecar_with(yaml_of(&d))).unwrap();
        let again = McpSidecar::from_yaml(&serde_yaml::to_string(&sidecar).unwrap()).unwrap();
        prop_assert_eq!(
            again.transition_descriptors().unwrap(),
            sidecar.transition_descriptors().unwrap()
        );
    }
}
