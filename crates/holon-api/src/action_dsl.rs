//! Action DSL parsing — Tier-1 domain helper (ADR 0006 #2).
//!
//! Parses an action block's source (e.g. `block.create(#{...})`) into a typed
//! [`ParsedAction`]. This is a **pure function over content** with no actor
//! state, so it lives in the domain tier: the Action-engine actor and any other
//! actor call the same parser rather than each owning a private one. Built on
//! the Tier-1 render DSL engine ([`crate::render_dsl`]).

use anyhow::Context;
use anyhow::Result;
use rhai::Expr;
use rhai::Stmt;

use crate::render_dsl::compile_expression;
use crate::render_dsl::render_expr_of;
use crate::render_types::Arg;

/// A parsed action invocation: which entity, which operation, with what args.
/// Each param is a render expression, evaluated against the row that fires.
pub struct ParsedAction {
    pub entity: String,
    pub operation: String,
    pub params: Vec<Arg>,
}

impl ParsedAction {
    /// Every param evaluated against the row that fires, outside any render:
    /// only the core value functions can be called. `Err` names the param.
    pub fn eval_params(
        &self,
        row: &crate::StorageEntity,
    ) -> std::result::Result<crate::StorageEntity, (String, crate::computation::ComputeError)> {
        self.params
            .iter()
            .map(|arg| {
                let name = arg
                    .name
                    .as_ref()
                    .expect("parse_action_dsl names every param");
                crate::render_eval::eval_plain_value(&arg.value, row)
                    .map(|v| (name.as_str().into(), v))
                    .map_err(|e| (name.clone(), e))
            })
            .collect()
    }
}

const ENTITIES: &[&str] = &["block"];

const OPERATIONS: &[&str] = &[
    "create",
    "set_field",
    "update",
    "delete",
    "cycle_task_state",
    // Engine-level compound (docs/Proposals/Templating-2026-07-12.md): a
    // rule effect may instantiate a template subtree. The operation itself
    // owns deterministic ids + fail-loud binding checks.
    "instantiate_template",
];

/// Parse an action DSL expression `<entity>.<operation>(#{param: <render expr>,
/// ...})`.
pub fn parse_action_dsl(source: &str) -> Result<ParsedAction> {
    let trimmed = source.trim();
    let ast = compile_expression(trimmed).context("action DSL")?;
    let shape_error = || {
        anyhow::anyhow!(
            "action DSL '{trimmed}' must be one `<entity>.<operation>(#{{...}})` call with \
             entity in {ENTITIES:?} and operation in {OPERATIONS:?}"
        )
    };
    let [Stmt::Expr(expr)] = ast.statements() else {
        return Err(shape_error());
    };
    let Expr::Dot(dot, ..) = expr.as_ref() else {
        return Err(shape_error());
    };
    let (Expr::Variable(var, ..), Expr::MethodCall(call, _)) = (&dot.lhs, &dot.rhs) else {
        return Err(shape_error());
    };
    let entity = var.1.as_str();
    let operation = call.name.as_str();
    if !ENTITIES.contains(&entity) || !OPERATIONS.contains(&operation) {
        return Err(shape_error());
    }
    let [Expr::Map(map, _)] = call.args.as_slice() else {
        return Err(shape_error());
    };
    let params = map
        .0
        .iter()
        .map(|(key, value)| {
            Ok(Arg {
                name: Some(key.name.to_string()),
                value: render_expr_of(value)
                    .with_context(|| format!("action DSL '{trimmed}', param '{}'", key.name))?,
            })
        })
        .collect::<Result<_>>()?;

    Ok(ParsedAction {
        entity: entity.to_string(),
        operation: operation.to_string(),
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_types::RenderExpr;

    #[test]
    fn an_action_parses_into_entity_operation_and_named_params() {
        let parsed = parse_action_dsl(r#"block.create(#{ content: "hi" })"#).unwrap();
        assert_eq!(parsed.entity, "block");
        assert_eq!(parsed.operation, "create");
        assert_eq!(parsed.params.len(), 1);
        assert_eq!(parsed.params[0].name.as_deref(), Some("content"));

        let parsed = parse_action_dsl(r#"block.set_field(#{ done: true })"#).unwrap();
        assert_eq!(parsed.operation, "set_field");
    }

    #[test]
    fn a_param_over_col_stays_a_per_row_expression() {
        let parsed = parse_action_dsl(
            r#"block.create(#{ content: "Re: " + col("title"), n: col("n") * 2 })"#,
        )
        .unwrap();
        let rhai: Vec<String> = parsed.params.iter().map(|a| a.value.to_rhai()).collect();
        assert!(
            parsed
                .params
                .iter()
                .all(|a| !matches!(a.value, RenderExpr::Literal { .. })),
            "params over col(..) must not be decided at load; got {rhai:?}"
        );
    }

    #[test]
    fn an_unknown_operation_or_a_non_per_row_param_is_refused() {
        for source in [
            r#"block.explode(#{})"#,
            r#"page.create(#{})"#,
            r#"block.create("x")"#,
            r#"block.create(#{ content: col("a").len() })"#,
        ] {
            assert!(
                parse_action_dsl(source).is_err(),
                "{source} must be refused"
            );
        }
    }
}
