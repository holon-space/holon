//! THE AGREEMENT GATE — ADR 0024 Phase-2 spike exit criterion (plan §7.1).
//!
//! One guard *semantics*, two *evaluators* that must agree (ADR 0024 P1b). This
//! test proves it: for the same [`Guard`] and the same world, the in-memory
//! [`Guard::evaluate`] and the SQL [`Guard::to_sql`] (run against an in-memory
//! Turso DB with a `clock` stub) produce the SAME enabled/disabled verdict AND
//! the SAME binding set — on a seeded journal fixture and across generated
//! worlds.
//!
//! If this ever goes red the Pattern AST shape is wrong; that is what the spike
//! is for (plan §7.1: "If this cannot be made to agree, stop and escalate").

use std::collections::HashMap;
use std::sync::OnceLock;

use holon_advice::holon_rule::parse_holon_rule;
use holon_api::Value;
use holon_api::pattern::BuiltinRef;
use holon_api::pattern::CmpOp;
use holon_api::pattern::CurrentSchema;
use holon_api::pattern::FieldRef;
use holon_api::pattern::Guard;
use holon_api::pattern::InMemoryWorld;
use holon_api::pattern::Operand;
use holon_api::pattern::PathPattern;
use holon_api::pattern::PathSegment;
use holon_api::pattern::Pattern;
use holon_api::pattern::Subject;
use holon_api::pattern::WorldBlock;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;
use tokio::runtime::Runtime;

// ─── DB harness (the reactive evaluator's substrate) ──────────────────────

/// A shared current-thread runtime: proptest cases are sync, the DB is async.
fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("tokio runtime"))
}

/// Fresh in-memory DB with the spike schema: a `block(id, name, parent_id,
/// properties)` table, a `block_tags` junction, and a deterministic `clock`
/// relation (never `date('now')` — ADR 0024 P5).
async fn setup_db(world: &InMemoryWorld) -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    for ddl in [
        "CREATE TABLE block (id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT, properties \
         TEXT NOT NULL DEFAULT '{}')",
        "CREATE TABLE block_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY \
         (block_id, tag))",
        "CREATE TABLE clock (today TEXT PRIMARY KEY)",
    ] {
        handle.execute_ddl(ddl).await.expect("ddl");
    }
    handle
        .execute(
            "INSERT INTO clock (today) VALUES (?)",
            vec![turso::Value::Text(world.today.clone())],
        )
        .await
        .expect("seed clock");
    for b in &world.blocks {
        let props = serde_json::to_string(&b.properties).expect("props json");
        handle
            .execute(
                "INSERT INTO block (id, name, parent_id, properties) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(b.id.clone()),
                    turso::Value::Text(b.name.clone()),
                    match &b.parent_id {
                        Some(p) => turso::Value::Text(p.clone()),
                        None => turso::Value::Null,
                    },
                    turso::Value::Text(props),
                ],
            )
            .await
            .expect("insert block");
        for tag in &b.tags {
            handle
                .execute(
                    "INSERT INTO block_tags (block_id, tag) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(b.id.clone()),
                        turso::Value::Text(tag.clone()),
                    ],
                )
                .await
                .expect("insert tag");
        }
    }
    handle
}

/// The SQL evaluator's bindings for `guard` against `world`, sorted.
async fn sql_bindings(guard: &Guard, world: &InMemoryWorld) -> Vec<String> {
    let handle = setup_db(world).await;
    let sql = guard.to_sql(&CurrentSchema);
    let rows = handle
        .query(&sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("SQL query failed:\n{sql}\n\nerror: {e}"));
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| {
            r.get("binding")
                .unwrap_or_else(|| panic!("row missing `binding` column for:\n{sql}"))
                .as_string()
                .expect("binding is text")
                .to_string()
        })
        .collect();
    out.sort();
    out
}

/// The in-memory evaluator's bindings for `guard` against `world`, sorted.
fn inmem_bindings(guard: &Guard, world: &InMemoryWorld) -> Vec<String> {
    let mut out: Vec<String> = guard
        .evaluate(world)
        .bindings
        .into_iter()
        .map(|b| match b {
            holon_api::pattern::Binding::Today(t) => t,
            holon_api::pattern::Binding::Block(id) => id,
        })
        .collect();
    out.sort();
    out
}

