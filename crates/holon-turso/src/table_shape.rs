//! A stored table's shape against the `CREATE TABLE` that declares it.
//!
//! `CREATE TABLE IF NOT EXISTS` leaves a stored table at whatever shape an
//! older binary gave it. [`reconcile_table`] compares the two by STRUCTURE
//! (`PRAGMA table_info` against the parsed declaration, never SQL text, which
//! `ALTER TABLE` reformats): a declared column the stored table lacks is added
//! when that cannot lose a row, and every other difference is refused with the
//! difference and the row count, so the caller can disclose it and leave the
//! rows alone.

use std::collections::HashMap;
use std::fmt;

use holon_api::Value;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;
use sqlparser::ast::ColumnOption;
use sqlparser::ast::Expr;
use sqlparser::ast::Statement;
use sqlparser::ast::TableConstraint;
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::matview_manager::drop_dependent_views;
use crate::sql_utils::sql_statements;
use crate::turso::DbHandle;

/// The structure of one column: what decides whether a row written for one
/// shape fits the other. A column's DEFAULT expression is not part of it, only
/// whether it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnShape {
    pub name: String,
    /// Uppercased, whitespace collapsed.
    pub sql_type: String,
    pub not_null: bool,
    pub has_default: bool,
    /// 1-based position in the primary key, 0 when not part of it.
    pub pk: u32,
}

impl ColumnShape {
    fn same_structure(&self, other: &Self) -> bool {
        self.sql_type == other.sql_type && self.not_null == other.not_null && self.pk == other.pk
    }

    /// A row that does not name this column can still be stored.
    fn optional(&self) -> bool {
        self.pk == 0 && (!self.not_null || self.has_default)
    }
}

impl fmt::Display for ColumnShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.sql_type)?;
        if self.not_null {
            write!(f, " NOT NULL")?;
        }
        if self.pk > 0 {
            write!(f, " PRIMARY KEY")?;
        }
        Ok(())
    }
}

fn normalize_type(sql_type: &str) -> String {
    sql_type
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// One column of a declaration, with the text `ALTER TABLE ADD COLUMN` takes.
#[derive(Clone, Debug)]
struct DeclaredColumn {
    shape: ColumnShape,
    definition: String,
}

/// A parsed `CREATE TABLE` statement.
#[derive(Clone, Debug)]
pub struct DeclaredTable {
    table: String,
    columns: Vec<DeclaredColumn>,
}

impl DeclaredTable {
    pub fn parse(create_sql: &str) -> Result<Self> {
        let statements = Parser::parse_sql(&SQLiteDialect {}, create_sql).map_err(|e| {
            StorageError::SchemaError(format!(
                "cannot parse table declaration {create_sql:?}: {e}"
            ))
        })?;
        let [Statement::CreateTable(create)] = statements.as_slice() else {
            return Err(StorageError::SchemaError(format!(
                "expected exactly one CREATE TABLE statement, got {create_sql:?}"
            )));
        };
        let table = create
            .name
            .0
            .last()
            .and_then(|part| part.as_ident())
            .map(|ident| ident.value.clone())
            .ok_or_else(|| {
                StorageError::SchemaError(format!(
                    "table declaration without a name: {create_sql:?}"
                ))
            })?;

        let mut table_pk: Vec<String> = Vec::new();
        for constraint in &create.constraints {
            if let TableConstraint::PrimaryKey(pk) = constraint {
                for column in &pk.columns {
                    let Expr::Identifier(ident) = &column.column.expr else {
                        return Err(StorageError::SchemaError(format!(
                            "{table}: a PRIMARY KEY entry is not a column name: {}",
                            column.column.expr
                        )));
                    };
                    table_pk.push(ident.value.clone());
                }
            }
        }

        let columns = create
            .columns
            .iter()
            .map(|def| {
                let name = def.name.value.clone();
                let options = || def.options.iter().map(|o| &o.option);
                let inline_pk = options().any(|o| matches!(o, ColumnOption::PrimaryKey(_)));
                let pk = match table_pk.iter().position(|c| c.eq_ignore_ascii_case(&name)) {
                    Some(i) => i as u32 + 1,
                    None if inline_pk => 1,
                    None => 0,
                };
                DeclaredColumn {
                    shape: ColumnShape {
                        sql_type: normalize_type(&def.data_type.to_string()),
                        not_null: options().any(|o| matches!(o, ColumnOption::NotNull)),
                        has_default: options().any(|o| matches!(o, ColumnOption::Default(_))),
                        pk,
                        name,
                    },
                    definition: def.to_string(),
                }
            })
            .collect();
        Ok(Self { table, columns })
    }

    pub fn table(&self) -> &str {
        &self.table
    }

    fn shape(&self) -> TableShape {
        TableShape {
            table: self.table.clone(),
            columns: self.columns.iter().map(|c| c.shape.clone()).collect(),
        }
    }
}

/// A table's columns, declared or stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableShape {
    pub table: String,
    pub columns: Vec<ColumnShape>,
}

