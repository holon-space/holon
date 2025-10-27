use crate::guard::RhaiEvaluator;
use crate::{Marking, NetDef};

pub struct ObjectiveResult {
    pub value: f64,
    pub constraint_violations: Vec<String>,
}

pub fn evaluate<N: NetDef, M: Marking>(
    evaluator: &RhaiEvaluator,
    net: &N,
    marking: &M,
) -> Result<ObjectiveResult, String> {
    let mut scope = RhaiEvaluator::build_marking_scope(marking);

    let discount_rate = net.discount_rate();
    if discount_rate > 0.0 {
        scope.push("discount", rhai::Dynamic::from(1.0 / (1.0 + discount_rate)));
    } else {
        scope.push("discount", rhai::Dynamic::from(1.0_f64));
    }

    let value = evaluator.eval_compiled_expr(net.objective_expr(), &mut scope)?;

    let mut violations = Vec::new();
    for constraint in net.constraints() {
        match evaluator.eval_compiled_bool(constraint, &mut scope) {
            Ok(true) => {}
            Ok(false) => violations.push(constraint.source.clone()),
            Err(e) => violations.push(format!("{}: {e}", constraint.source)),
        }
    }

    Ok(ObjectiveResult {
        value,
        constraint_violations: violations,
    })
}
