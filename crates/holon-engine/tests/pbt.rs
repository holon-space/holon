use std::collections::BTreeMap;

use holon_engine::Marking;
use holon_engine::TokenState;
use holon_engine::arc::CreateArc;
use holon_engine::arc::InputArc;
use holon_engine::arc::OutputArc;
use holon_engine::engine::Engine;
use holon_engine::value::Value;
use holon_engine::yaml::history::History;
use holon_engine::yaml::net::ObjectiveDef;
use holon_engine::yaml::net::YamlNet;
use holon_engine::yaml::net::YamlTransition;
use holon_engine::yaml::state::YamlMarking;
use holon_engine::yaml::state::YamlToken;
use proptest::prelude::*;

const TOKEN_TYPES: &[&str] = &["person", "document", "asset", "monetary"];
const STATUSES: &[&str] = &["active", "pending", "done", "available", "missing"];

fn arb_token(id: String) -> impl Strategy<Value = YamlToken> {
    let type_strategy = proptest::sample::select(TOKEN_TYPES);
    let status_strategy = proptest::sample::select(STATUSES);
    (
        type_strategy,
        status_strategy,
        proptest::collection::btree_map(
            // Prefixed so a generated attribute name can never collide with a
            // Rhai keyword (e.g. `for`, `nil`), which would make `tok.<name>`
            // in the generated objective fail to compile.
            "[a-z]{3,6}".prop_map(|s| format!("attr_{s}")),
            -10.0f64..10.0,
            0..3,
        ),
    )
        .prop_map(move |(token_type, status, float_attrs)| {
            let mut attributes = BTreeMap::new();
            attributes.insert("status".to_string(), Value::String(status.to_string()));
            for (k, v) in float_attrs {
                attributes.insert(k, Value::Float(v));
            }
            YamlToken {
                name: id.clone(),
                token_type: token_type.to_string(),
                attributes,
            }
        })
}

fn arb_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        (proptest::num::f64::NORMAL | proptest::num::f64::ZERO | proptest::num::f64::SUBNORMAL)
            .prop_map(Value::Float),
        ".*".prop_map(Value::String),
        Just(Value::Null),
    ]
}

fn arb_transition(
    name: String,
    token_ids: Vec<String>,
    token_types: Vec<String>,
) -> impl Strategy<Value = YamlTransition> {
    assert!(!token_ids.is_empty());

    let n_inputs = 1..=token_ids.len().min(2);
    n_inputs
        .prop_flat_map(move |n| {
            let ti = token_ids.clone();
            let tt = token_types.clone();
            proptest::sample::subsequence((0..ti.len()).collect::<Vec<_>>(), n).prop_flat_map(
                move |selected_indices| {
                    let ti2 = ti.clone();
                    let tt2 = tt.clone();
                    let si = selected_indices.clone();
                    let n = si.len();
                    let status_strats: Vec<_> = si
                        .iter()
                        .map(|_| proptest::sample::select(STATUSES))
                        .collect();
                    let consume_strats: Vec<_> = (0..n).map(|_| proptest::bool::ANY).collect();
                    let postcond_strats: Vec<_> = (0..n)
                        .map(|_| proptest::option::of(proptest::sample::select(STATUSES)))
                        .collect();
                    // true = a shared `$status` placeholder, which unifies
                    // across the transition's arcs; false = exact match.
                    let precond_style_strats: Vec<_> =
                        (0..n).map(|_| proptest::bool::ANY).collect();
                    let n_creates = 0..=2usize;
                    (
                        Just(si),
                        Just(ti2),
                        Just(tt2),
                        status_strats,
                        consume_strats,
                        postcond_strats,
                        precond_style_strats,
                        1.0f64..100.0,
                        n_creates,
                    )
                },
            )
        })
        .prop_map(
            move |(
                selected_indices,
                token_ids,
                token_types,
                statuses,
                consumes,
                postconds,
                placeholder_styles,
                duration,
                n_creates,
            )| {
                let mut inputs = Vec::new();
                let mut outputs = Vec::new();
                for (i, &idx) in selected_indices.iter().enumerate() {
                    let spec = if placeholder_styles[i] {
                        "$status"
                    } else {
                        statuses[i]
                    };
                    let mut precond = BTreeMap::new();
                    precond.insert(
                        "status".to_string(),
                        spec.parse().expect("generated precond spec is valid"),
                    );
                    let consume = consumes[i];
                    inputs.push(InputArc {
                        bind: token_ids[idx].clone(),
                        token_type: token_types[idx].clone(),
                        precond,
                        consume,
                    });
                    let mut postcond = BTreeMap::new();
                    if let Some(post_status) = &postconds[i] {
                        postcond.insert(
                            "status".to_string(),
                            format!("\"{}\"", post_status).parse().unwrap(),
                        );
                    }
                    if !consume {
                        outputs.push(OutputArc {
                            from: token_ids[idx].clone(),
                            postcond,
                        });
                    }
                }

                let all_types: Vec<&str> =
                    token_types.iter().map(|s: &String| s.as_str()).collect();
                let creates: Vec<CreateArc> = (0..n_creates)
                    .map(|i| {
                        let tt = all_types[i % all_types.len()].to_string();
                        CreateArc {
                            id_expr: format!("\"created-{}-{}-\" + step.n", name, i)
                                .parse()
                                .unwrap(),
                            token_type: tt,
                            attrs: BTreeMap::from([(
                                "status".to_string(),
                                holon_engine::AttrInit::Expr("\"new\"".parse().unwrap()),
                            )]),
                        }
                    })
                    .collect();

                YamlTransition {
                    name: name.clone(),
                    inputs,
                    outputs,
                    creates,
                    duration,
                }
            },
        )
}

