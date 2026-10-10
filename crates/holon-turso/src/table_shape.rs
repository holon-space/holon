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

use holon_api::QuarantinedTable;
use holon_api::Value;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;
use sqlparser::ast::ColumnOption;
use sqlparser::ast::Expr;
use sqlparser::ast::Ident;
use sqlparser::ast::ObjectName;
use sqlparser::ast::Statement;
use sqlparser::ast::TableConstraint;
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::matview_manager::drop_dependent_views;
use crate::sql_utils::sql_statements;
use crate::table_classes::Class;
use crate::table_classes::class_of;
use crate::table_classes::emptied_with;
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

    pub fn shape(&self) -> TableShape {
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

/// A stored table the org files and the Loro store refill
/// ([`Class::Rebuilt`]) that differed from its declaration, so it was dropped
/// and recreated empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRebuilt {
    pub diff: ShapeDiff,
    pub rows: u64,
}

/// What [`ensure_statement`] did to a stored table beyond creating it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableChange {
    ColumnsAdded(TableAdapted),
    Rebuilt(TableRebuilt),
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

/// `create_sql` declaring a table named `name` instead.
fn renamed_create(create_sql: &str, name: &str) -> Result<String> {
    let mut statements = Parser::parse_sql(&SQLiteDialect {}, create_sql).map_err(|e| {
        StorageError::SchemaError(format!(
            "cannot parse table declaration {create_sql:?}: {e}"
        ))
    })?;
    let [Statement::CreateTable(create)] = statements.as_mut_slice() else {
        return Err(StorageError::SchemaError(format!(
            "expected exactly one CREATE TABLE statement, got {create_sql:?}"
        )));
    };
    create.name = ObjectName::from(vec![Ident::with_quote('"', name)]);
    Ok(statements[0].to_string())
}

fn code_words(statement: &str) -> impl Iterator<Item = &str> {
    statement
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .flat_map(str::split_whitespace)
}

fn is_create_table(statement: &str) -> bool {
    let words: Vec<String> = code_words(statement)
        .take(2)
        .map(str::to_ascii_uppercase)
        .collect();
    words == ["CREATE", "TABLE"]
}

/// The first words of `statement`, enough to tell which object it creates.
fn statement_head(statement: &str) -> String {
    code_words(statement).take(6).collect::<Vec<_>>().join(" ")
}

/// Run one schema statement: a `CREATE TABLE` through [`reconcile_table`];
/// anything else as DDL. A refused shape is rebuilt when the table is one the
/// org files and the Loro store refill, and an error naming the difference
/// otherwise.
pub async fn ensure_statement(
    db_handle: &DbHandle,
    statement: &str,
) -> Result<Option<TableChange>> {
    if !is_create_table(statement) {
        db_handle.execute_ddl(statement).await?;
        return Ok(None);
    }
    match reconcile_table(db_handle, statement).await? {
        TableReconcile::Unchanged => Ok(None),
        TableReconcile::Adapted(adapted) => Ok(Some(TableChange::ColumnsAdded(adapted))),
        TableReconcile::Refused(refusal) => {
            if class_of(&refusal.diff.table, &HashMap::new()) != Some(Class::Rebuilt) {
                return Err(StorageError::SchemaError(refusal.to_string()));
            }
            rebuild_table(db_handle, statement, &refusal).await?;
            Ok(Some(TableChange::Rebuilt(TableRebuilt {
                diff: refusal.diff,
                rows: refusal.rows,
            })))
        }
    }
}

/// Drop `refusal`'s table with the views over it, create it from
/// `create_sql`, and empty the tables the org ingest refills with it.
///
/// The declaration is first created under a scratch name, so one the engine
/// cannot create fails before anything is dropped or emptied.
///
/// Foreign keys are off for the drop, as SQLite's table-rebuild procedure
/// prescribes: rows of a kept table that reference it (dismissed advice on a
/// block) find the same ids again once the ingest refills it.
async fn rebuild_table(
    db_handle: &DbHandle,
    create_sql: &str,
    refusal: &ShapeRefusal,
) -> Result<()> {
    let table = refusal.diff.table.as_str();
    let rebuild_error = |step: &str, e: &dyn fmt::Display| {
        StorageError::SchemaError(format!("rebuilding {refusal}: {step}: {e}"))
    };
    let probe = format!("{table}__declaration_probe");
    let probe_sql = renamed_create(create_sql, &probe)?;
    db_handle
        .execute_ddl(&format!("DROP TABLE IF EXISTS \"{probe}\""))
        .await
        .map_err(|e| rebuild_error("dropping a leftover declaration probe", &e))?;
    db_handle.execute_ddl(&probe_sql).await.map_err(|e| {
        rebuild_error(
            "its declaration cannot be created, so the stored table and its rows are kept",
            &e,
        )
    })?;
    db_handle
        .execute_ddl(&format!("DROP TABLE \"{probe}\""))
        .await
        .map_err(|e| rebuild_error("dropping the declaration probe", &e))?;
    drop_dependent_views(db_handle, table)
        .await
        .map_err(|e| rebuild_error("dropping the views over it", &e))?;
    for emptied in emptied_with(table) {
        if !table_exists(db_handle, emptied).await? {
            continue;
        }
        db_handle
            .execute(&format!("DELETE FROM \"{emptied}\""), vec![])
            .await
            .map_err(|e| rebuild_error(&format!("emptying {emptied}"), &e))?;
    }
    db_handle
        .execute_ddl("PRAGMA foreign_keys = OFF")
        .await
        .map_err(|e| rebuild_error("turning foreign keys off", &e))?;
    let replaced = async {
        db_handle
            .execute_ddl(&format!("DROP TABLE \"{table}\""))
            .await
            .map_err(|e| rebuild_error("dropping it", &e))?;
        db_handle
            .execute_ddl(create_sql)
            .await
            .map_err(|e| rebuild_error("creating it from its declaration", &e))
    }
    .await;
    db_handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .map_err(|e| rebuild_error("turning foreign keys back on", &e))?;
    replaced
}

