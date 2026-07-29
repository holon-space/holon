//! Deterministic oracle tests for engine semantics: known nets, known markings,
//! exact expected outcomes. Complements tests/pbt.rs, whose properties are
//! self-consistency checks and therefore blind to "everything is disabled" and
//! "storage is a no-op" failure modes.

use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::Utc;
use holon_engine::Marking;
use holon_engine::NetDef;
use holon_engine::PrecondSpec;
use holon_engine::TransitionDef;
use holon_engine::engine::Engine;
use holon_engine::guard::RhaiEvaluator;
use holon_engine::objective;
use holon_engine::value::Value;
use holon_engine::yaml::history::AttrChange;
use holon_engine::yaml::history::CreatedToken;
use holon_engine::yaml::history::Event;
use holon_engine::yaml::history::History;
use holon_engine::yaml::net::YamlNet;
use holon_engine::yaml::net::YamlNetFile;
use holon_engine::yaml::state::YamlMarking;
use holon_engine::yaml::state::YamlToken;

fn net_from_yaml(yaml: &str) -> YamlNet {
    let file: YamlNetFile = serde_yaml::from_str(yaml).expect("net yaml must parse");
    let transitions = file
        .transitions
        .into_iter()
        .map(|(name, mut t)| {
            t.name = name;
            t
        })
        .collect();
    YamlNet::new(transitions, file.objective).expect("net must compile")
}

fn t0() -> DateTime<Utc> {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

/// One token fixture: `(id, token_type, [(field, value)])`.
type TokenSpec<'a> = (&'a str, &'a str, Vec<(&'a str, Value)>);

fn marking(tokens: Vec<TokenSpec<'_>>) -> YamlMarking {
    YamlMarking {
        clock: t0(),
        tokens: tokens
            .into_iter()
            .map(|(name, ty, attrs)| YamlToken {
                name: name.to_string(),
                token_type: ty.to_string(),
                attributes: attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            })
            .collect(),
    }
}

#[test]
fn precond_spec_display_is_the_persistence_format() {
    for (input, canonical) in [
        ("$who", "$who"),
        ("done", "done"),
        (">= 0.2", ">= 0.2"),
        (">=0.2", ">= 0.2"),
        ("<= 10", "<= 10"),
        ("> 1", "> 1"),
        ("< 2", "< 2"),
        ("== 3", "== 3"),
        ("!= 4", "!= 4"),
    ] {
        let spec: PrecondSpec = input.parse().unwrap();
        assert_eq!(spec.to_string(), canonical, "Display of '{input}'");
        let reparsed: PrecondSpec = spec.to_string().parse().unwrap();
        assert_eq!(reparsed, spec, "round-trip of '{input}'");
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let deser: PrecondSpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deser, spec, "serde round-trip of '{input}'");
    }
}

#[test]
fn precond_spec_eq_distinguishes_variants_and_fields() {
    let p = |s: &str| s.parse::<PrecondSpec>().unwrap();
    assert_ne!(p("done"), p("todo"));
    assert_ne!(p("$a"), p("$b"));
    assert_ne!(p(">= 2"), p(">= 3"), "same op, different rhs");
    assert_ne!(p(">= 2"), p("<= 2"), "different op, same rhs");
    assert_ne!(p("done"), p("$done"));
    assert_ne!(p("done"), p("== done"));
    assert_eq!(p(">= 2"), p(">=  2"), "rhs is trimmed");
}