fn arb_net_and_marking() -> impl Strategy<Value = (YamlNet, YamlMarking)> {
    let n_tokens = 2..=5usize;

    n_tokens
        .prop_flat_map(|nt| {
            let token_ids: Vec<String> = (0..nt).map(|i| format!("tok{i}")).collect();

            let token_strats: Vec<_> = token_ids.iter().map(|id| arb_token(id.clone())).collect();

            let ti = token_ids.clone();
            let n_transitions = 2..=5usize;

            (token_strats, n_transitions).prop_flat_map(move |(tokens, n_trans)| {
                let ti2 = ti.clone();
                let token_types: Vec<String> =
                    tokens.iter().map(|t| t.token_type.clone()).collect();
                let trans_strats: Vec<_> = (0..n_trans)
                    .map(|i| arb_transition(format!("t{i}"), ti2.clone(), token_types.clone()))
                    .collect();
                (Just(tokens), trans_strats)
            })
        })
        .prop_map(|(tokens, mut transitions)| {
            // Make firing move the objective: a kept arc shifts a float attr by
            // a k that varies per transition. `??` because the arc binds by
            // type, so the bound token may lack the attr.
            for (i, t) in transitions.iter_mut().enumerate() {
                let k = ((i * 7 + 3) % 11) as f64 - 5.0;
                for out in &mut t.outputs {
                    let tok = tokens.iter().find(|tk| tk.name == out.from).unwrap();
                    if let Some((attr, _)) = tok
                        .attributes
                        .iter()
                        .find(|(_, v)| matches!(v, Value::Float(_)))
                    {
                        out.postcond.insert(
                            attr.clone(),
                            format!("({}.{} ?? 0.0) + ({:?})", tok.name, attr, k)
                                .parse()
                                .unwrap(),
                        );
                    }
                }
            }
            let obj_parts: Vec<String> = tokens
                .iter()
                .flat_map(|t| {
                    t.attributes
                        .iter()
                        .filter(|(_, v)| matches!(v, Value::Float(_)))
                        .map(move |(k, _)| format!("{}.{}", t.name, k))
                })
                .collect();
            let obj_expr = if obj_parts.is_empty() {
                "0.0".to_string()
            } else {
                obj_parts.join(" + ")
            };

            let net = YamlNet::new(
                transitions,
                ObjectiveDef {
                    expr: obj_expr,
                    constraints: vec![],
                    discount_rate: 0.0,
                },
            )
            .expect("generated net should compile");

            let marking = YamlMarking {
                clock: chrono::DateTime::from_timestamp(1_000_000, 0).unwrap(),
                tokens,
            };

            (net, marking)
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 50,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn determinism(
        (net, marking) in arb_net_and_marking()
    ) {
        let engine = Engine::new();
        let enabled = engine.enabled(&net, &marking)
            .expect("generated preconds cannot fail to evaluate");

        for binding in &enabled {
            let mut sim1 = marking.clone();
            let mut sim2 = marking.clone();
            let event1 = engine.fire(&net, &mut sim1, binding, 1);
            let event2 = engine.fire(&net, &mut sim2, binding, 1);

            match (event1, event2) {
                (Ok(e1), Ok(e2)) => {
                    prop_assert_eq!(e1.changes.len(), e2.changes.len());
                    for (c1, c2) in e1.changes.iter().zip(e2.changes.iter()) {
                        prop_assert_eq!(&c1.token, &c2.token);
                        prop_assert_eq!(&c1.attr, &c2.attr);
                        prop_assert_eq!(&c1.to, &c2.to);
                    }
                    prop_assert_eq!(sim1.tokens().count(), sim2.tokens().count(),
                        "token count mismatch after firing");
                    for t1 in sim1.tokens() {
                        let t2 = sim2.token(t1.id()).unwrap();
                        prop_assert_eq!(t1.token_type(), t2.token_type());
                        prop_assert_eq!(t1.attrs(), t2.attrs());
                    }
                }
                (Err(_), Err(_)) => {}
                _ => prop_assert!(false, "firing gave different error/ok results"),
            }
        }
    }

    #[test]
    fn event_sourcing_roundtrip(
        (net, marking) in arb_net_and_marking()
    ) {
        let engine = Engine::new();
        let mut live = marking.clone();
        let mut history = History { events: vec![] };
        let enabled = engine.enabled(&net, &live)
            .expect("generated preconds cannot fail to evaluate");

        let mut fired = 0;
        let mut current_enabled = enabled;
        while fired < 3 && !current_enabled.is_empty() {
            let binding = &current_enabled[0];
            let step = history.next_step();
            let event = engine.fire(&net, &mut live, binding, step)
                .expect("fire should succeed for enabled transition");
            history.append(event);
            fired += 1;
            current_enabled = engine.enabled(&net, &live)
                .expect("generated preconds cannot fail to evaluate");
        }

        let mut replayed = marking.clone();
        history.replay(&mut replayed);

        prop_assert_eq!(live.clock(), replayed.clock(), "clock mismatch");
        let live_count = live.tokens().count();
        let replay_count = replayed.tokens().count();
        prop_assert_eq!(live_count, replay_count, "token count mismatch");

        for t_live in live.tokens() {
            let t_replay = replayed.token(t_live.id()).unwrap();
            prop_assert_eq!(t_live.token_type(), t_replay.token_type(),
                "token_type mismatch for token {}", t_live.id());
            prop_assert_eq!(t_live.attrs(), t_replay.attrs(),
                "attrs mismatch for token {}", t_live.id());
        }
    }

    /// `rank` returns exactly the enabled transitions, best score first.
    /// Err results (a simulated firing can make the objective unevaluable)
    /// are skipped; only Ok results are asserted on.
    #[test]
    fn rank_returns_enabled_sorted_by_score(
        (net, marking) in arb_net_and_marking()
    ) {
        let engine = Engine::new();
        let enabled = engine.enabled(&net, &marking)
            .expect("generated preconds cannot fail to evaluate");
        if let Ok(ranked) = engine.rank(&net, &marking, &enabled) {
            prop_assert_eq!(ranked.len(), enabled.len());
            for r in &ranked {
                prop_assert!(enabled.iter().any(|e| e.transition_id == r.binding.transition_id));
            }
            for w in ranked.windows(2) {
                if w[0].delta_per_minute == w[1].delta_per_minute {
                    prop_assert!(w[0].binding.transition_id <= w[1].binding.transition_id);
                }
                prop_assert!(
                    w[0].delta_per_minute.is_nan()
                        || w[1].delta_per_minute.is_nan()
                        || w[0].delta_per_minute >= w[1].delta_per_minute,
                    "not sorted: {} then {}", w[0].delta_per_minute, w[1].delta_per_minute
                );
            }
        }
    }

    #[test]
    fn validation_passes_for_generated_nets(
        (net, _marking) in arb_net_and_marking()
    ) {
        let errors = net.validate();
        prop_assert!(errors.is_empty(),
            "generated net should be valid but got: {:?}", errors);
    }

    /// Every finite `Value` survives the trip through Rhai that a postcond
    /// value takes, and the accessors agree with the variant.
    #[test]
    fn value_survives_the_rhai_round_trip(value in arb_value()) {
        let back = Value::try_from_dynamic(value.to_rhai_dynamic())
            .expect("a finite value converts back");
        prop_assert_eq!(&back, &value);
        prop_assert_eq!(value.as_f64(), match value {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            _ => None,
        });
        prop_assert_eq!(value.as_str(), match &value {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        });
        prop_assert_eq!(value.as_bool(), match &value {
            Value::Bool(b) => Some(*b),
            _ => None,
        });
        prop_assert_eq!(value.to_rhai_dynamic().is_unit(), matches!(value, Value::Null));
    }
}
