use holon_engine::arc::{CreateArc, InputArc, OutputArc};
use holon_engine::engine::Engine;
use holon_engine::guard::RhaiEvaluator;
use holon_engine::value::Value;
use holon_engine::yaml::history::History;
use holon_engine::yaml::net::{ObjectiveDef, YamlNet, YamlTransition};
use holon_engine::yaml::state::{YamlMarking, YamlToken};
use holon_engine::{objective, Marking, TokenState};
use proptest::prelude::*;
use std::collections::BTreeMap;

const TOKEN_TYPES: &[&str] = &["person", "document", "asset", "monetary"];
const STATUSES: &[&str] = &["active", "pending", "done", "available", "missing"];

fn arb_token(id: String) -> impl Strategy<Value = YamlToken> {
    let type_strategy = proptest::sample::select(TOKEN_TYPES);
    let status_strategy = proptest::sample::select(STATUSES);
    (
        type_strategy,
        status_strategy,
        proptest::collection::btree_map("[a-z]{3,6}".prop_map(|s| s), -10.0f64..10.0, 0..3),
    )
        .prop_map(move |(token_type, status, float_attrs)| {
            let mut attributes = BTreeMap::new();
            attributes.insert("status".to_string(), Value::String(status.to_string()));
            for (k, v) in float_attrs {
                if k != "status" {
                    attributes.insert(k, Value::Float(v));
                }
            }
            YamlToken {
                name: id.clone(),
                token_type: token_type.to_string(),
                attributes,
            }
        })
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
                    // 0 = exact match, 1 = placeholder ($status), 2 = numeric comparison
                    let precond_style_strats: Vec<_> = (0..n).map(|_| 0u8..=2u8).collect();
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
                _precond_styles,
                duration,
                n_creates,
            )| {
                let mut inputs = Vec::new();
                let mut outputs = Vec::new();
                for (i, &idx) in selected_indices.iter().enumerate() {
                    let mut precond = BTreeMap::new();
                    precond.insert("status".to_string(), statuses[i].to_string());
                    let consume = consumes[i];
                    inputs.push(InputArc {
                        bind: token_ids[idx].clone(),
                        token_type: token_types[idx].clone(),
                        precond,
                        consume,
                    });
                    let mut postcond = BTreeMap::new();
                    if let Some(post_status) = &postconds[i] {
                        postcond.insert("status".to_string(), format!("\"{}\"", post_status));
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
                            id_expr: format!("\"created-{}-{}-\" + step.n", name, i),
                            token_type: tt,
                            attrs: BTreeMap::from([("status".to_string(), "\"new\"".to_string())]),
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
        .prop_map(|(tokens, transitions)| {
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
                clock: chrono::DateTime::from_timestamp(1_000_000, 0)
                    .unwrap()
                    .into(),
                tokens,
            };

            (net, marking)
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn determinism(
        (net, marking) in arb_net_and_marking()
    ) {
        let engine = Engine::new();
        let enabled = engine.enabled(&net, &marking);

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
        let enabled = engine.enabled(&net, &live);

        let mut fired = 0;
        let mut current_enabled = enabled;
        while fired < 3 && !current_enabled.is_empty() {
            let binding = &current_enabled[0];
            let step = history.next_step();
            match engine.fire(&net, &mut live, binding, step) {
                Ok(event) => {
                    history.append(event);
                    fired += 1;
                }
                Err(_) => break,
            }
            current_enabled = engine.enabled(&net, &live);
        }

        let mut replayed = marking.clone();
        history.replay(&mut replayed);

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

    #[test]
    fn validation_passes_for_generated_nets(
        (net, _marking) in arb_net_and_marking()
    ) {
        let errors = net.validate();
        prop_assert!(errors.is_empty(),
            "generated net should be valid but got: {:?}", errors);
    }

    #[test]
    fn wsjf_beats_random(
        (net, marking) in arb_net_and_marking()
    ) {
        let engine = Engine::new();
        let evaluator = RhaiEvaluator::new();

        let mut wsjf_marking = marking.clone();
        for i in 0..5 {
            let enabled = engine.enabled(&net, &wsjf_marking);
            if enabled.is_empty() { break; }
            let ranked = engine.rank(&net, &wsjf_marking, &enabled);
            if ranked.is_empty() { break; }
            let binding = &ranked[0].binding;
            engine.fire(&net, &mut wsjf_marking, binding, i)
                .expect("fire should succeed for enabled transition");
        }
        let wsjf_obj = objective::evaluate(&evaluator, &net, &wsjf_marking)
            .map(|r| r.value).unwrap_or(f64::NEG_INFINITY);

        let mut lex_marking = marking.clone();
        for i in 0..5 {
            let enabled = engine.enabled(&net, &lex_marking);
            if enabled.is_empty() { break; }
            let binding = enabled.into_iter()
                .min_by_key(|b| b.transition_id.clone())
                .unwrap();
            engine.fire(&net, &mut lex_marking, &binding, i + 100)
                .expect("fire should succeed for enabled transition");
        }
        let lex_obj = objective::evaluate(&evaluator, &net, &lex_marking)
            .map(|r| r.value).unwrap_or(f64::NEG_INFINITY);

        // Skip comparison when objective can't be evaluated (e.g., consumed token
        // referenced in objective expression → evaluation returns -inf)
        if wsjf_obj.is_finite() && lex_obj.is_finite() {
            prop_assert!(wsjf_obj >= lex_obj - 1e-9,
                "WSJF ({wsjf_obj}) should be >= lexicographic ({lex_obj})");
        }
    }
}