#[test]
fn exact_preconds_gate_enabling_per_value_type() {
    let net = net_from_yaml(
        r#"
transitions:
  t_str:
    inputs: [{ bind: x, token_type: ts, precond: { v: "done" }, consume: true }]
    outputs: []
  t_int:
    inputs: [{ bind: x, token_type: ti, precond: { v: "3" }, consume: true }]
    outputs: []
  t_float:
    inputs: [{ bind: x, token_type: tf, precond: { v: "0.5" }, consume: true }]
    outputs: []
  t_bool:
    inputs: [{ bind: x, token_type: tb, precond: { v: "true" }, consume: true }]
    outputs: []
  t_null:
    inputs: [{ bind: x, token_type: tn, precond: { v: "null" }, consume: true }]
    outputs: []
  t_cmp:
    inputs: [{ bind: x, token_type: tc, precond: { v: ">= 10" }, consume: true }]
    outputs: []
  t_absent:
    inputs: [{ bind: x, token_type: ts, precond: { v: "never" }, consume: true }]
    outputs: []
"#,
    );
    let m = marking(vec![
        ("m_str", "ts", vec![("v", Value::String("done".into()))]),
        ("n_str", "ts", vec![("v", Value::String("todo".into()))]),
        ("m_int", "ti", vec![("v", Value::Int(3))]),
        ("n_int", "ti", vec![("v", Value::Int(4))]),
        ("m_float", "tf", vec![("v", Value::Float(0.5))]),
        ("n_float", "tf", vec![("v", Value::Float(0.6))]),
        ("m_bool", "tb", vec![("v", Value::Bool(true))]),
        ("n_bool", "tb", vec![("v", Value::Bool(false))]),
        ("m_null", "tn", vec![("v", Value::Null)]),
        ("m_cmp", "tc", vec![("v", Value::Int(12))]),
        ("n_cmp", "tc", vec![("v", Value::Int(5))]),
    ]);

    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    let mut got: Vec<(String, String)> = enabled
        .iter()
        .map(|b| (b.transition_id.clone(), b.token_bindings["x"].clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("t_bool".to_string(), "m_bool".to_string()),
            ("t_cmp".to_string(), "m_cmp".to_string()),
            ("t_float".to_string(), "m_float".to_string()),
            ("t_int".to_string(), "m_int".to_string()),
            ("t_null".to_string(), "m_null".to_string()),
            ("t_str".to_string(), "m_str".to_string()),
        ]
    );
}

#[test]
fn placeholder_unification_across_arcs() {
    let net = net_from_yaml(
        r#"
transitions:
  pair:
    inputs:
      - { bind: a, token_type: person, precond: { who: "$who" }, consume: true }
      - { bind: b, token_type: document, precond: { who: "$who" }, consume: true }
    outputs: []
"#,
    );
    let engine = Engine::new();

    let m = marking(vec![
        ("p1", "person", vec![("who", Value::String("alice".into()))]),
        ("d1", "document", vec![("who", Value::String("bob".into()))]),
        (
            "d2",
            "document",
            vec![("who", Value::String("alice".into()))],
        ),
    ]);
    let enabled = engine.enabled(&net, &m).unwrap();
    assert_eq!(enabled.len(), 1);
    let b = &enabled[0];
    assert_eq!(b.token_bindings["a"], "p1");
    assert_eq!(
        b.token_bindings["b"], "d2",
        "unification must skip the bob document"
    );
    assert_eq!(b.placeholders["$who"], Value::String("alice".into()));

    let m2 = marking(vec![
        ("p1", "person", vec![("who", Value::String("alice".into()))]),
        ("d1", "document", vec![("who", Value::String("bob".into()))]),
    ]);
    assert!(
        engine.enabled(&net, &m2).unwrap().is_empty(),
        "unequal placeholder values must not unify"
    );
}

#[test]
fn backtracking_recovers_from_greedy_first_match() {
    let net = net_from_yaml(
        r#"
transitions:
  pick:
    inputs:
      - { bind: any, token_type: task, consume: true }
      - { bind: done, token_type: task, precond: { state: "done" }, consume: true }
    outputs: []
"#,
    );
    // 'any' greedily grabs t_done first (insertion order); only backtracking
    // can then satisfy 'done'.
    let m = marking(vec![
        (
            "t_done",
            "task",
            vec![("state", Value::String("done".into()))],
        ),
        (
            "t_open",
            "task",
            vec![("state", Value::String("open".into()))],
        ),
    ]);
    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].token_bindings["any"], "t_open");
    assert_eq!(enabled[0].token_bindings["done"], "t_done");
}