/// Drop every view that reads stored `table`, depth first.
async fn drop_views_over(db_handle: &DbHandle, table: &str) -> Result<()> {
    drop_dependent_views(db_handle, table)
        .await
        .map_err(|e| StorageError::SchemaError(format!("dropping the views over {table}: {e:#}")))
}

/// Holon's record of every table it quarantined. Only a recorded table is a
/// quarantine: a table merely named like one is the user's.
const QUARANTINE_RECORD: &str = "_holon_quarantine";

const QUARANTINE_RECORD_SQL: &str = "CREATE TABLE IF NOT EXISTS _holon_quarantine (
    quarantined TEXT PRIMARY KEY NOT NULL,
    type_table TEXT NOT NULL,
    rows INTEGER NOT NULL,
    reason TEXT NOT NULL,
    at TEXT NOT NULL
)";

struct Recorded {
    quarantined: String,
    type_table: String,
    rows: u64,
}

async fn recorded(db_handle: &DbHandle) -> Result<Vec<Recorded>> {
    db_handle.execute_ddl(QUARANTINE_RECORD_SQL).await?;
    db_handle
        .query(
            &format!(
                "SELECT quarantined, type_table, rows FROM {QUARANTINE_RECORD} ORDER BY \
                 quarantined"
            ),
            HashMap::new(),
        )
        .await?
        .into_iter()
        .map(|row| {
            match (
                row.get("quarantined"),
                row.get("type_table"),
                row.get("rows"),
            ) {
                (
                    Some(Value::String(quarantined)),
                    Some(Value::String(type_table)),
                    Some(Value::Integer(rows)),
                ) => Ok(Recorded {
                    quarantined: quarantined.clone(),
                    type_table: type_table.clone(),
                    rows: u64::try_from(*rows).map_err(|e| {
                        StorageError::SchemaError(format!("{QUARANTINE_RECORD}.rows: {e}"))
                    })?,
                }),
                _ => Err(StorageError::SchemaError(format!(
                    "{QUARANTINE_RECORD} row is {row:?}"
                ))),
            }
        })
        .collect()
}

async fn schema_names(db_handle: &DbHandle, kind: Option<&str>) -> Result<Vec<String>> {
    let sql = match kind {
        Some(kind) => format!("SELECT name FROM sqlite_schema WHERE type = '{kind}'"),
        None => "SELECT name FROM sqlite_schema".to_string(),
    };
    db_handle
        .query(&sql, HashMap::new())
        .await?
        .into_iter()
        .map(|row| match row.get("name") {
            Some(Value::String(name)) => Ok(name.clone()),
            other => Err(StorageError::SchemaError(format!(
                "sqlite_schema name: expected TEXT, got {other:?}"
            ))),
        })
        .collect()
}

/// The quarantines Holon recorded for `table`.
pub struct Quarantines {
    /// Recorded tables that are stored, with the rows they hold now.
    pub stored: Vec<QuarantinedTable>,
    /// Recorded tables that are not stored, with the rows they held when
    /// quarantined.
    pub missing: Vec<QuarantinedTable>,
}

pub async fn quarantines(db_handle: &DbHandle, table: &str) -> Result<Quarantines> {
    let tables: Vec<String> = schema_names(db_handle, Some("table")).await?;
    let mut quarantines = Quarantines {
        stored: Vec::new(),
        missing: Vec::new(),
    };
    for record in recorded(db_handle).await? {
        if record.type_table != table {
            continue;
        }
        if tables.contains(&record.quarantined) {
            let rows = row_count(db_handle, &record.quarantined).await?;
            quarantines.stored.push(QuarantinedTable {
                name: record.quarantined,
                rows,
            });
        } else {
            quarantines.missing.push(QuarantinedTable {
                name: record.quarantined,
                rows: record.rows,
            });
        }
    }
    Ok(quarantines)
}

