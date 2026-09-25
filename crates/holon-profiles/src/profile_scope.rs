//! Load-time refusal of profile expressions that read a name no row of the
//! profile's entity can carry (ruling D171.a).
//!
//! At eval time such a name is simply absent from the Rhai scope, so the
//! expression is structurally unbound on every row: a variant never matches
//! and a computed field is always `Null`, with nothing logged.

use std::collections::BTreeSet;

use anyhow::Result;
use holon_api::CompiledExpr;
use holon_api::EdgeField;
use holon_api::TypeDefinition;
use holon_api::block::Block;

use crate::ParsedProfile;
use crate::UI_STATE_VARIABLES;

/// Refuse `profile` if any computed field or variant condition reads, without
/// an `is_def_var("x")` or `x != ()` guard, a name that is neither a field of
/// `type_def`, one of the profile's own computed fields, nor a UI-state
/// variable. `type_def` is `None` for a profile whose entity has no registered
/// type.
pub fn check_profile_scope(
    profile: &ParsedProfile,
    type_def: Option<&TypeDefinition>,
) -> Result<()> {
    let known = scope_names(profile, type_def);
    let engine = holon_api::unoptimized_engine();
    for (name, decl) in &profile.computed {
        let compiled = CompiledExpr::compile(&engine, decl.expr.as_str())
            .map_err(|e| anyhow::anyhow!("computed field '{name}' failed to compile: {e}"))?;
        check_expr(
            profile,
            &format!("computed field '{name}'"),
            &compiled,
            &known,
        )?;
    }
    for variant in &profile.variants {
        if let Some(condition) = &variant.condition {
            check_expr(
                profile,
                &format!("variant '{}'", variant.name),
                condition,
                &known,
            )?;
        }
    }
    Ok(())
}

fn scope_names(profile: &ParsedProfile, type_def: Option<&TypeDefinition>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = UI_STATE_VARIABLES.iter().map(|v| v.to_string()).collect();
    names.extend(profile.computed.keys().cloned());
    if let Some(td) = type_def {
        names.extend(td.fields.iter().map(|f| f.name.clone()));
        // Edge fields are row columns of the block read table but not schema
        // fields of its TypeDefinition.
        if td.name == Block::type_definition().name {
            names.extend(EdgeField::ALL.iter().map(|f| f.column().to_string()));
        }
    }
    names
}

fn check_expr(
    profile: &ParsedProfile,
    site: &str,
    expr: &CompiledExpr,
    known: &BTreeSet<String>,
) -> Result<()> {
    let unknown: Vec<String> = holon_api::unguarded_columns(&expr.ast)
        .into_iter()
        .filter(|name| !known.contains(name))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let listed = unknown
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let entity = &profile.entity_name;
    let hint = match hyphenated_key_containing(&expr.source, &unknown) {
        Some(key) => format!(
            "did you mean properties[\"{key}\"]? A hyphenated key is not a Rhai identifier: \
             `{key}` parses as a subtraction"
        ),
        None => {
            let first = &unknown[0];
            format!(
                "read an optional property as properties[\"{first}\"], guard it with \
                 is_def_var(\"{first}\"), or declare the column on type `{entity}`"
            )
        }
    };
    anyhow::bail!(
        "profile '{entity}' {site}: `{source}` reads {listed}, which no `{entity}` row carries \
         (not a declared column, computed field or UI-state variable), so it can never \
         evaluate; {hint}",
        source = expr.source,
    )
}

/// The first `ident-ident(-ident)*` run written without spaces in `source`
/// (outside string literals) that has one of `unknown` as a segment.
fn hyphenated_key_containing(source: &str, unknown: &[String]) -> Option<String> {
    let is_key_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut in_string: Option<char> = None;
    let mut run = String::new();
    let mut runs = Vec::new();
    for c in source.chars().chain(std::iter::once(' ')) {
        if let Some(quote) = in_string {
            if c == quote {
                in_string = None;
            }
            continue;
        }
        if is_key_char(c) {
            run.push(c);
            continue;
        }
        runs.push(std::mem::take(&mut run));
        if c == '"' || c == '\'' {
            in_string = Some(c);
        }
    }
    runs.into_iter().find(|run| {
        let segments: Vec<&str> = run.split('-').collect();
        segments.len() > 1
            && segments.iter().all(|s| !s.is_empty())
            && segments.iter().any(|s| unknown.iter().any(|u| u == s))
    })
}