impl TableShape {
    /// The stored shape, from `PRAGMA table_info`. Empty columns: no such
    /// table.
    pub async fn stored(db_handle: &DbHandle, table: &str) -> Result<Self> {
        let rows = db_handle
            .query(&format!("PRAGMA table_info(\"{table}\")"), HashMap::new())
            .await?;
        let columns = rows
            .iter()
            .map(|row| {
                let text = |key: &str| match row.get(key) {
                    Some(Value::String(s)) => Ok(s.clone()),
                    other => Err(StorageError::SchemaError(format!(
                        "PRAGMA table_info({table}): `{key}` is {other:?}, not TEXT"
                    ))),
                };
                let int = |key: &str| match row.get(key) {
                    Some(Value::Integer(i)) => Ok(*i),
                    other => Err(StorageError::SchemaError(format!(
                        "PRAGMA table_info({table}): `{key}` is {other:?}, not INTEGER"
                    ))),
                };
                Ok(ColumnShape {
                    name: text("name")?,
                    sql_type: normalize_type(&text("type")?),
                    not_null: int("notnull")? != 0,
                    has_default: !matches!(row.get("dflt_value"), None | Some(Value::Null)),
                    pk: u32::try_from(int("pk")?).map_err(|e| {
                        StorageError::SchemaError(format!("PRAGMA table_info({table}): pk: {e}"))
                    })?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            table: table.to_string(),
            columns,
        })
    }
}

/// A declared column whose stored structure differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnChange {
    pub stored: ColumnShape,
    pub declared: ColumnShape,
}

/// How a stored table differs from its declaration, column by column (matched
/// by name, case-insensitively, as SQLite matches them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeDiff {
    pub table: String,
    /// Declared, not stored.
    pub added: Vec<ColumnShape>,
    /// Stored, not declared.
    pub removed: Vec<ColumnShape>,
    pub changed: Vec<ColumnChange>,
}

impl ShapeDiff {
    pub fn between(stored: &TableShape, declared: &TableShape) -> Self {
        let find = |columns: &[ColumnShape], name: &str| {
            columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(name))
                .cloned()
        };
        let mut added = Vec::new();
        let mut changed = Vec::new();
        for declared_column in &declared.columns {
            match find(&stored.columns, &declared_column.name) {
                None => added.push(declared_column.clone()),
                Some(stored_column) if !stored_column.same_structure(declared_column) => changed
                    .push(ColumnChange {
                        stored: stored_column,
                        declared: declared_column.clone(),
                    }),
                Some(_) => {}
            }
        }
        let removed = stored
            .columns
            .iter()
            .filter(|c| find(&declared.columns, &c.name).is_none())
            .cloned()
            .collect();
        Self {
            table: declared.table.clone(),
            added,
            removed,
            changed,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    /// Adding the declared columns keeps every stored row and every write the
    /// declaration allows. A column both removed and added may be a rename,
    /// which is never guessed.
    pub fn is_lossless(&self) -> bool {
        self.changed.is_empty()
            && (self.added.is_empty() || self.removed.is_empty())
            && self
                .added
                .iter()
                .chain(&self.removed)
                .all(ColumnShape::optional)
    }
}

impl fmt::Display for ShapeDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .changed
            .iter()
            .map(|c| format!("{} -> {}", c.stored, c.declared))
            .chain(self.added.iter().map(|c| format!("missing {c}")))
            .chain(self.removed.iter().map(|c| format!("undeclared {c}")))
            .collect();
        write!(f, "{}", parts.join("; "))
    }
}

/// A stored table that differs from its declaration in a way no column
/// addition fixes. Its rows are untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeRefusal {
    pub diff: ShapeDiff,
    pub rows: u64,
}