/// Make stored `table` unreadable by every query that names it: drop the
/// views over it, then, in one transaction, rename it to the first name no
/// schema object or record holds and record it as quarantined for `reason`.
/// Returns that name. Its rows stay as they are.
pub async fn quarantine(db_handle: &DbHandle, table: &str, reason: &str) -> Result<String> {
    drop_views_over(db_handle, table).await?;
    let taken: Vec<String> = schema_names(db_handle, None)
        .await?
        .into_iter()
        .chain(
            recorded(db_handle)
                .await?
                .into_iter()
                .map(|r| r.quarantined),
        )
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let base = format!("{table}__quarantined");
    let quarantined = std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base}_{n}")))
        .find(|name| !taken.contains(&name.to_ascii_lowercase()))
        .expect("an unbounded sequence of names has a free one");
    let rows = row_count(db_handle, table).await?;
    db_handle
        .transaction(vec![
            (
                format!("ALTER TABLE \"{table}\" RENAME TO \"{quarantined}\""),
                vec![],
            ),
            (
                format!(
                    "INSERT INTO {QUARANTINE_RECORD} (quarantined, type_table, rows, reason, at) \
                     VALUES (?, ?, ?, ?, ?)"
                ),
                vec![
                    turso::Value::Text(quarantined.clone()),
                    turso::Value::Text(table.to_string()),
                    turso::Value::Integer(i64::try_from(rows).map_err(|e| {
                        StorageError::SchemaError(format!("row count of {table}: {e}"))
                    })?),
                    turso::Value::Text(reason.to_string()),
                    turso::Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            ),
        ])
        .await
        .map_err(|e| StorageError::SchemaError(format!("quarantining {table}: {e}")))?;
    Ok(quarantined)
}

/// What [`release_quarantine`] leaves for the caller.
pub enum Release {
    /// Judge stored `table`, if any, against its declaration.
    Judge,
    /// Serve nothing of `table`, for the reason given.
    Hold(String),
}

/// Move the one recorded quarantine of `table` back to its name and forget
/// it, so its stored shape is judged against the declaration again. Holds
/// `table` when the record and the schema leave no single table to judge.
pub async fn release_quarantine(db_handle: &DbHandle, table: &str) -> Result<Release> {
    let Quarantines { stored, missing } = quarantines(db_handle, table).await?;
    if !missing.is_empty() {
        let gone: Vec<String> = missing
            .iter()
            .map(|q| format!("{} ({} rows)", q.name, q.rows))
            .collect();
        return Ok(Release::Hold(format!(
            "Holon recorded {} as holding rows of {table}, but no such table is stored",
            gone.join(", ")
        )));
    }
    let only = match stored.as_slice() {
        [] => return Ok(Release::Judge),
        [only] => only,
        _ => {
            return Ok(Release::Hold(format!(
                "Holon quarantined {table} more than once, so it serves none of those tables"
            )));
        }
    };
    if table_exists(db_handle, table).await? {
        return Ok(Release::Hold(format!(
            "{table} is stored beside {}, which holds its quarantined rows, so Holon serves \
             neither",
            only.name
        )));
    }
    db_handle
        .transaction(vec![
            (
                format!("ALTER TABLE \"{}\" RENAME TO \"{table}\"", only.name),
                vec![],
            ),
            (
                format!("DELETE FROM {QUARANTINE_RECORD} WHERE quarantined = ?"),
                vec![turso::Value::Text(only.name.clone())],
            ),
        ])
        .await
        .map_err(|e| StorageError::SchemaError(format!("releasing {}: {e}", only.name)))?;
    Ok(Release::Judge)
}

/// Drop every stored table recorded as a quarantine of `table` and forget
/// every record of it, in one transaction. A table no record names is never
/// dropped.
pub async fn drop_quarantined(db_handle: &DbHandle, table: &str) -> Result<()> {
    let Quarantines { stored, .. } = quarantines(db_handle, table).await?;
    let mut statements: Vec<(String, Vec<turso::Value>)> = stored
        .iter()
        .map(|q| (format!("DROP TABLE \"{}\"", q.name), vec![]))
        .collect();
    statements.push((
        format!("DELETE FROM {QUARANTINE_RECORD} WHERE type_table = ?"),
        vec![turso::Value::Text(table.to_string())],
    ));
    db_handle
        .transaction(statements)
        .await
        .map_err(|e| StorageError::SchemaError(format!("dropping the quarantines of {table}: {e}")))
}

pub async fn table_exists(db_handle: &DbHandle, table: &str) -> Result<bool> {
    Ok(!TableShape::stored(db_handle, table)
        .await?
        .columns
        .is_empty())
}

/// [`ensure_statement`] for every statement of a schema file.
pub async fn ensure_schema_sql(db_handle: &DbHandle, sql: &str) -> Result<Vec<TableChange>> {
    let statements: Vec<&str> = sql_statements(sql).collect();
    let mut changes = Vec::new();
    for (i, statement) in statements.iter().enumerate() {
        match ensure_statement(db_handle, statement).await {
            Ok(change) => changes.extend(change),
            Err(e) => {
                let not_run: Vec<String> = statements[i + 1..]
                    .iter()
                    .map(|s| statement_head(s))
                    .filter(|head| !head.is_empty())
                    .collect();
                return Err(StorageError::SchemaError(format!(
                    "{e}; not run after it: [{}]",
                    not_run.join("; ")
                )));
            }
        }
    }
    Ok(changes)
}
