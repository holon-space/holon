//! SQL parsing, dependency extraction, and SQL-level transformations using sqlparser.
//!
//! All public functions operate on parsed `Vec<Statement>` ASTs.
//! Use `parse_sql` / `sql_to_string` at the boundaries.
//!
//! ## SqlTransformer trait
//!
//! All post-compilation SQL transformations (entity_name injection, _change_origin,
//! JSON aggregation for UNIONs) are implemented as `SqlTransformer` trait objects.
//! They are registered in DI and applied uniformly regardless of source language
//! (PRQL, SQL, GQL).

use std::collections::HashSet;

use sqlparser::ast::helpers::attached_token::AttachedToken;
use sqlparser::ast::{
    Cte, Expr, FunctionArg, FunctionArgExpr, Ident, ObjectName, ObjectNamePart, Query, Select,
    SelectItem, SetExpr, SetOperator, SetQuantifier, Statement, TableAlias, TableFactor,
    TableWithJoins, Value as SqlValue, With,
};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;
pub use sqlparser::parser::ParserError;

use super::resource::Resource;

// =============================================================================
// Parse / render boundary
// =============================================================================

pub fn parse_sql(sql: &str) -> Result<Vec<Statement>, ParserError> {
    let dialect = SQLiteDialect {};
    Parser::parse_sql(&dialect, sql)
}

pub fn sql_to_string(stmts: &[Statement]) -> String {
    stmts
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

// =============================================================================
// Dependency extraction (AST-based)
// =============================================================================

pub fn extract_table_refs(stmts: &[Statement]) -> Vec<Resource> {
    let mut refs = Vec::new();
    for stmt in stmts {
        extract_refs_from_statement(stmt, &mut refs);
    }
    refs.sort_by(|a, b| a.name().cmp(b.name()));
    refs.dedup();
    refs
}

pub fn extract_created_tables(stmts: &[Statement]) -> Vec<Resource> {
    let mut created = Vec::new();
    for stmt in stmts {
        match stmt {
            Statement::CreateTable(ct) => {
                created.push(Resource::schema(normalize_table_name(&ct.name.to_string())));
            }
            Statement::CreateView(cv) => {
                created.push(Resource::schema(normalize_table_name(&cv.name.to_string())));
            }
            Statement::CreateIndex(ci) => {
                if let Some(idx_name) = &ci.name {
                    created.push(Resource::schema(normalize_table_name(
                        &idx_name.to_string(),
                    )));
                }
            }
            _ => {}
        }
    }
    created
}

// =============================================================================
// Entity name injection
// =============================================================================

/// Convenience: parse SQL string, inject entity_name, return modified SQL string.
/// Returns the original string unchanged if parsing fails.
pub fn inject_entity_name_into_sql(sql: &str) -> String {
    match parse_sql(sql) {
        Ok(mut stmts) => {
            inject_entity_name(&mut stmts);
            sql_to_string(&stmts)
        }
        Err(_) => sql.to_string(),
    }
}

/// Inject `'<table>' AS entity_name` into every SELECT projection.
///
/// Mutates statements in place. For UNION queries, each branch gets the
/// entity_name of its own primary table. CTE names are excluded from
/// resolution (they aren't entity types). Idempotent: skips SELECTs
/// that already project `entity_name`.
pub fn inject_entity_name(stmts: &mut [Statement]) {
    for stmt in stmts.iter_mut() {
        if let Statement::Query(query) = stmt {
            let mut cte_names = HashSet::new();
            let is_recursive = query.with.as_ref().is_some_and(|w| w.recursive);
            if let Some(with) = &query.with {
                for cte in &with.cte_tables {
                    cte_names.insert(cte.alias.name.value.to_lowercase());
                }
            }
            // Skip recursive CTE bodies — UNION ALL branches must have matching column counts
            if !is_recursive {
                if let Some(with) = &mut query.with {
                    for cte in &mut with.cte_tables {
                        inject_into_query(&mut cte.query, &cte_names);
                    }
                }
            }
            inject_into_set_expr(&mut query.body, &cte_names);
        }
    }
}

fn inject_into_query(query: &mut Query, cte_names: &HashSet<String>) {
    let mut all_cte_names = cte_names.clone();
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            all_cte_names.insert(cte.alias.name.value.to_lowercase());
        }
    }
    if let Some(with) = &mut query.with {
        for cte in &mut with.cte_tables {
            inject_into_query(&mut cte.query, &all_cte_names);
        }
    }
    inject_into_set_expr(&mut query.body, &all_cte_names);
}

fn inject_into_set_expr(set_expr: &mut SetExpr, cte_names: &HashSet<String>) {
    match set_expr {
        SetExpr::Select(select) => {
            if has_entity_name_in_projection(&select.projection) {
                return;
            }
            if let Some(table_name) = get_primary_table_name(select, cte_names) {
                select.projection.push(SelectItem::ExprWithAlias {
                    expr: Expr::Value(SqlValue::SingleQuotedString(table_name).into()),
                    alias: Ident::new("entity_name"),
                });
            } else if is_primary_table_cte(select, cte_names) && !has_wildcard(&select.projection) {
                // Primary table is a CTE whose body already has entity_name injected.
                // Add a column reference so the outer SELECT doesn't drop it.
                select
                    .projection
                    .push(SelectItem::UnnamedExpr(Expr::Identifier(Ident::new(
                        "entity_name",
                    ))));
            }
        }
        SetExpr::SetOperation { left, right, .. } => {
            inject_into_set_expr(left, cte_names);
            inject_into_set_expr(right, cte_names);
        }
        SetExpr::Query(query) => {
            inject_into_query(query, cte_names);
        }
        _ => {}
    }
}

fn has_entity_name_in_projection(projection: &[SelectItem]) -> bool {
    projection.iter().any(|item| match item {
        SelectItem::ExprWithAlias { alias, .. } => alias.value.eq_ignore_ascii_case("entity_name"),
        SelectItem::UnnamedExpr(Expr::Identifier(ident)) => {
            ident.value.eq_ignore_ascii_case("entity_name")
        }
        _ => false,
    })
}

