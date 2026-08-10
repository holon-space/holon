//! The evaluation seam for declared `#[require]` guards (ADR 0031 Increment 3).
//!
//! Ruling G1=A: a declared guard is evaluated against the **current** world and
//! refuses before the op fires. The dispatcher asks a yes/no question through
//! [`GuardWorld`] and never learns whether the answer came from SQL or memory.

use async_trait::async_trait;
use holon_api::pattern::Guard;
use holon_api::pattern::SchemaAbstraction;
use holon_api::pattern::Subject;
use holon_api::schema::block;
use holon_api::schema::clock;
use holon_core::Result;
use holon_turso::turso::DbHandle;

/// The row a dispatched op's guard must hold for.
///
/// `Block` carries the op's `id` param — the same subject the ADR 0028
/// boundary seam reads, so the two seams agree on what "the subject" is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardSubject {
    Block(String),
    /// The single `clock` row (`{today}` guards, ADR 0024 P5).
    Clock,
}

/// A guard paired with the row it is being asked about. Constructed only by
/// [`GuardQuery::bind`], so a block-driven guard can never be asked about the
/// clock row or vice versa.
pub struct GuardQuery<'a> {
    guard: &'a Guard,
    subject: GuardSubject,
}

impl<'a> GuardQuery<'a> {
    /// Pair `guard` with the subject named by an op's `id` param.
    ///
    /// # Errors
    /// A block-driven guard on an op that names no subject is undispatchable:
    /// there is no row to evaluate it for, and passing it would be fail-open.
    pub fn bind(guard: &'a Guard, op_id: Option<&str>) -> Result<Self> {
        let subject = match guard.subject {
            Subject::Block => {
                let id = op_id.ok_or_else(|| {
                    "declared guard is block-driven but the operation names no `id` subject; \
                     it cannot be evaluated and must not be waved through"
                        .to_string()
                })?;
                GuardSubject::Block(id.to_string())
            }
            Subject::Clock => GuardSubject::Clock,
        };
        Ok(Self { guard, subject })
    }

    pub fn guard(&self) -> &Guard {
        self.guard
    }

    pub fn subject(&self) -> &GuardSubject {
        &self.subject
    }
}

/// Answers "does this guard hold for this row, right now".
#[async_trait]
pub trait GuardWorld: Send + Sync {
    async fn guard_holds(&self, query: &GuardQuery<'_>) -> Result<bool>;
}

/// The production projection's shape, for the guard compiler.
///
/// Two deliberate deviations from the spike's [`holon_api::pattern::
/// CurrentSchema`]: the block relation has no `name` column (a block's name is
/// its `content`), and `clock` holds one row PER GRAIN, so `{today}` must read
/// the day row or a guard would bind three differently-formatted labels.
///
/// The columns named here come from the ONE schema declaration
/// (`holon_api::schema`), which is also the vocabulary `#[reads]`/`#[emits]`
/// parse against — so a rename moves both together.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionSchema;

impl SchemaAbstraction for ProjectionSchema {
    fn block_relation(&self) -> &str {
        crate::storage::BLOCK_READ_TABLE
    }
    fn id_column(&self, alias: &str) -> String {
        format!("{alias}.{}", block::ID)
    }
    fn name_column(&self, alias: &str) -> String {
        format!("{alias}.{}", block::CONTENT)
    }
    fn property_expr(&self, alias: &str, key: &str) -> String {
        format!(
            "json_extract({alias}.{}, '$.{}')",
            block::PROPERTIES,
            holon_api::pattern::sql_ident(key)
        )
    }
    fn parent_id_column(&self, alias: &str) -> String {
        format!("{alias}.{}", block::PARENT_ID)
    }
    fn has_tag_exists(&self, alias: &str, tag: &str) -> String {
        format!(
            "EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = {alias}.{} AND bt.tag = {})",
            block::ID,
            holon_api::pattern::sql_string(tag)
        )
    }
    fn clock_relation(&self) -> (&'static str, &'static str) {
        (
            "(SELECT today FROM clock WHERE grain = 'day')",
            clock::TODAY,
        )
    }
}

/// Evaluates guards against the SQL projection (OQ-1 = (a)).
pub struct SqlGuardWorld {
    db: DbHandle,
}

impl SqlGuardWorld {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GuardWorld for SqlGuardWorld {
    async fn guard_holds(&self, query: &GuardQuery<'_>) -> Result<bool> {
        let guard = query.guard();
        let (sql, params) = match query.subject() {
            GuardSubject::Block(id) => (
                guard.to_sql_bound(&ProjectionSchema),
                vec![turso::Value::Text(id.clone())],
            ),
            GuardSubject::Clock => (guard.to_sql_any(&ProjectionSchema), vec![]),
        };
        let rows = self
            .db
            .query_positional(&sql, params)
            .await
            .map_err(|e| format!("guard evaluation failed for:\n{sql}\n\nerror: {e}"))?;
        Ok(!rows.is_empty())
    }
}