impl fmt::Display for ShapeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stored table {} ({} rows) differs from its declaration: {}",
            self.diff.table, self.rows, self.diff
        )
    }
}

/// A stored table that gained the declared columns it lacked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableAdapted {
    pub table: String,
    pub added: Vec<String>,
    /// Stored columns the declaration does not name, kept as they are.
    pub undeclared: Vec<String>,
    pub rows: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableReconcile {
    Unchanged,
    Adapted(TableAdapted),
    Refused(ShapeRefusal),
}

/// Run `create_sql` (a `CREATE TABLE IF NOT EXISTS`) and bring an older stored
/// table to its shape where that loses nothing.
///
/// Adding a column drops the views that read the table first, because the
/// engine refuses `ALTER TABLE` under a dependent materialized view; their
/// owners recreate them on their own reconcile.
pub async fn reconcile_table(db_handle: &DbHandle, create_sql: &str) -> Result<TableReconcile> {
    let declared = DeclaredTable::parse(create_sql)?;
    db_handle.execute_ddl(create_sql).await?;
    let stored = TableShape::stored(db_handle, declared.table()).await?;
    let diff = ShapeDiff::between(&stored, &declared.shape());
    if diff.is_empty() {
        return Ok(TableReconcile::Unchanged);
    }
    let rows = row_count(db_handle, declared.table()).await?;
    if !diff.is_lossless() {
        return Ok(TableReconcile::Refused(ShapeRefusal { diff, rows }));
    }
    if !diff.added.is_empty() {
        drop_dependent_views(db_handle, declared.table())
            .await
            .map_err(|e| {
                StorageError::SchemaError(format!(
                    "dropping the views over {} before adding its declared columns: {e:#}",
                    declared.table()
                ))
            })?;
    }
    for added in &diff.added {
        let column = declared
            .columns
            .iter()
            .find(|c| c.shape.name == added.name)
            .expect("an added column comes from the declaration");
        db_handle
            .execute_ddl(&format!(
                "ALTER TABLE \"{}\" ADD COLUMN {}",
                declared.table(),
                column.definition
            ))
            .await
            .map_err(|e| {
                StorageError::SchemaError(format!(
                    "adding declared column {added} to stored table {} ({rows} rows): {e}",
                    declared.table()
                ))
            })?;
    }
    Ok(TableReconcile::Adapted(TableAdapted {
        table: declared.table().to_string(),
        added: diff.added.iter().map(|c| c.name.clone()).collect(),
        undeclared: diff.removed.iter().map(|c| c.name.clone()).collect(),
        rows,
    }))
}

async fn row_count(db_handle: &DbHandle, table: &str) -> Result<u64> {
    let rows = db_handle
        .query(
            &format!("SELECT COUNT(*) AS n FROM \"{table}\""),
            HashMap::new(),
        )
        .await?;
    match rows.first().and_then(|r| r.get("n")) {
        Some(Value::Integer(n)) => u64::try_from(*n)
            .map_err(|e| StorageError::SchemaError(format!("row count of {table}: {e}"))),
        other => Err(StorageError::SchemaError(format!(
            "row count of {table}: expected INTEGER, got {other:?}"
        ))),
    }
}

fn is_create_table(statement: &str) -> bool {
    let code: String = statement
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join(" ");
    let words: Vec<String> = code
        .split_whitespace()
        .take(2)
        .map(str::to_ascii_uppercase)
        .collect();
    words == ["CREATE", "TABLE"]
}

/// Run one schema statement: a `CREATE TABLE` through [`reconcile_table`], a
/// refused shape as an error naming the difference; anything else as DDL.
pub async fn ensure_statement(db_handle: &DbHandle, statement: &str) -> Result<()> {
    if !is_create_table(statement) {
        return db_handle.execute_ddl(statement).await;
    }
    match reconcile_table(db_handle, statement).await? {
        TableReconcile::Unchanged => Ok(()),
        TableReconcile::Adapted(adapted) => {
            tracing::warn!(
                table = %adapted.table,
                added = ?adapted.added,
                undeclared = ?adapted.undeclared,
                rows = adapted.rows,
                "stored table gained its newly declared columns"
            );
            Ok(())
        }
        TableReconcile::Refused(refusal) => Err(StorageError::SchemaError(refusal.to_string())),
    }
}