fn is_primary_table_cte(select: &Select, cte_names: &HashSet<String>) -> bool {
    let Some(first_from) = select.from.first() else {
        return false;
    };
    match &first_from.relation {
        TableFactor::Table { name, .. } => {
            let table_name = normalize_table_name(&name.to_string());
            cte_names.contains(&table_name.to_lowercase())
        }
        _ => false,
    }
}

fn has_wildcard(projection: &[SelectItem]) -> bool {
    projection.iter().any(|item| {
        matches!(
            item,
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(..)
        )
    })
}

/// Resolve the entity name for a SELECT.
///
/// Prefers the table that provides the selected columns (dominant qualifier)
/// over the first FROM table. This handles GQL-compiled SQL where the first
/// FROM is a matview like `current_focus` but all RETURN columns come from
/// `blocks` (via JOINs).
fn get_primary_table_name(select: &Select, cte_names: &HashSet<String>) -> Option<String> {
    // Try dominant qualifier first: if all qualified columns reference the same alias,
    // resolve that alias to a real table name.
    if let Some(name) = get_dominant_qualifier_table(select, cte_names) {
        return Some(name);
    }
    let first_from = select.from.first()?;
    resolve_table_factor(&first_from.relation, cte_names)
}

/// If all qualified columns in the SELECT projection reference the same table alias,
/// resolve that alias to the real table name by looking through FROM + JOINs.
fn get_dominant_qualifier_table(select: &Select, cte_names: &HashSet<String>) -> Option<String> {
    let mut qualifiers: HashSet<String> = HashSet::new();
    for item in &select.projection {
        match item {
            SelectItem::ExprWithAlias { expr, .. } | SelectItem::UnnamedExpr(expr) => {
                if let Some(q) = extract_qualifier(expr) {
                    qualifiers.insert(q);
                }
            }
            _ => {}
        }
    }
    if qualifiers.len() != 1 {
        return None;
    }
    let alias = qualifiers.into_iter().next()?;
    resolve_alias_to_table(select, &alias, cte_names)
}

/// Extract the table qualifier from a compound identifier (e.g., `_v4."id"` → `_v4`).
fn extract_qualifier(expr: &Expr) -> Option<String> {
    if let Expr::CompoundIdentifier(parts) = expr {
        if parts.len() == 2 {
            return Some(parts[0].value.clone());
        }
    }
    None
}

/// Look up a table alias in FROM + JOINs to find the real table name.
fn resolve_alias_to_table(
    select: &Select,
    alias: &str,
    cte_names: &HashSet<String>,
) -> Option<String> {
    for from_item in &select.from {
        if let Some(name) = check_table_factor_for_alias(&from_item.relation, alias, cte_names) {
            return Some(name);
        }
        for join in &from_item.joins {
            if let Some(name) = check_table_factor_for_alias(&join.relation, alias, cte_names) {
                return Some(name);
            }
        }
    }
    None
}

/// Check if a table factor has the given alias and return the real table name.
fn check_table_factor_for_alias(
    factor: &TableFactor,
    alias: &str,
    cte_names: &HashSet<String>,
) -> Option<String> {
    if let TableFactor::Table {
        name,
        alias: Some(table_alias),
        ..
    } = factor
    {
        if table_alias.name.value == alias {
            let table_name = normalize_table_name(&name.to_string());
            if !cte_names.contains(&table_name.to_lowercase()) {
                return Some(table_name);
            }
        }
    }
    None
}

/// Returns alias if present, otherwise real table name. For column qualification in SQL.
fn get_primary_table_name_aliased(select: &Select, cte_names: &HashSet<String>) -> Option<String> {
    let first_from = select.from.first()?;
    resolve_table_factor_aliased(&first_from.relation, cte_names)
}

/// Get both the real table name and the alias for _change_origin injection.
/// If columns are all qualified with a single JOIN alias, returns that table's
/// real name and alias. Otherwise falls back to the primary FROM table.
fn get_change_origin_table_and_alias(
    select: &Select,
    cte_names: &HashSet<String>,
) -> (Option<String>, Option<String>) {
    // Try dominant qualifier: if all qualified columns reference the same alias,
    // use that alias's real table name.
    if let Some(dominant_alias) = get_dominant_qualifier(select) {
        if let Some(real_name) = resolve_alias_to_table(select, &dominant_alias, cte_names) {
            return (Some(real_name), Some(dominant_alias));
        }
    }
    // Fall back to primary FROM table
    let real_name = select
        .from
        .first()
        .and_then(|f| resolve_table_factor(&f.relation, cte_names));
    let aliased = select
        .from
        .first()
        .and_then(|f| resolve_table_factor_aliased(&f.relation, cte_names));
    (real_name, aliased)
}

/// Extract the dominant qualifier from SELECT projection (if all qualified columns
/// reference the same alias). Returns None if there are mixed or no qualifiers.
fn get_dominant_qualifier(select: &Select) -> Option<String> {
    let mut qualifiers: HashSet<String> = HashSet::new();
    for item in &select.projection {
        match item {
            SelectItem::ExprWithAlias { expr, .. } | SelectItem::UnnamedExpr(expr) => {
                if let Some(q) = extract_qualifier(expr) {
                    qualifiers.insert(q);
                }
            }
            _ => {}
        }
    }
    if qualifiers.len() == 1 {
        qualifiers.into_iter().next()
    } else {
        None
    }
}

fn resolve_table_factor(factor: &TableFactor, cte_names: &HashSet<String>) -> Option<String> {
    resolve_table_factor_inner(factor, cte_names, false)
}

/// Like `resolve_table_factor` but returns the alias when present (for column qualification).
fn resolve_table_factor_aliased(
    factor: &TableFactor,
    cte_names: &HashSet<String>,
) -> Option<String> {
    resolve_table_factor_inner(factor, cte_names, true)
}