fn wb(id: &str, name: &str, parent: Option<&str>, tags: &[&str], kind: Option<&str>) -> WorldBlock {
    let mut properties = HashMap::new();
    if let Some(k) = kind {
        properties.insert("kind".to_string(), Value::String(k.to_string()));
    }
    WorldBlock {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: parent.map(str::to_string),
        properties,
        tags: tags.iter().map(|s| s.to_string()).collect(),
    }
}

// ─── The seeded fixture (exit-criterion clauses a/b/c) ────────────────────

#[test]
fn journal_guard_fixture_agreement() {
    // The journal guard, authored as YAML (both forms must agree — §7.1).
    let sugar =
        parse_holon_rule("name: daily_journal\nwhen: 'not block_exists(\"Journals/{today}\")'\n")
            .expect("sugar parses")
            .guard;
    let canonical = parse_holon_rule(
        "name: daily_journal\ninput:\n  - bind: c\n    type: clock\n  - bind: j\n    type: \
         journal\n    when: 'block_exists(\"Journals/{today}\")'\n    absent: true\n",
    )
    .expect("canonical parses")
    .guard;
    assert_eq!(
        sugar, canonical,
        "both YAML forms desugar to the same guard"
    );
    assert_eq!(sugar.subject, Subject::Clock);
    let guard = sugar;

    // (a) today's journal ABSENT ⇒ both evaluators say ENABLED.
    let absent = InMemoryWorld::new(vec![wb("j", "Journals", None, &[], None)], "2026-07-10");
    let inmem = inmem_bindings(&guard, &absent);
    let sql = rt().block_on(sql_bindings(&guard, &absent));
    assert_eq!(inmem, vec!["2026-07-10".to_string()], "in-mem enabled");
    assert_eq!(inmem, sql, "absent: in-memory ≡ SQL");

    // (b/c) today's journal PRESENT ⇒ both say DISABLED (empty binding set).
    let present = InMemoryWorld::new(
        vec![
            wb("j", "Journals", None, &[], None),
            wb("d", "2026-07-10", Some("j"), &[], None),
        ],
        "2026-07-10",
    );
    let inmem = inmem_bindings(&guard, &present);
    let sql = rt().block_on(sql_bindings(&guard, &present));
    assert!(inmem.is_empty(), "in-mem disabled");
    assert_eq!(inmem, sql, "present: in-memory ≡ SQL");

    // Day rollover re-enables (the clock relation drives re-fire — P5).
    let tomorrow = InMemoryWorld::new(
        vec![
            wb("j", "Journals", None, &[], None),
            wb("d", "2026-07-10", Some("j"), &[], None),
        ],
        "2026-07-11",
    );
    assert_eq!(
        inmem_bindings(&guard, &tomorrow),
        rt().block_on(sql_bindings(&guard, &tomorrow)),
        "rollover: in-memory ≡ SQL"
    );
    assert!(!inmem_bindings(&guard, &tomorrow).is_empty(), "re-enabled");
}

#[test]
fn advice_has_tag_fixture_agreement() {
    // Deliverable 5: the advice guard `has_tag("project")` (block-driven).
    let guard = parse_holon_rule("name: r\nwhen: 'has_tag(\"project\")'\n")
        .expect("parses")
        .guard;
    assert_eq!(guard.subject, Subject::Block);
    let world = InMemoryWorld::new(
        vec![
            wb("a", "A", None, &["project"], None),
            wb("b", "B", None, &["lesson"], None),
            wb("c", "C", None, &["project", "urgent"], None),
        ],
        "2026-07-10",
    );
    let inmem = inmem_bindings(&guard, &world);
    let sql = rt().block_on(sql_bindings(&guard, &world));
    assert_eq!(inmem, vec!["a".to_string(), "c".to_string()]);
    assert_eq!(inmem, sql, "advice guard: in-memory ≡ SQL");
}

// ─── The property test (agreement across generated worlds) ────────────────

const NAMES: &[&str] = &[
    "Journals",
    "Work",
    "note",
    "2026-07-10",
    "2026-07-11",
    "leaf",
];
const TAGS: &[&str] = &["project", "lesson", "urgent"];
const KINDS: &[&str] = &["note", "task"];
const TODAYS: &[&str] = &["2026-07-10", "2026-07-11"];