#[test]
fn fire_applies_postconds_consumes_creates_and_advances_clock() {
    let net = net_from_yaml(
        r#"
transitions:
  review:
    duration: 7
    inputs:
      - { bind: doc, token_type: document, consume: false }
      - { bind: tmp, token_type: scratch, consume: true }
    outputs:
      - from: doc
        postcond: { state: "tmp.payload" }
    creates:
      - id_expr: '"log_" + step.n'
        token_type: log
        attrs: { origin: "doc.state" }
"#,
    );
    let mut m = marking(vec![
        (
            "doc1",
            "document",
            vec![("state", Value::String("open".into()))],
        ),
        (
            "tmp1",
            "scratch",
            vec![("payload", Value::String("done".into()))],
        ),
    ]);
    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    assert_eq!(enabled.len(), 1);

    let event = engine.fire(&net, &mut m, &enabled[0], 5).unwrap();

    assert_eq!(event.step, 5);
    assert_eq!(event.time, t0());
    assert_eq!(event.duration, 7.0);
    assert_eq!(event.changes.len(), 1);
    assert_eq!(event.changes[0].token, "doc1");
    assert_eq!(event.changes[0].attr, "state");
    assert_eq!(event.changes[0].from, Value::String("open".into()));
    assert_eq!(event.changes[0].to, Value::String("done".into()));
    assert_eq!(event.removed, vec!["tmp1".to_string()]);
    assert_eq!(event.created.len(), 1);
    assert_eq!(event.created[0].id, "log_5");
    assert_eq!(event.created[0].token_type, "log");
    assert_eq!(
        event.created[0].attrs["origin"],
        Value::String("open".into())
    );

    assert_eq!(
        m.token("doc1").unwrap().attributes["state"],
        Value::String("done".into())
    );
    assert!(m.token("tmp1").is_none(), "consumed token must be removed");
    let log = m.token("log_5").expect("created token must exist");
    assert_eq!(log.token_type, "log");
    assert_eq!(log.attributes["origin"], Value::String("open".into()));
    assert_eq!(m.clock, t0() + chrono::Duration::minutes(7));
}

#[test]
fn fire_records_no_change_for_identical_postcond_value() {
    let net = net_from_yaml(
        r#"
transitions:
  keep:
    inputs: [{ bind: x, token_type: task, consume: false }]
    outputs: [{ from: x, postcond: { state: '"open"' } }]
"#,
    );
    let mut m = marking(vec![(
        "t1",
        "task",
        vec![("state", Value::String("open".into()))],
    )]);
    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    let event = engine.fire(&net, &mut m, &enabled[0], 1).unwrap();
    assert!(
        event.changes.is_empty(),
        "no-op postcond must not record a change"
    );
}

#[test]
fn rank_orders_by_objective_delta_per_minute() {
    let net = net_from_yaml(
        r#"
transitions:
  quick:
    duration: 2
    inputs: [{ bind: x, token_type: a, consume: false }]
    outputs: [{ from: x, postcond: { v: "x.v + 4.0" } }]
  slow:
    duration: 10
    inputs: [{ bind: y, token_type: b, consume: false }]
    outputs: [{ from: y, postcond: { v: "y.v + 30.0" } }]
objective:
  expr: "q.v + s.v"
"#,
    );
    let m = marking(vec![
        ("q", "a", vec![("v", Value::Float(1.0))]),
        ("s", "b", vec![("v", Value::Float(2.0))]),
    ]);
    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    assert_eq!(enabled.len(), 2);
    let ranked = engine.rank(&net, &m, &enabled).unwrap();

    assert_eq!(ranked[0].binding.transition_id, "slow");
    assert_eq!(ranked[1].binding.transition_id, "quick");
    assert!(
        (ranked[0].delta_obj - 30.0).abs() < 1e-9,
        "slow delta_obj = {}",
        ranked[0].delta_obj
    );
    assert!((ranked[0].delta_per_minute - 3.0).abs() < 1e-9);
    assert!(
        (ranked[1].delta_obj - 4.0).abs() < 1e-9,
        "quick delta_obj = {}",
        ranked[1].delta_obj
    );
    assert!((ranked[1].delta_per_minute - 2.0).abs() < 1e-9);
}