fn resolve_table_factor_inner(
    factor: &TableFactor,
    cte_names: &HashSet<String>,
    prefer_alias: bool,
) -> Option<String> {
    match factor {
        TableFactor::Table { name, alias, .. } => {
            let table_name = normalize_table_name(&name.to_string());
            if cte_names.contains(&table_name.to_lowercase()) {
                None
            } else if prefer_alias {
                if let Some(a) = alias {
                    Some(a.name.value.clone())
                } else {
                    Some(table_name)
                }
            } else {
                Some(table_name)
            }
        }
        TableFactor::Derived { subquery, .. } => {
            resolve_query_table_inner(subquery, cte_names, prefer_alias)
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => resolve_table_factor_inner(&table_with_joins.relation, cte_names, prefer_alias),
        _ => None,
    }
}

fn resolve_query_table(query: &Query, cte_names: &HashSet<String>) -> Option<String> {
    resolve_query_table_inner(query, cte_names, false)
}

fn resolve_query_table_inner(
    query: &Query,
    cte_names: &HashSet<String>,
    prefer_alias: bool,
) -> Option<String> {
    match query.body.as_ref() {
        SetExpr::Select(select) => {
            let first_from = select.from.first()?;
            resolve_table_factor_inner(&first_from.relation, cte_names, prefer_alias)
        }
        SetExpr::Query(inner) => resolve_query_table_inner(inner, cte_names, prefer_alias),
        _ => None,
    }
}

// =============================================================================
// Internal helpers (unchanged)
// =============================================================================

fn normalize_table_name(name: &str) -> String {
    name.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
        .trim_matches('[')
        .trim_matches(']')
        .split('.')
        .last()
        .unwrap_or(name)
        .to_string()
}

fn extract_refs_from_statement(stmt: &Statement, refs: &mut Vec<Resource>) {
    match stmt {
        Statement::Query(query) => {
            extract_refs_from_query(query, refs);
        }
        Statement::Insert(insert) => {
            refs.push(Resource::schema(normalize_table_name(
                &insert.table.to_string(),
            )));
            if let Some(src) = &insert.source {
                extract_refs_from_query(src, refs);
            }
        }
        Statement::Update(update) => {
            extract_refs_from_table_with_joins(&update.table, refs);
            if let Some(from_clause) = &update.from {
                match from_clause {
                    sqlparser::ast::UpdateTableFromKind::BeforeSet(tables) => {
                        for table in tables {
                            extract_refs_from_table_with_joins(table, refs);
                        }
                    }
                    sqlparser::ast::UpdateTableFromKind::AfterSet(tables) => {
                        for table in tables {
                            extract_refs_from_table_with_joins(table, refs);
                        }
                    }
                }
            }
        }
        Statement::Delete(delete) => {
            for table in &delete.tables {
                refs.push(Resource::schema(normalize_table_name(&table.to_string())));
            }
            match &delete.from {
                sqlparser::ast::FromTable::WithFromKeyword(tables)
                | sqlparser::ast::FromTable::WithoutKeyword(tables) => {
                    for table in tables {
                        extract_refs_from_table_with_joins(table, refs);
                    }
                }
            }
        }
        Statement::CreateView(cv) => {
            extract_refs_from_query(&cv.query, refs);
        }
        Statement::CreateTable(ct) => {
            if let Some(q) = &ct.query {
                extract_refs_from_query(q, refs);
            }
        }
        _ => {}
    }
}

fn extract_refs_from_query(query: &Query, refs: &mut Vec<Resource>) {
    let mut cte_names: HashSet<String> = HashSet::new();
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            cte_names.insert(cte.alias.name.value.to_lowercase());
        }
    }

    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            extract_refs_from_query_with_ctes(&cte.query, refs, &cte_names);
        }
    }

    extract_refs_from_set_expr_with_ctes(&query.body, refs, &cte_names);
}

fn extract_refs_from_query_with_ctes(
    query: &Query,
    refs: &mut Vec<Resource>,
    cte_names: &HashSet<String>,
) {
    let mut all_cte_names = cte_names.clone();
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            all_cte_names.insert(cte.alias.name.value.to_lowercase());
        }
    }

    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            extract_refs_from_query_with_ctes(&cte.query, refs, &all_cte_names);
        }
    }

    extract_refs_from_set_expr_with_ctes(&query.body, refs, &all_cte_names);
}

fn extract_refs_from_set_expr_with_ctes(
    set_expr: &SetExpr,
    refs: &mut Vec<Resource>,
    cte_names: &HashSet<String>,
) {
    match set_expr {
        SetExpr::Select(select) => {
            extract_refs_from_select_with_ctes(select, refs, cte_names);
        }
        SetExpr::Query(query) => {
            extract_refs_from_query_with_ctes(query, refs, cte_names);
        }
        SetExpr::SetOperation { left, right, .. } => {
            extract_refs_from_set_expr_with_ctes(left, refs, cte_names);
            extract_refs_from_set_expr_with_ctes(right, refs, cte_names);
        }
        SetExpr::Values(_) => {}
        SetExpr::Insert(stmt) => {
            extract_refs_from_statement(stmt, refs);
        }
        SetExpr::Update(stmt) => {
            extract_refs_from_statement(stmt, refs);
        }
        SetExpr::Delete(stmt) => {
            extract_refs_from_statement(stmt, refs);
        }
        SetExpr::Merge(stmt) => {
            extract_refs_from_statement(stmt, refs);
        }
        SetExpr::Table(_) => {}
    }
}

fn extract_refs_from_select_with_ctes(
    select: &Select,
    refs: &mut Vec<Resource>,
    cte_names: &HashSet<String>,
) {
    for table in &select.from {
        extract_refs_from_table_with_joins_with_ctes(table, refs, cte_names);
    }

    for item in &select.projection {
        if let SelectItem::ExprWithAlias { expr, .. } | SelectItem::UnnamedExpr(expr) = item {
            extract_refs_from_expr_with_ctes(expr, refs, cte_names);
        }
    }

    if let Some(selection) = &select.selection {
        extract_refs_from_expr_with_ctes(selection, refs, cte_names);
    }

    if let Some(having) = &select.having {
        extract_refs_from_expr_with_ctes(having, refs, cte_names);
    }
}

fn extract_refs_from_table_with_joins(table: &TableWithJoins, refs: &mut Vec<Resource>) {
    extract_refs_from_table_with_joins_with_ctes(table, refs, &HashSet::new());
}

fn extract_refs_from_table_with_joins_with_ctes(
    table: &TableWithJoins,
    refs: &mut Vec<Resource>,
    cte_names: &HashSet<String>,
) {
    extract_refs_from_table_factor_with_ctes(&table.relation, refs, cte_names);

    for join in &table.joins {
        extract_refs_from_table_factor_with_ctes(&join.relation, refs, cte_names);
    }
}

