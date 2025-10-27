use crate::guard::RhaiEvaluator;
use crate::value::Value;
use crate::yaml::history::{AttrChange, CreatedToken, Event};
use crate::{Marking, NetDef, TokenState, TransitionDef};
use std::collections::BTreeMap;

/// A binding of input arc bind-names to actual token ids, plus captured placeholders.
#[derive(Clone, Debug)]
pub struct Binding {
    pub transition_id: String,
    pub token_bindings: BTreeMap<String, String>, // bind_name → token_id
    pub placeholders: BTreeMap<String, Value>,
}

/// A ranked transition with its expected value improvement.
#[derive(Clone, Debug)]
pub struct RankedTransition {
    pub binding: Binding,
    pub delta_obj: f64,
    pub delta_per_minute: f64,
}

pub struct Engine {
    evaluator: RhaiEvaluator,
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            evaluator: RhaiEvaluator::new(),
        }
    }

    /// Find all enabled transitions with their bindings.
    pub fn enabled<N: NetDef, M: Marking>(&self, net: &N, marking: &M) -> Vec<Binding> {
        let mut result = Vec::new();
        for transition in net.transitions() {
            if let Some(binding) = self.find_binding(transition, marking) {
                result.push(binding);
            }
        }
        result
    }

    /// Try to find a valid binding for all input arcs of a transition.
    fn find_binding<T: TransitionDef, M: Marking>(
        &self,
        transition: &T,
        marking: &M,
    ) -> Option<Binding> {
        let mut token_bindings = BTreeMap::new();
        let mut placeholders = BTreeMap::new();
        let mut bound_tokens = Vec::new();

        for arc in transition.inputs() {
            let (token_id, new_placeholders) =
                self.evaluator
                    .find_matching_token(marking, arc, &bound_tokens, &placeholders)?;
            token_bindings.insert(arc.bind.clone(), token_id.clone());
            bound_tokens.push(token_id);
            placeholders.extend(new_placeholders);
        }

        Some(Binding {
            transition_id: transition.id().to_string(),
            token_bindings,
            placeholders,
        })
    }

    /// Fire a transition: apply postconditions, move tokens, record changes.
    pub fn fire<N: NetDef, M: Marking>(
        &self,
        net: &N,
        marking: &mut M,
        binding: &Binding,
        step: usize,
    ) -> Result<Event, String> {
        let transition = net
            .transition(&binding.transition_id)
            .ok_or_else(|| format!("unknown transition: {}", binding.transition_id))?;

        // Build Rhai maps for bound tokens
        let mut rhai_maps: BTreeMap<String, rhai::Map> = BTreeMap::new();
        for (bind_name, token_id) in &binding.token_bindings {
            let token = marking
                .token(token_id)
                .ok_or_else(|| format!("token '{token_id}' not found"))?;
            rhai_maps.insert(bind_name.clone(), RhaiEvaluator::token_to_map(token));
        }

        // Collect changes
        let mut changes = Vec::new();
        let time = marking.clock();

        for output in transition.outputs() {
            let token_id = binding
                .token_bindings
                .get(&output.from)
                .ok_or_else(|| format!("output references unbound name: {}", output.from))?;

            let token = marking
                .token(token_id)
                .ok_or_else(|| format!("token '{token_id}' not found"))?;

            // Apply postconditions
            for (attr, expr) in &output.postcond {
                let old_val = token.get(attr).cloned().unwrap_or(Value::Null);
                let new_val =
                    self.evaluator
                        .eval_postcond(expr, &rhai_maps, &binding.placeholders)?;
                if old_val != new_val {
                    changes.push(AttrChange {
                        token: token_id.clone(),
                        attr: attr.clone(),
                        from: old_val,
                        to: new_val,
                    });
                }
            }
        }

        for change in &changes {
            marking.set_attr(&change.token, &change.attr, change.to.clone());
        }

        // Handle create arcs — inject `step` so id_expr can produce unique IDs per firing
        let mut create_maps = rhai_maps.clone();
        let mut step_map = rhai::Map::new();
        step_map.insert("n".into(), rhai::Dynamic::from(step as i64));
        create_maps.insert("step".into(), step_map);

        let mut created = Vec::new();
        for create_arc in transition.creates() {
            let new_id = self
                .evaluator
                .eval_postcond(&create_arc.id_expr, &create_maps, &binding.placeholders)?
                .to_string();
            let mut attrs = BTreeMap::new();
            for (attr, expr) in &create_arc.attrs {
                let val = self
                    .evaluator
                    .eval_postcond(expr, &rhai_maps, &binding.placeholders)?;
                attrs.insert(attr.clone(), val);
            }
            marking.create_token(new_id.clone(), create_arc.token_type.clone(), attrs.clone());
            created.push(CreatedToken {
                id: new_id,
                token_type: create_arc.token_type.clone(),
                attrs,
            });
        }

        // Handle consume arcs
        let mut removed = Vec::new();
        for input in transition.inputs() {
            if input.consume {
                let token_id = binding
                    .token_bindings
                    .get(&input.bind)
                    .expect("consumed input must be bound");
                marking.remove_token(token_id);
                removed.push(token_id.clone());
            }
        }

        // Advance clock
        let duration = transition.duration_minutes();
        marking.set_clock(time + chrono::Duration::minutes(duration as i64));

        Ok(Event {
            step,
            time,
            transition: binding.transition_id.clone(),
            duration,
            changes,
            created,
            removed,
        })
    }

    /// Rank enabled transitions by Δobj/duration (WSJF).
    pub fn rank<N: NetDef, M: Marking>(
        &self,
        net: &N,
        marking: &M,
        enabled: &[Binding],
    ) -> Vec<RankedTransition> {
        let obj_before = crate::objective::evaluate(&self.evaluator, net, marking)
            .map(|r| r.value)
            .unwrap_or(0.0);

        let mut ranked: Vec<RankedTransition> = enabled
            .iter()
            .enumerate()
            .filter_map(|(i, binding)| {
                let transition = net.transition(&binding.transition_id)?;
                let mut sim = marking.clone();
                // Use a high step offset so created-token IDs don't collide with real firings
                if self.fire(net, &mut sim, binding, usize::MAX - i).is_err() {
                    return None;
                }
                let obj_after = crate::objective::evaluate(&self.evaluator, net, &sim)
                    .map(|r| r.value)
                    .unwrap_or(0.0);
                let delta = obj_after - obj_before;
                let duration = transition.duration_minutes().max(0.001);
                Some(RankedTransition {
                    binding: binding.clone(),
                    delta_obj: delta,
                    delta_per_minute: delta / duration,
                })
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.delta_per_minute
                .partial_cmp(&a.delta_per_minute)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.binding.transition_id.cmp(&b.binding.transition_id))
        });

        ranked
    }
}