#[test]
fn objective_applies_discount_and_reports_violations() {
    let net = net_from_yaml(
        r#"
transitions: {}
objective:
  expr: "discount * q.v"
  discount_rate: 0.5
  constraints: ["q.v >= 0.0", "q.v > 100.0"]
"#,
    );
    let m = marking(vec![("q", "a", vec![("v", Value::Float(3.0))])]);
    let ev = RhaiEvaluator::new();
    let result = objective::evaluate(&ev, &net, &m).unwrap();
    assert!(
        (result.value - 2.0).abs() < 1e-12,
        "discount = 1/(1+0.5), value = {}",
        result.value
    );
    assert_eq!(
        result.constraint_violations,
        vec!["q.v > 100.0".to_string()]
    );
}

#[test]
fn objective_defaults_to_undiscounted() {
    let net = net_from_yaml(
        r#"
transitions: {}
objective:
  expr: "discount * q.v"
"#,
    );
    let m = marking(vec![("q", "a", vec![("v", Value::Float(3.0))])]);
    let ev = RhaiEvaluator::new();
    let result = objective::evaluate(&ev, &net, &m).unwrap();
    assert!(
        (result.value - 3.0).abs() < 1e-12,
        "default discount_rate must be 0 => discount factor 1, value = {}",
        result.value
    );
}

#[test]
fn transition_duration_defaults_to_one_minute() {
    let net = net_from_yaml(
        r#"
transitions:
  t:
    inputs: [{ bind: x, token_type: a, consume: true }]
    outputs: []
"#,
    );
    let t = net.transition("t").unwrap();
    assert_eq!(t.duration_minutes(), 1.0);
}

#[test]
fn validate_reports_unbound_output_and_dropped_input() {
    let net = net_from_yaml(
        r#"
transitions:
  bad:
    inputs: [{ bind: x, token_type: a, consume: false }]
    outputs: [{ from: ghost, postcond: {} }]
"#,
    );
    let errors = net.validate();
    assert_eq!(errors.len(), 2, "errors: {errors:?}");
    assert!(errors.iter().any(|e| e.contains("unbound name 'ghost'")));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("'x' not re-produced in any output"))
    );
}

#[test]
fn history_save_load_replay_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.yaml");

    let event = Event {
        step: 1,
        time: t0(),
        transition: "review".to_string(),
        duration: 7.0,
        changes: vec![AttrChange {
            token: "doc1".to_string(),
            attr: "state".to_string(),
            from: Value::String("open".into()),
            to: Value::String("done".into()),
        }],
        created: vec![CreatedToken {
            id: "log_1".to_string(),
            token_type: "log".to_string(),
            attrs: BTreeMap::from([("origin".to_string(), Value::String("open".into()))]),
        }],
        removed: vec!["tmp1".to_string()],
    };
    let history = History {
        events: vec![event],
    };
    history.save(&path).unwrap();

    let loaded = History::load(&path).unwrap();
    assert_eq!(loaded.events.len(), 1, "saved event must survive load");
    assert_eq!(loaded.next_step(), 2);

    let mut m = marking(vec![
        (
            "doc1",
            "document",
            vec![("state", Value::String("open".into()))],
        ),
        (
            "tmp1",
            "scratch",
            vec![("payload", Value::String("done".into()))],
        ),
    ]);
    loaded.replay(&mut m);
    assert_eq!(
        m.token("doc1").unwrap().attributes["state"],
        Value::String("done".into())
    );
    assert!(m.token("tmp1").is_none());
    assert!(m.token("log_1").is_some());
    assert_eq!(m.clock, t0() + chrono::Duration::minutes(7));
}

#[test]
fn history_load_of_missing_path_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = History::load(&dir.path().join("absent.yaml")).unwrap();
    assert!(loaded.events.is_empty());
    assert_eq!(loaded.next_step(), 1);
}