fn extract_refs_from_table_factor_with_ctes(
    factor: &TableFactor,
    refs: &mut Vec<Resource>,
    cte_names: &HashSet<String>,
) {
    match factor {
        TableFactor::Table { name, .. } => {
            let table_name = normalize_table_name(&name.to_string());
            if !cte_names.contains(&table_name.to_lowercase()) {
                refs.push(Resource::schema(table_name));
            }
        }
        TableFactor::Derived { subquery, .. } => {
            extract_refs_from_query_with_ctes(subquery, refs, cte_names);
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => {
            extract_refs_from_table_with_joins_with_ctes(table_with_joins, refs, cte_names);
        }
        _ => {}
    }
}

fn extract_refs_from_expr(expr: &Expr, refs: &mut Vec<Resource>) {
    extract_refs_from_expr_with_ctes(expr, refs, &HashSet::new());
}

fn extract_refs_from_expr_with_ctes(
    expr: &Expr,
    refs: &mut Vec<Resource>,
    cte_names: &HashSet<String>,
) {
    match expr {
        Expr::Subquery(query) => {
            extract_refs_from_query_with_ctes(query, refs, cte_names);
        }
        Expr::InSubquery { subquery, expr, .. } => {
            extract_refs_from_expr_with_ctes(expr, refs, cte_names);
            extract_refs_from_query_with_ctes(subquery, refs, cte_names);
        }
        Expr::Exists { subquery, .. } => {
            extract_refs_from_query_with_ctes(subquery, refs, cte_names);
        }
        Expr::BinaryOp { left, right, .. } => {
            extract_refs_from_expr_with_ctes(left, refs, cte_names);
            extract_refs_from_expr_with_ctes(right, refs, cte_names);
        }
        Expr::UnaryOp { expr, .. } => {
            extract_refs_from_expr_with_ctes(expr, refs, cte_names);
        }
        Expr::Nested(inner) => {
            extract_refs_from_expr_with_ctes(inner, refs, cte_names);
        }
        Expr::Function(func) => match &func.args {
            sqlparser::ast::FunctionArguments::List(arg_list) => {
                for arg in &arg_list.args {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) = arg {
                        extract_refs_from_expr_with_ctes(e, refs, cte_names);
                    }
                }
            }
            sqlparser::ast::FunctionArguments::Subquery(query) => {
                extract_refs_from_query_with_ctes(query, refs, cte_names);
            }
            sqlparser::ast::FunctionArguments::None => {}
        },
        Expr::Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            if let Some(op) = operand {
                extract_refs_from_expr_with_ctes(op, refs, cte_names);
            }
            for case_when in conditions {
                extract_refs_from_expr_with_ctes(&case_when.condition, refs, cte_names);
                extract_refs_from_expr_with_ctes(&case_when.result, refs, cte_names);
            }
            if let Some(else_res) = else_result {
                extract_refs_from_expr_with_ctes(else_res, refs, cte_names);
            }
        }
        _ => {}
    }
}

// =============================================================================
// SqlTransformer trait + implementations
// =============================================================================

/// A SQL-level transformation applied after compilation (PRQL/GQL/SQL → SQL).
///
/// All transformers operate on parsed `Vec<Statement>` ASTs and are applied
/// in priority order (lower values run first). They must be idempotent.
pub trait SqlTransformer: Send + Sync {
    fn name(&self) -> &'static str;
    fn priority(&self) -> i32;
    fn transform(&self, stmts: &mut Vec<Statement>);
}

/// Injects `'<table>' AS entity_name` into every SELECT projection (priority 10).
///
/// Wraps the existing `inject_entity_name()` logic.
pub struct EntityNameInjector;

impl SqlTransformer for EntityNameInjector {
    fn name(&self) -> &'static str {
        "EntityNameInjector"
    }
    fn priority(&self) -> i32 {
        10
    }
    fn transform(&self, stmts: &mut Vec<Statement>) {
        inject_entity_name(stmts);
    }
}

/// Injects `table._change_origin` column reference into SELECT projections (priority 20).
///
/// For SELECTs with explicit column lists, adds `table._change_origin` as a qualified
/// column reference. For `SELECT *`, skips (already included). Idempotent.
pub struct ChangeOriginInjector;

impl SqlTransformer for ChangeOriginInjector {
    fn name(&self) -> &'static str {
        "ChangeOriginInjector"
    }
    fn priority(&self) -> i32 {
        20
    }
    fn transform(&self, stmts: &mut Vec<Statement>) {
        inject_change_origin(stmts);
    }
}

/// For UNION queries, wraps each branch in a CTE with `SELECT json_object(*) AS data` (priority 30).
///
/// This ensures heterogeneous UNION branches (with different column sets) are normalized
/// into a single `data` JSON column before being combined.
pub struct JsonAggregationSqlTransformer;

impl SqlTransformer for JsonAggregationSqlTransformer {
    fn name(&self) -> &'static str {
        "JsonAggregationSqlTransformer"
    }
    fn priority(&self) -> i32 {
        30
    }
    fn transform(&self, stmts: &mut Vec<Statement>) {
        inject_json_aggregation(stmts);
    }
}

/// Apply all registered SQL transformers in priority order.
///
/// Parses SQL, applies transforms, and returns the modified SQL string.
/// Returns the original string unchanged if parsing fails.
pub fn apply_sql_transforms(sql: &str, transformers: &[Box<dyn SqlTransformer>]) -> String {
    match parse_sql(sql) {
        Ok(mut stmts) => {
            for t in transformers {
                t.transform(&mut stmts);
            }
            sql_to_string(&stmts)
        }
        Err(_) => sql.to_string(),
    }
}

// =============================================================================
// _change_origin injection
// =============================================================================

/// Tables known to have the `_change_origin` column.
/// Only base tables managed by Holon's Entity system have this column.
/// Materialized views, CTEs, and other derived tables do not.
const TABLES_WITH_CHANGE_ORIGIN: &[&str] = &[
    "block",
    "document",
    "directory",
    "file",
    "operation",
    "todoist_task",
    "todoist_project",
];

fn has_change_origin_column(table_name: &str) -> bool {
    TABLES_WITH_CHANGE_ORIGIN
        .iter()
        .any(|t| t.eq_ignore_ascii_case(table_name))
}

