//! Load-time refusal of a profile whose widget would `set_field` a field only
//! a structural op may write (Model.md invariants 3 and 16).
//!
//! The field such a widget writes is author data, so it is parsed here, where
//! the profile enters, instead of failing at the first click or drag.

use anyhow::Context;
use anyhow::Result;
use holon_api::Value;
use holon_api::block_write_field::refuse_structural_field;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;

use crate::ParsedProfile;

/// Refuse `profile` if a variant's render names, as a literal, a private field
/// or an order key as the field a widget writes: `board`'s `lane_field`,
/// `state_toggle`'s field, `editable_text`'s `field`.
pub fn check_profile_write_targets(profile: &ParsedProfile) -> Result<()> {
    let entity = &profile.entity_name;
    for variant in &profile.variants {
        let site = format!("profile '{entity}' variant '{}'", variant.name);
        let render = crate::parse_render_text(&variant.render)
            .with_context(|| format!("{site}: render failed to parse"))?;
        check_expr(&render, &site)?;
    }
    Ok(())
}

fn check_expr(expr: &RenderExpr, site: &str) -> Result<()> {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            if let Some((arg, field)) = write_target(name, args) {
                refuse_structural_field(field).map_err(|e| {
                    anyhow::anyhow!(
                        "{site}: `{name}` {arg} \"{field}\" makes the widget set_field it; {e}"
                    )
                })?;
            }
            args.iter().try_for_each(|a| check_expr(&a.value, site))
        }
        RenderExpr::BinaryOp { left, right, .. } => {
            check_expr(left, site)?;
            check_expr(right, site)
        }
        RenderExpr::Array { items } => items.iter().try_for_each(|e| check_expr(e, site)),
        RenderExpr::Object { fields } => fields.values().try_for_each(|e| check_expr(e, site)),
        RenderExpr::LiveBlock { .. }
        | RenderExpr::ColumnRef { .. }
        | RenderExpr::Literal { .. } => Ok(()),
    }
}

/// The argument and literal field name `widget` writes, chosen with the same
/// precedence as its shadow builder.
fn write_target<'a>(widget: &str, args: &'a [Arg]) -> Option<(&'static str, &'a str)> {
    let named = |key: &str| {
        args.iter()
            .find(|a| a.name.as_deref() == Some(key))
            .and_then(|a| literal_str(&a.value))
    };
    let positional = |index: usize| args.iter().filter(|a| a.name.is_none()).nth(index);
    match widget {
        "board" => named("lane_field").map(|f| ("lane_field", f)),
        "state_toggle" => match positional(0).map(|a| &a.value) {
            Some(RenderExpr::ColumnRef { name }) => Some(("field", name.as_str())),
            first => named("field")
                .or_else(|| first.and_then(literal_str))
                .map(|f| ("field", f)),
        },
        "editable_text" => named("field")
            .or_else(|| positional(1).and_then(|a| literal_str(&a.value)))
            .map(|f| ("field", f)),
        _ => None,
    }
}

fn literal_str(expr: &RenderExpr) -> Option<&str> {
    match expr {
        RenderExpr::Literal {
            value: Value::String(s),
        } => Some(s.as_str()),
        _ => None,
    }
}