/// A world of up to 6 blocks (parents point only at earlier blocks, so no
/// cycles) plus a clock value. Deliberately dense in `Journals` / date names so
/// the journal inhibitor flips both ways.
fn world_strategy() -> impl Strategy<Value = InMemoryWorld> {
    let block = (
        prop::sample::select(NAMES),
        prop::option::of(0usize..6),
        prop::collection::vec(prop::sample::select(TAGS), 0..3),
        prop::option::of(prop::sample::select(KINDS)),
    );
    (
        prop::collection::vec(block, 0..6),
        prop::sample::select(TODAYS),
    )
        .prop_map(|(specs, today)| {
            let n = specs.len();
            let blocks = specs
                .into_iter()
                .enumerate()
                .map(|(i, (name, parent_idx, tags, kind))| {
                    let parent = parent_idx
                        .filter(|p| *p < i && *p < n)
                        .map(|p| format!("b{p}"));
                    let mut properties = HashMap::new();
                    if let Some(k) = kind {
                        properties.insert("kind".to_string(), Value::String(k.to_string()));
                    }
                    let mut tags: Vec<String> = tags.into_iter().map(str::to_string).collect();
                    tags.sort();
                    tags.dedup();
                    WorldBlock {
                        id: format!("b{i}"),
                        name: name.to_string(),
                        parent_id: parent,
                        properties,
                        tags,
                    }
                })
                .collect();
            InMemoryWorld::new(blocks, today.to_string())
        })
}

/// A clock-driven guard: BlockExists paths that always carry `{today}`, under
/// arbitrary And/Or/Not (the inhibitor family — the journal rule generalized).
fn clock_body_strategy() -> impl Strategy<Value = Pattern> {
    let leaf = prop_oneof![
        Just(vec![
            PathSegment::Lit("Journals".to_string()),
            PathSegment::Builtin(BuiltinRef::Today)
        ]),
        Just(vec![
            PathSegment::Lit("Work".to_string()),
            PathSegment::Builtin(BuiltinRef::Today)
        ]),
        Just(vec![PathSegment::Builtin(BuiltinRef::Today)]),
    ]
    .prop_map(|segments| Pattern::BlockExists(PathPattern { segments }));
    recursive_bool(leaf)
}

/// A block-driven guard: HasTag / field comparisons / literal-path existence,
/// under arbitrary And/Or/Not. No builtin (block-subject by construction).
fn block_body_strategy() -> impl Strategy<Value = Pattern> {
    let leaf = prop_oneof![
        prop::sample::select(TAGS).prop_map(|t| Pattern::HasTag(t.to_string())),
        (prop::sample::select(NAMES), any::<bool>()).prop_map(|(v, ne)| Pattern::Field {
            field: FieldRef::Name,
            op: if ne { CmpOp::Ne } else { CmpOp::Eq },
            rhs: Operand::Lit(Value::String(v.to_string())),
        }),
        (prop::sample::select(KINDS), any::<bool>()).prop_map(|(v, ne)| Pattern::Field {
            field: FieldRef::Property("kind".to_string()),
            op: if ne { CmpOp::Ne } else { CmpOp::Eq },
            rhs: Operand::Lit(Value::String(v.to_string())),
        }),
        (prop::sample::select(NAMES), prop::sample::select(NAMES)).prop_map(|(a, b)| {
            Pattern::BlockExists(PathPattern {
                segments: vec![
                    PathSegment::Lit(a.to_string()),
                    PathSegment::Lit(b.to_string()),
                ],
            })
        }),
    ];
    recursive_bool(leaf)
}

/// Wrap a leaf strategy in bounded And/Or/Not nesting.
fn recursive_bool(
    leaf: impl Strategy<Value = Pattern> + 'static,
) -> impl Strategy<Value = Pattern> {
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|p| Pattern::Not(Box::new(p))),
            prop::collection::vec(inner.clone(), 1..3).prop_map(Pattern::And),
            prop::collection::vec(inner, 1..3).prop_map(Pattern::Or),
        ]
    })
}

fn guard_strategy() -> impl Strategy<Value = Guard> {
    prop_oneof![
        clock_body_strategy().prop_map(|body| Guard {
            subject: Subject::Clock,
            body
        }),
        block_body_strategy().prop_map(|body| Guard {
            subject: Subject::Block,
            body
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 400,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// The pinned invariant: in-memory ≡ SQL, on every generated world+guard.
    #[test]
    fn in_memory_agrees_with_sql(world in world_strategy(), guard in guard_strategy()) {
        let inmem = inmem_bindings(&guard, &world);
        let sql = rt().block_on(sql_bindings(&guard, &world));
        prop_assert_eq!(
            &inmem, &sql,
            "DISAGREEMENT\nguard = {:?}\nsql = {}\nworld today = {}\nblocks = {:?}",
            guard, guard.to_sql(&CurrentSchema), world.today, world.blocks
        );
    }
}
