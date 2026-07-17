//! Action DSL parsing — Tier-1 domain helper (ADR 0006 #2).
//!
//! Parses an action block's source (e.g. `block.create(#{...})`) into a typed
//! [`ParsedAction`]. This is a **pure function over content** with no actor
//! state, so it lives in the domain tier: the Action-engine actor and any other
//! actor call the same parser rather than each owning a private one. Built on
//! the Tier-1 render DSL engine ([`crate::render_dsl`]).

use anyhow::Context;
use anyhow::Result;
use rhai::Dynamic;
use rhai::Engine as RhaiEngine;
use rhai::Map as RhaiMap;
use rhai::Scope;

use crate::render_dsl::create_render_engine;
use crate::render_dsl::dynamic_to_render_expr;
use crate::render_types::Arg;

/// A parsed action invocation: which entity, which operation, with what args.
pub struct ParsedAction {
    pub entity: String,
    pub operation: String,
    pub params: Vec<Arg>,
}

/// Parse an action DSL expression into a [`ParsedAction`].
pub fn parse_action_dsl(source: &str) -> Result<ParsedAction> {
    let trimmed = source.trim();

    let engine = build_action_engine();
    let mut scope = Scope::new();
    scope.push("block", EntityRef("block".to_string()));
    let result = engine
        .eval_expression_with_scope::<Dynamic>(&mut scope, trimmed)
        .map_err(|e| anyhow::anyhow!("Rhai eval failed for action DSL '{trimmed}': {e}"))?;

    let map = result
        .clone()
        .try_cast::<RhaiMap>()
        .ok_or_else(|| anyhow::anyhow!("Action DSL did not return a map, got: {result:?}"))?;

    let entity = map
        .get("_action_entity")
        .and_then(|v| v.clone().into_string().ok()) // ALLOW(ok): Rhai value type mismatch → None
        .ok_or_else(|| anyhow::anyhow!("Action DSL result missing _action_entity"))?;
    let operation = map
        .get("_action_op")
        .and_then(|v| v.clone().into_string().ok()) // ALLOW(ok): Rhai value type mismatch → None
        .ok_or_else(|| anyhow::anyhow!("Action DSL result missing _action_op"))?;

    let params_map = map
        .get("_action_params")
        .and_then(|v| v.clone().try_cast::<RhaiMap>())
        .unwrap_or_default();

    let mut params: Vec<Arg> = Vec::new();
    for (k, v) in &params_map {
        let expr = dynamic_to_render_expr(v)
            .with_context(|| format!("Failed to convert param '{k}' to RenderExpr"))?;
        params.push(Arg {
            name: Some(k.to_string()),
            value: expr,
        });
    }

    Ok(ParsedAction {
        entity,
        operation,
        params,
    })
}

fn build_action_engine() -> RhaiEngine {
    let mut engine = create_render_engine();

    engine.register_type_with_name::<EntityRef>("EntityRef");

    for op in &[
        "create",
        "set_field",
        "update",
        "delete",
        "cycle_task_state",
        // Engine-level compound (docs/Proposals/Templating-2026-07-12.md): a
        // rule effect may instantiate a template subtree. The operation itself
        // owns deterministic ids + fail-loud binding checks.
        "instantiate_template",
    ] {
        let op_str = op.to_string();
        engine.register_fn(
            *op,
            move |entity: &mut EntityRef, params: Dynamic| -> Dynamic {
                make_action_node(&entity.0, &op_str, params)
            },
        );
    }

    engine
}

fn make_action_node(entity: &str, operation: &str, params: Dynamic) -> Dynamic {
    let params_map = if params.is_map() {
        params.cast::<RhaiMap>()
    } else {
        RhaiMap::new()
    };

    let mut map = RhaiMap::new();
    map.insert("_action_entity".into(), Dynamic::from(entity.to_string()));
    map.insert("_action_op".into(), Dynamic::from(operation.to_string()));
    map.insert("_action_params".into(), Dynamic::from(params_map));
    Dynamic::from(map)
}

// Rhai custom type for dot-notation: block.create(#{...})
#[derive(Clone, Debug)]
struct EntityRef(String);

impl std::fmt::Display for EntityRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityRef({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_action_engine` must register the action operations on the
    /// `EntityRef` custom type. A `Default::default()` Rhai engine lacks
    /// both the type and the `create`/`set_field`/... functions, so parsing
    /// `block.create(#{...})` would fail. Probe a registered fn end-to-end.
    #[test]
    fn build_action_engine_registers_action_ops() {
        let parsed = parse_action_dsl(r#"block.create(#{ content: "hi" })"#)
            .expect("registered `create` op must parse");
        assert_eq!(parsed.entity, "block");
        assert_eq!(parsed.operation, "create");
        assert_eq!(parsed.params.len(), 1);
        assert_eq!(parsed.params[0].name.as_deref(), Some("content"));

        // A second registered op, to pin more than one registration.
        let parsed = parse_action_dsl(r#"block.set_field(#{ done: true })"#)
            .expect("registered `set_field` op must parse");
        assert_eq!(parsed.operation, "set_field");
    }

    /// `Display for EntityRef` must format the wrapped id, not the empty
    /// default string.
    #[test]
    fn entity_ref_display_formats_content() {
        let r = EntityRef("block".to_string());
        assert_eq!(format!("{r}"), "EntityRef(block)");
    }
}
