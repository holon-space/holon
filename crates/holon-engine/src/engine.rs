use std::collections::BTreeMap;

use crate::Marking;
use crate::NetDef;
use crate::TokenState;
use crate::TransitionDef;
use crate::guard::RhaiEvaluator;
use crate::value::Value;
use crate::yaml::history::AttrChange;
use crate::yaml::history::CreatedToken;
use crate::yaml::history::Event;

/// A binding of input arc bind-names to actual token ids, plus captured
/// placeholders.
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

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            evaluator: RhaiEvaluator::new(),
        }
    }

    /// Find all enabled transitions with their bindings.
    pub fn enabled<N: NetDef, M: Marking>(
        &self,
        net: &N,
        marking: &M,
    ) -> Result<Vec<Binding>, String> {
        let mut result = Vec::new();
        for transition in net.transitions() {
            if let Some(binding) = self.find_binding(transition, marking)? {
                result.push(binding);
            }
        }
        Ok(result)
    }

    /// Try to find a valid binding for all input arcs of a transition.
    fn find_binding<T: TransitionDef, M: Marking>(
        &self,
        transition: &T,
        marking: &M,
    ) -> Result<Option<Binding>, String> {
        let mut token_bindings = BTreeMap::new();
        let mut bound_tokens = Vec::new();
        let mut placeholders = BTreeMap::new();

        let found = self.bind_arcs(
            transition.inputs(),
            marking,
            &mut token_bindings,
            &mut bound_tokens,
            &mut placeholders,
        )?;

        Ok(found.then(|| Binding {
            transition_id: transition.id().to_string(),
            token_bindings,
            placeholders,
        }))
    }

    /// Backtracking search for a token assignment that satisfies all input
    /// arcs. A greedy first-match pass would starve later arcs when an
    /// earlier arc of the same token type grabs the only token satisfying
    /// the later precond.
    fn bind_arcs<M: Marking>(
        &self,
        arcs: &[crate::InputArc],
        marking: &M,
        token_bindings: &mut BTreeMap<String, String>,
        bound_tokens: &mut Vec<String>,
        placeholders: &mut BTreeMap<String, Value>,
    ) -> Result<bool, String> {
        let Some((arc, rest)) = arcs.split_first() else {
            return Ok(true);
        };
        for (token_id, new_placeholders) in
            self.evaluator
                .matching_tokens(marking, arc, bound_tokens, placeholders)?
        {
            token_bindings.insert(arc.bind.clone(), token_id.clone());
            bound_tokens.push(token_id);
            let saved_placeholders = placeholders.clone();
            placeholders.extend(new_placeholders);
            if self.bind_arcs(rest, marking, token_bindings, bound_tokens, placeholders)? {
                return Ok(true);
            }
            *placeholders = saved_placeholders;
            bound_tokens.pop();
            token_bindings.remove(&arc.bind);
        }
        Ok(false)
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

        // Handle create arcs — inject `step` so id_expr can produce unique IDs per
        // firing
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
    ) -> Result<Vec<RankedTransition>, String> {
        let obj_before = crate::objective::evaluate(&self.evaluator, net, marking)?.value;

        let mut ranked = Vec::with_capacity(enabled.len());
        for (i, binding) in enabled.iter().enumerate() {
            let transition = net
                .transition(&binding.transition_id)
                .ok_or_else(|| format!("unknown transition: {}", binding.transition_id))?;
            let mut sim = marking.clone();
            // Use a high step offset so created-token IDs don't collide with real firings
            self.fire(net, &mut sim, binding, usize::MAX - i)
                .map_err(|e| format!("simulating '{}': {e}", binding.transition_id))?;
            let obj_after = crate::objective::evaluate(&self.evaluator, net, &sim)
                .map_err(|e| {
                    format!(
                        "objective after simulating '{}': {e}",
                        binding.transition_id
                    )
                })?
                .value;
            let delta = obj_after - obj_before;
            let duration = transition.duration_minutes().max(0.001);
            ranked.push(RankedTransition {
                binding: binding.clone(),
                delta_obj: delta,
                delta_per_minute: delta / duration,
            });
        }

        ranked.sort_by(|a, b| {
            b.delta_per_minute
                .partial_cmp(&a.delta_per_minute)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.binding.transition_id.cmp(&b.binding.transition_id))
        });

        Ok(ranked)
    }
}