/// [`ensure_statement`] for every statement of a schema file.
pub async fn ensure_schema_sql(db_handle: &DbHandle, sql: &str) -> Result<()> {
    for statement in sql_statements(sql) {
        ensure_statement(db_handle, statement).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(
        name: &str,
        sql_type: &str,
        not_null: bool,
        has_default: bool,
        pk: u32,
    ) -> ColumnShape {
        ColumnShape {
            name: name.into(),
            sql_type: sql_type.into(),
            not_null,
            has_default,
            pk,
        }
    }

    #[test]
    fn a_declaration_parses_into_its_column_structure() {
        let declared = DeclaredTable::parse(
            "CREATE TABLE IF NOT EXISTS \"t\" (\n  \"id\" TEXT PRIMARY KEY,\n  \"n\" real NOT \
             NULL DEFAULT 0,\n  \"note\" TEXT\n)",
        )
        .expect("parse");
        assert_eq!(declared.table(), "t");
        assert_eq!(
            declared.shape().columns,
            vec![
                column("id", "TEXT", false, false, 1),
                column("n", "REAL", true, true, 0),
                column("note", "TEXT", false, false, 0),
            ]
        );
        assert_eq!(
            declared.columns[1].definition,
            "\"n\" REAL NOT NULL DEFAULT 0"
        );
    }

    #[test]
    fn a_table_level_primary_key_numbers_its_columns() {
        let declared = DeclaredTable::parse(
            "CREATE TABLE d (block_id TEXT NOT NULL, field_name TEXT NOT NULL, v TEXT, PRIMARY \
             KEY (block_id, field_name))",
        )
        .expect("parse");
        let pks: Vec<u32> = declared.shape().columns.iter().map(|c| c.pk).collect();
        assert_eq!(pks, vec![1, 2, 0]);
    }

    fn shape(columns: Vec<ColumnShape>) -> TableShape {
        TableShape {
            table: "t".into(),
            columns,
        }
    }

    #[test]
    fn only_optional_additions_are_lossless() {
        let id = column("id", "TEXT", false, false, 1);
        let stored = shape(vec![id.clone()]);
        let nullable = ShapeDiff::between(
            &stored,
            &shape(vec![id.clone(), column("a", "TEXT", false, false, 0)]),
        );
        assert!(nullable.is_lossless(), "{nullable}");
        let defaulted = ShapeDiff::between(
            &stored,
            &shape(vec![id.clone(), column("a", "TEXT", true, true, 0)]),
        );
        assert!(defaulted.is_lossless(), "{defaulted}");
        let required = ShapeDiff::between(
            &stored,
            &shape(vec![id.clone(), column("a", "TEXT", true, false, 0)]),
        );
        assert!(!required.is_lossless(), "{required}");
    }

    #[test]
    fn a_type_change_or_a_possible_rename_is_refused() {
        let id = column("id", "TEXT", false, false, 1);
        let retyped = ShapeDiff::between(
            &shape(vec![id.clone(), column("q", "REAL", true, false, 0)]),
            &shape(vec![id.clone(), column("q", "TEXT", true, false, 0)]),
        );
        assert!(!retyped.is_lossless());
        assert_eq!(retyped.to_string(), "q REAL NOT NULL -> q TEXT NOT NULL");
        let renamed = ShapeDiff::between(
            &shape(vec![id.clone(), column("old", "TEXT", false, false, 0)]),
            &shape(vec![id.clone(), column("new", "TEXT", false, false, 0)]),
        );
        assert!(!renamed.is_lossless(), "{renamed}");
    }

    #[test]
    fn a_default_expression_is_not_structure() {
        let a = column("a", "TEXT", true, true, 0);
        let b = column("A", "text", true, false, 0);
        let diff = ShapeDiff::between(
            &shape(vec![a]),
            &shape(vec![ColumnShape {
                sql_type: normalize_type(&b.sql_type),
                ..b
            }]),
        );
        assert!(diff.is_empty(), "{diff}");
    }

    #[test]
    fn create_table_detection_skips_leading_comments() {
        assert!(is_create_table("-- note\n  create  table x (a)"));
        assert!(!is_create_table("CREATE INDEX i ON x (a)"));
        assert!(!is_create_table("CREATE MATERIALIZED VIEW v AS SELECT 1"));
    }
}