#[test]
fn marking_save_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.yaml");
    let m = marking(vec![
        (
            "q",
            "a",
            vec![("v", Value::Float(1.5)), ("s", Value::String("x".into()))],
        ),
        ("r", "b", vec![("n", Value::Int(7))]),
    ]);
    m.save(&path).unwrap();
    let loaded = YamlMarking::load(&path).unwrap();
    assert_eq!(loaded.clock, m.clock);
    assert_eq!(loaded.tokens.len(), 2);
    let q = loaded.token("q").unwrap();
    assert_eq!(q.token_type, "a");
    assert_eq!(q.attributes["v"], Value::Float(1.5));
    assert_eq!(loaded.token("r").unwrap().attributes["n"], Value::Int(7));
}

#[test]
fn set_attr_targets_exactly_the_named_token() {
    let mut m = marking(vec![
        ("q", "a", vec![("v", Value::Int(1))]),
        ("r", "a", vec![("v", Value::Int(2))]),
    ]);
    m.set_attr("r", "v", Value::Int(99));
    assert_eq!(m.token("q").unwrap().attributes["v"], Value::Int(1));
    assert_eq!(m.token("r").unwrap().attributes["v"], Value::Int(99));
}

#[test]
fn value_accessors() {
    assert_eq!(Value::Float(2.5).as_f64(), Some(2.5));
    assert_eq!(Value::Int(2).as_f64(), Some(2.0));
    assert_eq!(Value::String("x".into()).as_f64(), None);

    assert_eq!(Value::String("x".into()).as_str(), Some("x"));
    assert_eq!(Value::Int(2).as_str(), None);

    assert_eq!(Value::Bool(true).as_bool(), Some(true));
    assert_eq!(Value::Bool(false).as_bool(), Some(false));
    assert_eq!(Value::Int(1).as_bool(), None);

    assert_eq!(Value::Float(2.5).to_rhai_dynamic().as_float(), Ok(2.5));
    assert_eq!(Value::Int(3).to_rhai_dynamic().as_int(), Ok(3));
    assert_eq!(Value::Bool(true).to_rhai_dynamic().as_bool(), Ok(true));
    assert_eq!(
        Value::String("s".into()).to_rhai_dynamic().into_string(),
        Ok("s".to_string())
    );
    assert!(Value::Null.to_rhai_dynamic().is_unit());
}

#[test]
fn evaluator_expr_and_bool() {
    let ev = RhaiEvaluator::new();
    let mut scope = rhai::Scope::new();
    assert_eq!(ev.eval_expr("2.0 + 3.0", &mut scope), Ok(5.0));
    assert_eq!(ev.eval_bool("1 < 2", &mut scope), Ok(true));
    assert_eq!(ev.eval_bool("2 < 1", &mut scope), Ok(false));
}

/// F3.1 regression: a duration past chrono::Duration's minute range must be
/// an `Err` from `fire()`, never a panic — `rank()` simulates `fire` on the
/// live `rank_tasks` MCP path with vault-stored durations.
#[test]
fn fire_returns_err_on_duration_overflowing_chrono_duration() {
    let net = net_from_yaml(
        r#"
transitions:
  megatask:
    duration: 200000000000000
    inputs:
      - { bind: doc, token_type: document, consume: false }
    outputs: []
"#,
    );
    let mut m = marking(vec![("doc1", "document", vec![])]);
    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    assert_eq!(enabled.len(), 1);
    let err = engine
        .fire(&net, &mut m, &enabled[0], 0)
        .expect_err("2e14 minutes must be an Err, not a chrono panic");
    assert!(err.contains("overflow"), "got: {err}");
}

/// Companion: a duration that fits in `chrono::Duration` but overflows the
/// representable `DateTime` range hits the second checked step.
#[test]
fn fire_returns_err_on_duration_overflowing_datetime_range() {
    let net = net_from_yaml(
        r#"
transitions:
  megatask:
    duration: 200000000000
    inputs:
      - { bind: doc, token_type: document, consume: false }
    outputs: []
"#,
    );
    let mut m = marking(vec![("doc1", "document", vec![])]);
    let engine = Engine::new();
    let enabled = engine.enabled(&net, &m).unwrap();
    let err = engine
        .fire(&net, &mut m, &enabled[0], 0)
        .expect_err("2e11 minutes must be an Err, not a chrono panic");
    assert!(err.contains("overflow"), "got: {err}");
}