/// Inject `table._change_origin` into every SELECT projection.
///
/// For `SELECT *`, skips (already included). For explicit columns, adds
/// `table._change_origin` as a qualified column reference. Idempotent.
/// Only injects for base tables known to have the `_change_origin` column.
fn inject_change_origin(stmts: &mut [Statement]) {
    for stmt in stmts.iter_mut() {
        if let Statement::Query(query) = stmt {
            let mut cte_names = HashSet::new();
            let is_recursive = query.with.as_ref().is_some_and(|w| w.recursive);
            if let Some(with) = &query.with {
                for cte in &with.cte_tables {
                    cte_names.insert(cte.alias.name.value.to_lowercase());
                }
            }
            // Skip recursive CTE bodies — UNION ALL branches must have matching column counts
            if !is_recursive {
                if let Some(with) = &mut query.with {
                    for cte in &mut with.cte_tables {
                        inject_change_origin_into_query(&mut cte.query, &cte_names);
                    }
                }
            }
            inject_change_origin_into_set_expr(&mut query.body, &cte_names);
        }
    }
}

fn inject_change_origin_into_query(query: &mut Query, cte_names: &HashSet<String>) {
    let mut all_cte_names = cte_names.clone();
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            all_cte_names.insert(cte.alias.name.value.to_lowercase());
        }
    }
    if let Some(with) = &mut query.with {
        for cte in &mut with.cte_tables {
            inject_change_origin_into_query(&mut cte.query, &all_cte_names);
        }
    }
    inject_change_origin_into_set_expr(&mut query.body, &all_cte_names);
}

fn inject_change_origin_into_set_expr(set_expr: &mut SetExpr, cte_names: &HashSet<String>) {
    match set_expr {
        SetExpr::Select(select) => {
            // Skip if already has _change_origin or if projection is SELECT *
            if has_change_origin_alias(&select.projection) || has_wildcard(&select.projection) {
                return;
            }
            // Resolve both the real table name (for has_change_origin check)
            // and the alias to use for qualification. Both must come from the
            // same table — if a dominant qualifier points to a JOINed table,
            // use that table's alias, not the first FROM alias.
            let (real_name, table_ref) = get_change_origin_table_and_alias(select, cte_names);
            if let Some(ref name) = real_name {
                if !has_change_origin_column(name) {
                    return;
                }
            }
            if let Some(table_ref) = table_ref {
                select.projection.push(SelectItem::ExprWithAlias {
                    expr: Expr::CompoundIdentifier(vec![
                        Ident::new(table_ref),
                        Ident::new("_change_origin"),
                    ]),
                    alias: Ident::new("_change_origin"),
                });
            }
        }
        SetExpr::SetOperation { left, right, .. } => {
            inject_change_origin_into_set_expr(left, cte_names);
            inject_change_origin_into_set_expr(right, cte_names);
        }
        SetExpr::Query(query) => {
            inject_change_origin_into_query(query, cte_names);
        }
        _ => {}
    }
}

fn has_change_origin_alias(projection: &[SelectItem]) -> bool {
    projection.iter().any(|item| match item {
        SelectItem::ExprWithAlias { alias, .. } => {
            alias.value.eq_ignore_ascii_case("_change_origin")
        }
        _ => false,
    })
}

// =============================================================================
// JSON aggregation for UNIONs
// =============================================================================

/// For UNION queries, wrap each branch in a CTE with `SELECT json_object(*) AS data`.
fn inject_json_aggregation(stmts: &mut Vec<Statement>) {
    let len = stmts.len();
    for i in 0..len {
        if let Statement::Query(query) = &mut stmts[i] {
            if !matches!(query.body.as_ref(), SetExpr::SetOperation { .. }) {
                continue;
            }
            // Collect branches and operators from the UNION tree
            let mut branches: Vec<Box<SetExpr>> = Vec::new();
            let mut operators: Vec<SetOperator> = Vec::new();
            collect_union_branches(&query.body, &mut branches, &mut operators);

            if branches.len() < 2 {
                continue;
            }

            // Build CTEs: preserve existing ones, add _branch_N for each UNION branch
            let mut all_ctes: Vec<Cte> = query
                .with
                .as_ref()
                .map(|w| w.cte_tables.clone())
                .unwrap_or_default();

            for (idx, branch) in branches.iter().enumerate() {
                all_ctes.push(make_cte(
                    &format!("_branch_{}", idx),
                    Query {
                        with: None,
                        body: branch.clone(),
                        order_by: None,
                        limit_clause: None,
                        fetch: None,
                        locks: vec![],
                        for_clause: None,
                        settings: None,
                        format_clause: None,
                        pipe_operators: vec![],
                    },
                ));
            }

            // Build new body: UNION ALL over `SELECT json_object(*) AS data FROM _branch_N`
            let mut new_body = make_json_select_from_cte("_branch_0");
            for (idx, op) in operators.iter().enumerate() {
                new_body = SetExpr::SetOperation {
                    op: *op,
                    set_quantifier: SetQuantifier::All,
                    left: Box::new(new_body),
                    right: Box::new(make_json_select_from_cte(&format!("_branch_{}", idx + 1))),
                };
            }

            query.with = Some(With {
                with_token: AttachedToken::empty(),
                recursive: false,
                cte_tables: all_ctes,
            });
            query.body = Box::new(new_body);
        }
    }
}

/// Recursively collect all leaf branches and operators from a nested SetOperation tree.
fn collect_union_branches(
    body: &SetExpr,
    branches: &mut Vec<Box<SetExpr>>,
    operators: &mut Vec<SetOperator>,
) {
    match body {
        SetExpr::SetOperation {
            op, left, right, ..
        } => {
            collect_union_branches(left, branches, operators);
            operators.push(*op);
            if matches!(right.as_ref(), SetExpr::SetOperation { .. }) {
                collect_union_branches(right, branches, operators);
            } else {
                branches.push(right.clone());
            }
        }
        _ => branches.push(Box::new(body.clone())),
    }
}

fn make_cte(name: &str, query: Query) -> Cte {
    Cte {
        alias: TableAlias {
            explicit: false,
            name: Ident::new(name),
            columns: vec![],
        },
        query: Box::new(query),
        from: None,
        materialized: None,
        closing_paren_token: AttachedToken::empty(),
    }
}

fn make_ident_name(name: &str) -> ObjectName {
    ObjectName(vec![ObjectNamePart::Identifier(Ident::new(name))])
}

/// Build `SELECT json_object(*) AS data FROM <cte_name>`
fn make_json_select_from_cte(cte_name: &str) -> SetExpr {
    SetExpr::Select(Box::new(Select {
        select_token: AttachedToken::empty(),
        optimizer_hint: None,
        distinct: None,
        select_modifiers: None,
        top: None,
        top_before_distinct: false,
        projection: vec![SelectItem::ExprWithAlias {
            expr: Expr::Function(sqlparser::ast::Function {
                name: make_ident_name("json_object"),
                uses_odbc_syntax: false,
                args: sqlparser::ast::FunctionArguments::List(
                    sqlparser::ast::FunctionArgumentList {
                        duplicate_treatment: None,
                        args: vec![FunctionArg::Unnamed(FunctionArgExpr::Wildcard)],
                        clauses: vec![],
                    },
                ),
                filter: None,
                null_treatment: None,
                over: None,
                within_group: vec![],
                parameters: sqlparser::ast::FunctionArguments::None,
            }),
            alias: Ident::new("data"),
        }],
        exclude: None,
        into: None,
        from: vec![TableWithJoins {
            relation: TableFactor::Table {
                name: make_ident_name(cte_name),
                alias: None,
                args: None,
                with_hints: vec![],
                version: None,
                partitions: vec![],
                with_ordinality: false,
                json_path: None,
                sample: None,
                index_hints: vec![],
            },
            joins: vec![],
        }],
        lateral_views: vec![],
        prewhere: None,
        selection: None,
        connect_by: vec![],
        group_by: sqlparser::ast::GroupByExpr::Expressions(vec![], vec![]),
        cluster_by: vec![],
        distribute_by: vec![],
        sort_by: vec![],
        having: None,
        named_window: vec![],
        qualify: None,
        window_before_qualify: false,
        value_table_mode: None,
        flavor: sqlparser::ast::SelectFlavor::Standard,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(sql: &str) -> Vec<Statement> {
        parse_sql(sql).unwrap()
    }

    // =========================================================================
    // extract_table_refs tests
    // =========================================================================

    #[test]
    fn test_extract_simple_select() {
        let refs = extract_table_refs(&parse("SELECT * FROM users"));
        assert_eq!(refs, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_extract_join() {
        let refs = extract_table_refs(&parse(
            "SELECT * FROM users u JOIN orders o ON u.id = o.user_id LEFT JOIN items i ON o.id = i.order_id",
        ));
        assert_eq!(
            refs,
            vec![
                Resource::schema("items"),
                Resource::schema("orders"),
                Resource::schema("users"),
            ]
        );
    }

    #[test]
    fn test_extract_subquery() {
        let refs = extract_table_refs(&parse(
            "SELECT * FROM users WHERE id IN (SELECT user_id FROM active_users)",
        ));
        assert!(refs.contains(&Resource::schema("users")));
        assert!(refs.contains(&Resource::schema("active_users")));
    }

    #[test]
    fn test_extract_insert() {
        let refs = extract_table_refs(&parse("INSERT INTO users (name) VALUES ('test')"));
        assert_eq!(refs, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_extract_insert_select() {
        let refs = extract_table_refs(&parse(
            "INSERT INTO users_backup SELECT * FROM users WHERE active = 1",
        ));
        assert!(refs.contains(&Resource::schema("users_backup")));
        assert!(refs.contains(&Resource::schema("users")));
    }

    #[test]
    fn test_extract_update() {
        let refs = extract_table_refs(&parse("UPDATE users SET name = 'test' WHERE id = 1"));
        assert_eq!(refs, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_extract_delete() {
        let refs = extract_table_refs(&parse("DELETE FROM users WHERE id = 1"));
        assert_eq!(refs, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_extract_create_table() {
        let created = extract_created_tables(&parse("CREATE TABLE users (id INTEGER PRIMARY KEY)"));
        assert_eq!(created, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_extract_create_table_if_not_exists() {
        let created = extract_created_tables(&parse(
            "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY)",
        ));
        assert_eq!(created, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_extract_create_view() {
        let created = extract_created_tables(&parse(
            "CREATE VIEW active_users AS SELECT * FROM users WHERE active = 1",
        ));
        assert_eq!(created, vec![Resource::schema("active_users")]);
    }

    #[test]
    fn test_view_definition_refs() {
        let refs = extract_table_refs(&parse(
            "CREATE VIEW active_users AS SELECT * FROM users WHERE active = 1",
        ));
        assert_eq!(refs, vec![Resource::schema("users")]);
    }

    #[test]
    fn test_normalize_quoted_names() {
        let refs = extract_table_refs(&parse(r#"SELECT * FROM "my table""#));
        assert_eq!(refs, vec![Resource::schema("my table")]);
    }

    #[test]
    fn test_extract_empty() {
        let refs = extract_table_refs(&[]);
        assert!(refs.is_empty());

        let created = extract_created_tables(&[]);
        assert!(created.is_empty());
    }

    #[test]
    fn test_extract_cte() {
        let refs = extract_table_refs(&parse(
            "WITH active AS (SELECT * FROM users WHERE active = 1) SELECT * FROM active JOIN orders ON active.id = orders.user_id",
        ));
        assert!(refs.contains(&Resource::schema("users")));
        assert!(refs.contains(&Resource::schema("orders")));
        assert!(
            !refs.contains(&Resource::schema("active")),
            "CTE name should not be included as external dependency"
        );
    }

    #[test]
    fn test_extract_nested_cte() {
        let refs = extract_table_refs(&parse(
            "WITH
                descendants AS (SELECT * FROM block WHERE parent_id = 'root'),
                grandchildren AS (SELECT * FROM descendants WHERE level > 1)
            SELECT * FROM grandchildren JOIN document ON grandchildren.doc_id = document.id",
        ));
        assert!(refs.contains(&Resource::schema("block")));
        assert!(refs.contains(&Resource::schema("document")));
        assert!(
            !refs.contains(&Resource::schema("descendants")),
            "CTE name 'descendants' should not be included"
        );
        assert!(
            !refs.contains(&Resource::schema("grandchildren")),
            "CTE name 'grandchildren' should not be included"
        );
    }

    #[test]
    fn test_extract_union() {
        let refs = extract_table_refs(&parse("SELECT * FROM users UNION SELECT * FROM admins"));
        assert!(refs.contains(&Resource::schema("users")));
        assert!(refs.contains(&Resource::schema("admins")));
    }

    #[test]
    fn test_extract_recursive_cte_in_subquery() {
        // GQL varlen queries generate WITH RECURSIVE _vl1 and reference it
        // in a WHERE ... IN (SELECT ... FROM _vl1) subquery. The CTE name
        // must not leak as an external table dependency.
        let sql = "\
            WITH RECURSIVE _vl1 AS (\
                SELECT _v0.id AS node_id, 0 AS depth, CAST(_v0.id AS TEXT) AS visited \
                FROM block AS _v0 WHERE _v0.id = 'test-root' \
                UNION ALL \
                SELECT _fk.id, _vl1.depth + 1, _vl1.visited || ',' || CAST(_fk.id AS TEXT) \
                FROM _vl1 JOIN block _fk ON _fk.parent_id = _vl1.node_id \
                WHERE _vl1.depth < 10\
            ) \
            SELECT _v2.id, _v2.content \
            FROM block AS _v0 \
            JOIN block AS _v2 ON 1 = 1 \
            WHERE _v0.id = 'test-root' \
            AND _v2.id IN (SELECT node_id FROM _vl1 WHERE depth >= 1 AND depth <= 10)";
        let refs = extract_table_refs(&parse(sql));
        assert!(
            refs.contains(&Resource::schema("block")),
            "should contain 'block'"
        );
        assert!(
            !refs.contains(&Resource::schema("_vl1")),
            "CTE name '_vl1' should not be an external dependency, got: {:?}",
            refs
        );
    }

    // =========================================================================
    // inject_entity_name tests
    // =========================================================================

    #[test]
    fn test_inject_simple_select() {
        let mut stmts = parse("SELECT id, name FROM block");
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
    }

    #[test]
    fn test_inject_union() {
        let mut stmts = parse("SELECT id FROM block UNION ALL SELECT id FROM document");
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
        assert!(sql.contains("'document' AS entity_name"), "got: {sql}");
    }

    #[test]
    fn test_inject_cte() {
        let mut stmts = parse(
            "WITH active AS (SELECT * FROM block WHERE active = 1) SELECT * FROM active JOIN document ON active.doc_id = document.id",
        );
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        // CTE body injects 'block' AS entity_name
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
        // Outer SELECT * FROM CTE: wildcard already includes entity_name, no extra column added
    }

    #[test]
    fn test_inject_cte_explicit_columns_passes_through_entity_name() {
        let mut stmts = parse(
            "WITH children AS (SELECT * FROM block WHERE parent_id = 'x') SELECT id, content FROM children",
        );
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        // CTE body gets 'block' AS entity_name
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
        // Outer SELECT has explicit columns — entity_name added as column reference
        assert!(
            sql.contains("content, entity_name FROM"),
            "outer SELECT should reference entity_name from CTE: {sql}"
        );
    }

    #[test]
    fn test_inject_join_uses_primary_table() {
        let mut stmts =
            parse("SELECT b.id, d.name FROM block b JOIN document d ON b.doc_id = d.id");
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
        assert!(
            !sql.contains("'document' AS entity_name"),
            "JOIN table should not get its own entity_name: {sql}"
        );
    }

    #[test]
    fn test_inject_gql_style_qualified_columns_from_join() {
        // GQL-compiled SQL: first FROM is a matview, but all RETURN columns come
        // from a JOINed table (block). entity_name should be 'block', not the matview.
        let mut stmts = parse(
            "SELECT _v4.\"id\" AS \"id\", _v4.\"content\" AS \"content\" \
             FROM current_focus AS _v0 \
             JOIN block AS _v2 ON _v0.block_id = _v2.parent_id \
             JOIN block AS _v4 ON _v4.id = _v2.id",
        );
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            sql.contains("'block' AS entity_name"),
            "should resolve to 'block' via dominant qualifier _v4, got: {sql}"
        );
        assert!(
            !sql.contains("'current_focus' AS entity_name"),
            "should NOT use matview name: {sql}"
        );
    }

    #[test]
    fn test_inject_subquery() {
        let mut stmts = parse("SELECT * FROM (SELECT id, name FROM block)");
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        // The outer SELECT resolves through the subquery to 'block'
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
    }

    #[test]
    fn test_inject_nested_union() {
        let mut stmts = parse(
            "SELECT id FROM block UNION ALL SELECT id FROM document UNION ALL SELECT id FROM tags",
        );
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
        assert!(sql.contains("'document' AS entity_name"), "got: {sql}");
        assert!(sql.contains("'tags' AS entity_name"), "got: {sql}");
    }

    #[test]
    fn test_inject_idempotent() {
        let mut stmts = parse("SELECT id, 'block' AS entity_name FROM block");
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        let count = sql.matches("entity_name").count();
        assert_eq!(count, 1, "should not duplicate entity_name: {sql}");
    }

    #[test]
    fn test_inject_select_star() {
        let mut stmts = parse("SELECT * FROM block");
        inject_entity_name(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(sql.contains("'block' AS entity_name"), "got: {sql}");
    }

    // =========================================================================
    // _change_origin injection tests
    // =========================================================================

    #[test]
    fn test_change_origin_explicit_cols() {
        let mut stmts = parse("SELECT id, content FROM block");
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            sql.contains("block._change_origin"),
            "should inject _change_origin: {sql}"
        );
    }

    #[test]
    fn test_change_origin_skips_wildcard() {
        let mut stmts = parse("SELECT * FROM block");
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            !sql.contains("block._change_origin"),
            "should NOT inject for SELECT *: {sql}"
        );
    }

    #[test]
    fn test_change_origin_idempotent() {
        let mut stmts = parse("SELECT id, block._change_origin AS _change_origin FROM block");
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        let count = sql.matches("_change_origin").count();
        assert_eq!(count, 2, "should not duplicate _change_origin: {sql}");
    }

    #[test]
    fn test_change_origin_union() {
        let mut stmts = parse("SELECT id FROM block UNION ALL SELECT id FROM document");
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(sql.contains("block._change_origin"), "block branch: {sql}");
        assert!(
            sql.contains("document._change_origin"),
            "document branch: {sql}"
        );
    }

    #[test]
    fn test_change_origin_skips_matview() {
        let mut stmts = parse("SELECT id, content FROM block_with_path WHERE path LIKE '/abc%'");
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            !sql.contains("_change_origin"),
            "should NOT inject _change_origin for materialized view: {sql}"
        );
    }

    #[test]
    fn test_change_origin_gql_style_uses_join_alias() {
        // GQL-compiled SQL: first FROM is a matview (no _change_origin), but all
        // selected columns come from block (via JOIN). Should inject using the
        // block alias, not the matview alias.
        let mut stmts = parse(
            "SELECT _v4.\"id\" AS \"id\", _v4.\"content\" AS \"content\" \
             FROM current_focus AS _v0 \
             JOIN block AS _v2 ON _v0.block_id = _v2.parent_id \
             JOIN block AS _v4 ON _v4.id = _v2.id",
        );
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            sql.contains("_v4._change_origin"),
            "should inject via block alias _v4, got: {sql}"
        );
        assert!(
            !sql.contains("_v0._change_origin"),
            "should NOT inject via matview alias _v0: {sql}"
        );
    }

    #[test]
    fn test_change_origin_uses_alias() {
        let mut stmts =
            parse("SELECT _v0.id AS \"b.id\", _v0.content AS \"b.content\" FROM block AS _v0");
        inject_change_origin(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            sql.contains("_v0._change_origin"),
            "should use alias _v0 not bare table name: {sql}"
        );
        assert!(
            !sql.contains("block._change_origin"),
            "should NOT use bare table name when aliased: {sql}"
        );
    }

    // =========================================================================
    // JSON aggregation tests
    // =========================================================================

    #[test]
    fn test_json_agg_skips_non_union() {
        let mut stmts = parse("SELECT id, content FROM block");
        inject_json_aggregation(&mut stmts);
        let sql = sql_to_string(&stmts);
        assert!(
            !sql.to_lowercase().contains("json_object"),
            "non-UNION should not get json_object: {sql}"
        );
    }

    #[test]
    fn test_json_agg_union() {
        let mut stmts =
            parse("SELECT id, name FROM projects UNION ALL SELECT id, content AS name FROM tasks");
        inject_json_aggregation(&mut stmts);
        let sql = sql_to_string(&stmts);
        let lower = sql.to_lowercase();
        assert!(
            lower.contains("_branch_0"),
            "should have CTE _branch_0: {sql}"
        );
        assert!(
            lower.contains("_branch_1"),
            "should have CTE _branch_1: {sql}"
        );
        assert!(
            lower.contains("json_object(*)"),
            "should have json_object(*): {sql}"
        );
        assert!(lower.contains("union"), "should still have UNION: {sql}");
    }

    #[test]
    fn test_json_agg_three_way_union() {
        let mut stmts =
            parse("SELECT id FROM a UNION ALL SELECT id FROM b UNION ALL SELECT id FROM c");
        inject_json_aggregation(&mut stmts);
        let sql = sql_to_string(&stmts);
        let lower = sql.to_lowercase();
        assert!(lower.contains("_branch_0"), "CTE 0: {sql}");
        assert!(lower.contains("_branch_1"), "CTE 1: {sql}");
        assert!(lower.contains("_branch_2"), "CTE 2: {sql}");
        // 3 json_object(*) calls
        assert_eq!(
            lower.matches("json_object(*)").count(),
            3,
            "3 branches: {sql}"
        );
    }

    #[test]
    fn test_json_agg_preserves_existing_ctes() {
        let mut stmts = parse(
            "WITH active AS (SELECT * FROM projects WHERE active = 1) SELECT id FROM active UNION ALL SELECT id FROM tasks",
        );
        inject_json_aggregation(&mut stmts);
        let sql = sql_to_string(&stmts);
        let lower = sql.to_lowercase();
        assert!(
            lower.contains("active"),
            "should preserve existing CTE: {sql}"
        );
        assert!(lower.contains("_branch_0"), "should add branch CTEs: {sql}");
    }

    // =========================================================================
    // apply_sql_transforms integration test
    // =========================================================================

    #[test]
    fn test_apply_sql_transforms_chains_all() {
        let transformers: Vec<Box<dyn SqlTransformer>> =
            vec![Box::new(EntityNameInjector), Box::new(ChangeOriginInjector)];
        let result = apply_sql_transforms("SELECT id FROM block", &transformers);
        assert!(
            result.contains("entity_name"),
            "should inject entity_name: {result}"
        );
        assert!(
            result.contains("_change_origin"),
            "should inject _change_origin: {result}"
        );
    }

    #[test]
    fn test_apply_sql_transforms_gql_style_full_pipeline() {
        // End-to-end: GQL-compiled SQL through both EntityNameInjector and ChangeOriginInjector.
        // Both should resolve to the block table (via dominant qualifier), not the matview.
        let transformers: Vec<Box<dyn SqlTransformer>> =
            vec![Box::new(EntityNameInjector), Box::new(ChangeOriginInjector)];
        let sql = "SELECT _v4.\"id\" AS \"id\", _v4.\"content\" AS \"content\" \
                   FROM current_focus AS _v0 \
                   JOIN block AS _v2 ON _v0.block_id = _v2.parent_id \
                   JOIN block AS _v4 ON _v4.id = _v2.id";
        let result = apply_sql_transforms(sql, &transformers);
        assert!(
            result.contains("'block' AS entity_name"),
            "entity_name should be 'block': {result}"
        );
        assert!(
            result.contains("_v4._change_origin"),
            "_change_origin should use block alias _v4: {result}"
        );
        assert!(
            !result.contains("'current_focus'"),
            "should NOT reference matview name: {result}"
        );
        assert!(
            !result.contains("_v0._change_origin"),
            "should NOT use matview alias for _change_origin: {result}"
        );
    }

    #[test]
    fn test_apply_sql_transforms_returns_original_on_parse_error() {
        let transformers: Vec<Box<dyn SqlTransformer>> = vec![Box::new(EntityNameInjector)];
        let bad_sql = "NOT VALID SQL AT ALL";
        let result = apply_sql_transforms(bad_sql, &transformers);
        assert_eq!(result, bad_sql);
    }
}
