//! Dual-evaluated guard `Pattern` AST — ADR 0024 Phase-2 risk-elimination
//! spike.
//!
//! One guard *semantics*, two *evaluators* that must agree (ADR 0024 P1b):
//! - [`Guard::evaluate`] runs the guard in memory over an [`InMemoryWorld`]
//!   (the standalone / `holon-engine` path);
//! - [`Guard::to_sql`] compiles the guard to a matview-eligible `SELECT` over a
//!   projection-owned [`SchemaAbstraction`] (the reactive / Turso-CDC path).
//!
//! The agreement between the two is the load-bearing bet of the ADR. It is
//! pinned by the property test in `holon-advice` (the spike's exit gate).
//!
//! # Why every leaf is 2-valued
//!
//! SQL `WHERE` is tri-state (TRUE / FALSE / NULL) while in-memory `bool` is
//! 2-valued. Naively, `json_extract(...) = 'v'` on a *missing* property is
//! NULL, and `NOT NULL` is NULL (row excluded), whereas in-memory `!(missing ==
//! v)` is `true` (row kept) — a divergence exactly under negation, which is the
//! inhibitor arc this ADR leans on. We eliminate the class at the source: every
//! leaf compiles to a **2-valued** SQL fragment (`x IS NOT NULL AND x = v`,
//! `EXISTS(...)`, `NOT EXISTS(...)` are all never-NULL), so boolean algebra
//! over leaves agrees with the in-memory evaluator under arbitrary
//! `And/Or/Not`.
//!
//! # Builtins are environment references, not pattern variables
//!
//! `{today}` is ambient time (ADR 0024 P5, Amendment). It is
//! [`BuiltinRef::Today`] — resolved in memory to [`InMemoryWorld::today`], and
//! in SQL to a read of the deterministic `clock` relation (never
//! `date('now')`). A guard that references a builtin is *clock-driven* (fires
//! per tick, re-fires on rollover): [`Subject::Clock`]. A guard over block
//! fields/tags is *block-driven*: [`Subject::Block`].
//!
//! # Convergence with the scalar `Predicate` (ADR 0024 Q4)
//!
//! The committed end state is `Pattern::Scalar(crate::Predicate)` — the scalar
//! subset re-expressed as a Pattern variant, so no third predicate-ish type
//! appears. It is intentionally **not** implemented in this spike (it would
//! perturb the `flutter_rust_bridge:non_opaque` bridge that `Predicate`
//! crosses); it is recorded here as the direction, per the plan §7.2
//! done-criteria.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::Value;

// ─── AST ────────────────────────────────────────────────────────────────

/// An ambient environment value, interpolated at compile time — *not* a pattern
/// variable. Desugars to a `clock`-relation read (SQL) or
/// [`InMemoryWorld::today`] (in memory), never to a non-deterministic
/// `date('now')`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinRef {
    /// The clock relation's current day (`{today}` / `{clock.today}`).
    Today,
}

/// A field of the *subject block* a block-driven guard iterates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldRef {
    /// The block's name (the leaf identifier used by path patterns).
    Name,
    /// A JSON property of the block, by key.
    Property(String),
    /// A declared column of the subject relation, spelled `relation.name` —
    /// the same vocabulary `#[reads]`/`#[emits]` use.
    Column { relation: String, name: String },
}

/// The right-hand side of a field comparison: a literal or an environment
/// builtin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operand {
    Lit(Value),
    Builtin(BuiltinRef),
}

/// Comparison operator for [`Pattern::Field`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
}

/// One segment of a block path pattern (`"Journals/{today}"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSegment {
    Lit(String),
    Builtin(BuiltinRef),
}

/// A parent/child path pattern; the desugar target of `block_exists(path)`.
///
/// The last segment names the leaf block; each preceding segment names an
/// ancestor (nearest-parent last). "Journals/{today}" =
/// `[Lit("Journals"), Builtin(Today)]`: a block named `{today}` whose parent is
/// named `Journals`. Ancestor matching is a *suffix* match on the parent chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathPattern {
    pub segments: Vec<PathSegment>,
}

/// The guard predicate AST. Every leaf compiles to a 2-valued SQL fragment (see
/// module docs) so the in-memory and SQL evaluators agree under `And/Or/Not`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Compare a field of the subject block to an operand. Block-driven only.
    Field {
        field: FieldRef,
        op: CmpOp,
        rhs: Operand,
    },
    /// The subject block carries `tag`. Block-driven only.
    HasTag(String),
    /// Some block in the world matches `path` (parent + name). Uncorrelated
    /// with the subject row; `Not(BlockExists(..))` is the inhibitor /
    /// anti-join arc.
    BlockExists(PathPattern),
    /// The subject block HAS a parent AND that parent satisfies the inner
    /// pattern. Block-driven only.
    ///
    /// Existential, not implicative: a root block never matches, so
    /// `parent(not has_tag("Page"))` means "has a parent, which is not a page"
    /// — the shape `page_under_non_page_prohibited` needs, where a parentless
    /// block is legal. A 2-valued `EXISTS` in SQL, so it stays sound under
    /// `Not`.
    Parent(Box<Pattern>),
    And(Vec<Pattern>),
    Or(Vec<Pattern>),
    Not(Box<Pattern>),
}

/// The relation a guard iterates.
///
/// `Clock` = the single `today` row — drives inhibitor rules that must re-fire
/// on day rollover (any guard referencing a [`BuiltinRef`]). `Block` = each
/// block — drives anchors/advice (guards over the subject block's own
/// fields/tags).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Clock,
    Block,
    /// A declared relation with a queryable binding, iterated row by row.
    Relation(String),
}

/// A guard = the relation it iterates + the predicate applied to each row. This
/// is the public dual-evaluated surface: [`Guard::evaluate`] /
/// [`Guard::to_sql`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guard {
    pub subject: Subject,
    pub body: Pattern,
}

/// An operation's declared precondition (ADR 0031). Non-defaultable, with
/// [`OpGuard::None`] as the explicit "this op declares no precondition" — a
/// stated fact, never an absence, so an `OperationDescriptor` cannot be silent
/// about its guard.
///
/// A separate type from [`Guard`] rather than a `None` variant on it: a
/// `holon_rule` always HAS a guard, and adding an unguarded variant to `Guard`
/// would make that illegal state representable at every rule site.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpGuard {
    /// Declares no precondition. Always enabled.
    None,
    /// The declared relational precondition (P6=A: predicates over the state
    /// the op touches; parameter validity belongs in typed params).
    ///
    /// `source` is the developer's own `#[require]` text (several attributes
    /// joined with ` and `). The dispatcher's refusal quotes it verbatim rather
    /// than re-rendering the AST, so a refusal never shows a developer a guard
    /// they did not write.
    Declared { guard: Guard, source: String },
}

impl OpGuard {
    /// Parse a guard string into a declared op guard. The `#[require("…")]`
    /// macro calls this at expansion time, so a parse error is a compile error.
    pub fn parse(input: &str) -> Result<OpGuard, GuardParseError> {
        Ok(OpGuard::Declared {
            guard: Guard::parse(input)?,
            source: input.to_string(),
        })
    }

    /// The declared guard, or `None` when the op declares no precondition.
    pub fn guard(&self) -> Option<&Guard> {
        match self {
            OpGuard::None => Option::None,
            OpGuard::Declared { guard, .. } => Some(guard),
        }
    }

    /// The declared guard's source text, for diagnostics.
    pub fn source(&self) -> Option<&str> {
        match self {
            OpGuard::None => Option::None,
            OpGuard::Declared { source, .. } => Some(source),
        }
    }
}

// ─── In-memory world + evaluation ─────────────────────────────────────────

/// A block in the in-memory evaluation world.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldBlock {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub properties: HashMap<String, Value>,
    pub tags: Vec<String>,
}

/// The in-memory evaluator's world: a block collection + the clock's `today`
/// value (ADR 0024 P1b standalone path; the clock is a cache of ambient time,
/// P5).
#[derive(Debug, Clone)]
pub struct InMemoryWorld {
    pub blocks: Vec<WorldBlock>,
    pub today: String,
}

impl InMemoryWorld {
    pub fn new(blocks: Vec<WorldBlock>, today: impl Into<String>) -> Self {
        Self {
            blocks,
            today: today.into(),
        }
    }

    fn block_by_id(&self, id: &str) -> Option<&WorldBlock> {
        self.blocks.iter().find(|b| b.id == id)
    }
}

/// A matched subject row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Binding {
    /// The clock's `today` value (for a [`Subject::Clock`] guard).
    Today(String),
    /// A block id (for a [`Subject::Block`] guard).
    Block(String),
}

/// The result of an in-memory guard evaluation: the matched rows (bindings).
/// A guard is *enabled* iff it produced at least one binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardResult {
    pub bindings: Vec<Binding>,
}

impl GuardResult {
    pub fn enabled(&self) -> bool {
        !self.bindings.is_empty()
    }
}

/// The row a guard body is being tested against. The clock row carries no
/// payload here — `{today}` resolves through [`InMemoryWorld::today`], which is
/// that row's only column.
enum SubjectRow<'a> {
    Clock,
    Block(&'a WorldBlock),
}

/// A guard whose body does not fit its subject — a predicate reached a row kind
/// it cannot read. Carried rather than panicked: a guard can arrive
/// deserialized, and the render layer evaluates guards on the paint path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GuardEvalError {
    #[error("{predicate} needs a subject block; this guard's subject is {subject}")]
    NeedsBlockSubject {
        predicate: &'static str,
        subject: String,
    },
    #[error("{{today}} resolves against the clock row; this guard's subject is {subject}")]
    NeedsClockSubject { subject: String },
    #[error("a guard over relation {relation:?} is not evaluated by this world")]
    WrongWorld { relation: String },
    #[error("relation {relation:?} has no table a guard can iterate")]
    UnboundRelation { relation: String },
    #[error(
        "the row carries no column {column:?} of relation {relation:?}, so the guard has no answer"
    )]
    MissingColumn { relation: String, column: String },
}

impl Subject {
    fn describe(&self) -> String {
        match self {
            Subject::Clock => "clock".to_string(),
            Subject::Block => "block".to_string(),
            Subject::Relation(r) => format!("relation {r:?}"),
        }
    }
}

/// One row's column values, for a guard evaluated where no database is
/// reachable — the render layer holds a row and must decide synchronously.
pub trait GuardRowValues {
    fn column(&self, name: &str) -> Option<&Value>;
}

impl GuardRowValues for std::collections::HashMap<String, Value> {
    fn column(&self, name: &str) -> Option<&Value> {
        self.get(name)
    }
}

impl Guard {
    /// Does this relation-scoped guard hold for `row`?
    ///
    /// The SQL evaluator answers the same question over the same relation; a
    /// column the row does not carry is an error rather than `false`, because
    /// the two evaluators can only agree on a row that carries the guard's
    /// columns.
    pub fn evaluate_row(&self, row: &dyn GuardRowValues) -> Result<bool, GuardEvalError> {
        let Subject::Relation(relation) = &self.subject else {
            return Err(GuardEvalError::WrongWorld {
                relation: self.subject.describe(),
            });
        };
        self.body.matches_row(row, relation)
    }

    /// Evaluate this guard in memory (ADR 0024 P1b standalone path). Returns
    /// the matched bindings, sorted for a stable comparison with the SQL
    /// evaluator.
    pub fn evaluate(&self, world: &InMemoryWorld) -> Result<GuardResult, GuardEvalError> {
        let subject = self.subject.describe();
        let mut bindings = match &self.subject {
            Subject::Clock => {
                if self.body.matches(&SubjectRow::Clock, world, &subject)? {
                    vec![Binding::Today(world.today.clone())]
                } else {
                    vec![]
                }
            }
            Subject::Block => {
                let mut out = Vec::new();
                for b in &world.blocks {
                    if self.body.matches(&SubjectRow::Block(b), world, &subject)? {
                        out.push(Binding::Block(b.id.clone()));
                    }
                }
                out
            }
            Subject::Relation(relation) => {
                return Err(GuardEvalError::WrongWorld {
                    relation: relation.clone(),
                });
            }
        };
        bindings.sort();
        Ok(GuardResult { bindings })
    }
}

fn resolve_builtin(b: &BuiltinRef, world: &InMemoryWorld) -> String {
    match b {
        BuiltinRef::Today => world.today.clone(),
    }
}

fn resolve_operand(op: &Operand, world: &InMemoryWorld) -> Value {
    match op {
        Operand::Lit(v) => v.clone(),
        Operand::Builtin(b) => Value::String(resolve_builtin(b, world)),
    }
}

fn resolve_segment(seg: &PathSegment, world: &InMemoryWorld) -> String {
    match seg {
        PathSegment::Lit(s) => s.clone(),
        PathSegment::Builtin(b) => resolve_builtin(b, world),
    }
}

impl Pattern {
    fn matches_row(
        &self,
        row: &dyn GuardRowValues,
        relation: &str,
    ) -> Result<bool, GuardEvalError> {
        match self {
            Pattern::Field {
                field: FieldRef::Column { name, .. },
                op,
                rhs,
            } => {
                let lhs = row
                    .column(name)
                    .ok_or_else(|| GuardEvalError::MissingColumn {
                        relation: relation.to_string(),
                        column: name.clone(),
                    })?;
                let Operand::Lit(rhs) = rhs else {
                    return Err(GuardEvalError::NeedsClockSubject {
                        subject: format!("relation {relation:?}"),
                    });
                };
                Ok(compare_2valued(Some(lhs), *op, rhs))
            }
            Pattern::And(ps) => {
                for p in ps {
                    if !p.matches_row(row, relation)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Pattern::Or(ps) => {
                for p in ps {
                    if p.matches_row(row, relation)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Pattern::Not(p) => Ok(!p.matches_row(row, relation)?),
            Pattern::Field { .. }
            | Pattern::HasTag(_)
            | Pattern::BlockExists(_)
            | Pattern::Parent(_) => Err(GuardEvalError::NeedsBlockSubject {
                predicate: "this predicate",
                subject: format!("relation {relation:?}"),
            }),
        }
    }

    fn matches(
        &self,
        row: &SubjectRow,
        world: &InMemoryWorld,
        subject: &str,
    ) -> Result<bool, GuardEvalError> {
        let block_row = |predicate: &'static str| match row {
            SubjectRow::Block(b) => Ok(*b),
            SubjectRow::Clock => Err(GuardEvalError::NeedsBlockSubject {
                predicate,
                subject: subject.to_string(),
            }),
        };
        match self {
            Pattern::Field { field, op, rhs } => {
                let block = block_row("a field comparison")?;
                let lhs = field_value(block, field);
                let rhs = resolve_operand(rhs, world);
                Ok(compare_2valued(lhs.as_ref(), *op, &rhs))
            }
            Pattern::HasTag(tag) => {
                let block = block_row("has_tag")?;
                Ok(block.tags.iter().any(|t| t == tag))
            }
            Pattern::BlockExists(path) => Ok(path_exists(path, world)),
            Pattern::Parent(inner) => {
                let block = block_row("parent")?;
                match block
                    .parent_id
                    .as_deref()
                    .and_then(|pid| world.block_by_id(pid))
                {
                    Some(parent) => inner.matches(&SubjectRow::Block(parent), world, subject),
                    None => Ok(false),
                }
            }
            Pattern::And(ps) => {
                for p in ps {
                    if !p.matches(row, world, subject)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Pattern::Or(ps) => {
                for p in ps {
                    if p.matches(row, world, subject)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Pattern::Not(p) => Ok(!p.matches(row, world, subject)?),
        }
    }
}

fn field_value(block: &WorldBlock, field: &FieldRef) -> Option<Value> {
    match field {
        FieldRef::Name => Some(Value::String(block.name.clone())),
        FieldRef::Property(k) => block.properties.get(k).cloned(),
        // A relation column names no part of a block; the block world binds
        // none of them.
        FieldRef::Column { .. } => None,
    }
}

/// 2-valued comparison. A missing field is never a match (fail-shut), mirroring
/// [`crate::Predicate`]'s numeric comparisons and the SQL `IS NOT NULL` guard.
fn compare_2valued(lhs: Option<&Value>, op: CmpOp, rhs: &Value) -> bool {
    match op {
        CmpOp::Eq => lhs == Some(rhs),
        CmpOp::Ne => lhs != Some(rhs),
        CmpOp::Gt | CmpOp::Lt | CmpOp::Gte | CmpOp::Lte => {
            let (Some(l), Some(r)) = (lhs.and_then(Value::as_f64), rhs.as_f64()) else {
                return false;
            };
            match op {
                CmpOp::Gt => l > r,
                CmpOp::Lt => l < r,
                CmpOp::Gte => l >= r,
                CmpOp::Lte => l <= r,
                _ => unreachable!(),
            }
        }
    }
}

/// Does some block in the world match `path`? Suffix-matches the parent chain:
/// the leaf's name equals the last segment, its parent's name the previous,
/// etc.
fn path_exists(path: &PathPattern, world: &InMemoryWorld) -> bool {
    assert!(
        !path.segments.is_empty(),
        "path_exists: empty path pattern is ill-formed (parser must reject)"
    );
    let names: Vec<String> = path
        .segments
        .iter()
        .map(|s| resolve_segment(s, world))
        .collect();
    world
        .blocks
        .iter()
        .any(|leaf| block_matches_chain(leaf, &names, world))
}

fn block_matches_chain(leaf: &WorldBlock, names: &[String], world: &InMemoryWorld) -> bool {
    let mut cur = Some(leaf);
    for name in names.iter().rev() {
        let Some(block) = cur else {
            return false;
        };
        if &block.name != name {
            return false;
        }
        cur = block
            .parent_id
            .as_deref()
            .and_then(|pid| world.block_by_id(pid));
    }
    true
}

// ─── Schema abstraction (projection-owned) ─────────────────────────────────

/// The projection-owned schema the guard compiler targets. Keeps `to_sql` free
/// of hardcoded shapes (the ADR caveat about the PBT `query_ast`'s
/// `json_extract` / `block_tags` literals). The projection implements this
/// later; the spike ships [`CurrentSchema`] targeting the current
/// block/properties/tags shapes.
pub trait SchemaAbstraction {
    /// The block relation (FROM target for a block-driven guard).
    fn block_relation(&self) -> &str;
    /// SQL for a block alias's id column.
    fn id_column(&self, alias: &str) -> String;
    /// SQL for a block alias's name column.
    fn name_column(&self, alias: &str) -> String;
    /// SQL reading a JSON property of a block alias.
    fn property_expr(&self, alias: &str, key: &str) -> String;
    /// SQL for a block alias's parent-id column (path chain joins).
    fn parent_id_column(&self, alias: &str) -> String;
    /// A 2-valued `EXISTS(...)` fragment: block `alias` carries `tag`.
    fn has_tag_exists(&self, alias: &str, tag: &str) -> String;
    /// The clock relation and its `today` column (`{today}` read arc, P5).
    fn clock_relation(&self) -> (&'static str, &'static str);
}

/// The spike's concrete schema: a `block(id, name, parent_id, properties)`
/// table, a `block_tags(block_id, tag)` junction, and a `clock(today)`
/// relation. Mirrors the shapes in the PBT `query_ast` (`json_extract` on
/// `properties`, `block_tags` EXISTS) plus `name`/`parent_id` for path
/// patterns.
#[derive(Debug, Clone, Copy, Default)]
pub struct CurrentSchema;

impl SchemaAbstraction for CurrentSchema {
    fn block_relation(&self) -> &str {
        "block"
    }
    fn id_column(&self, alias: &str) -> String {
        format!("{alias}.id")
    }
    fn name_column(&self, alias: &str) -> String {
        format!("{alias}.name")
    }
    fn property_expr(&self, alias: &str, key: &str) -> String {
        format!("json_extract({alias}.properties, '$.{}')", sql_ident(key))
    }
    fn parent_id_column(&self, alias: &str) -> String {
        format!("{alias}.parent_id")
    }
    fn has_tag_exists(&self, alias: &str, tag: &str) -> String {
        format!(
            "EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = {}.id AND bt.tag = {})",
            alias,
            sql_string(tag)
        )
    }
    fn clock_relation(&self) -> (&'static str, &'static str) {
        ("clock", "today")
    }
}

// ─── SQL compilation ──────────────────────────────────────────────────────

/// The compilation context threaded through [`Pattern::to_sql`].
struct SqlCtx<'a> {
    schema: &'a dyn SchemaAbstraction,
    /// The subject alias and, for a clock subject, the resolved `{today}` SQL.
    subject: SqlSubject,
    /// How many [`Pattern::Parent`] hops deep we are; keeps the ancestor
    /// aliases (`par1`, `par2`, …) distinct along a nesting chain.
    parent_depth: usize,
}

impl<'a> SqlCtx<'a> {
    /// The context one `parent(...)` hop up, binding the ancestor alias.
    fn up(&self) -> (String, SqlCtx<'a>) {
        let depth = self.parent_depth + 1;
        let alias = format!("par{depth}");
        (
            alias.clone(),
            SqlCtx {
                schema: self.schema,
                subject: SqlSubject::Block { alias },
                parent_depth: depth,
            },
        )
    }
}

enum SqlSubject {
    /// Block-driven: `alias` binds the subject block; no builtin is resolvable.
    Block { alias: String },
    /// Clock-driven: `today_sql` (e.g. `c.today`) resolves
    /// [`BuiltinRef::Today`].
    Clock { today_sql: String },
    /// Relation-driven: `alias` binds one row of the relation's table.
    Relation { alias: String },
}

impl Guard {
    /// Compile this guard to a matview-eligible `SELECT` (ADR 0024 P1b reactive
    /// path). Returns exactly the rows the in-memory [`Guard::evaluate`] binds:
    /// the clock row (enabled/disabled) for a clock subject, matching block ids
    /// for a block subject. Deterministic — `{today}` reads the `clock`
    /// relation, never `date('now')`.
    pub fn to_sql(&self, schema: &dyn SchemaAbstraction) -> Result<String, GuardEvalError> {
        match &self.subject {
            Subject::Clock => {
                let (clock, col) = schema.clock_relation();
                let ctx = SqlCtx {
                    schema,
                    subject: SqlSubject::Clock {
                        today_sql: format!("c.{col}"),
                    },
                    parent_depth: 0,
                };
                Ok(format!(
                    "SELECT c.{col} AS binding\nFROM {clock} c\nWHERE {}",
                    self.body.to_sql(&ctx)?
                ))
            }
            Subject::Block => {
                let block = schema.block_relation();
                let ctx = SqlCtx {
                    schema,
                    subject: SqlSubject::Block {
                        alias: "b".to_string(),
                    },
                    parent_depth: 0,
                };
                Ok(format!(
                    "SELECT {} AS binding\nFROM {block} b\nWHERE {}",
                    schema.id_column("b"),
                    self.body.to_sql(&ctx)?
                ))
            }
            Subject::Relation(relation) => {
                let binding = crate::schema::builtin_entity(relation)
                    .and_then(|e| e.binding)
                    .ok_or_else(|| GuardEvalError::UnboundRelation {
                        relation: relation.clone(),
                    })?;
                let ctx = SqlCtx {
                    schema,
                    subject: SqlSubject::Relation {
                        alias: "r".to_string(),
                    },
                    parent_depth: 0,
                };
                Ok(format!(
                    "SELECT r.{} AS binding\nFROM {} r\nWHERE {}",
                    binding.id_column,
                    binding.table,
                    self.body.to_sql(&ctx)?
                ))
            }
        }
    }

    /// The dispatcher gate's **subject-bound** query: does the row named by
    /// `?1` appear among [`Guard::to_sql`]'s bindings? Wraps that query rather
    /// than compiling a second one, so the agreement oracle proving `to_sql`
    /// correct covers this shape too.
    pub fn to_sql_bound(&self, schema: &dyn SchemaAbstraction) -> Result<String, GuardEvalError> {
        Ok(format!(
            "SELECT g.binding FROM (\n{}\n) g WHERE g.binding = ?1 LIMIT 1",
            self.to_sql(schema)?
        ))
    }

    /// The dispatcher gate's **unbound** query: does the guard bind ANY row?
    /// The clock-subject form, where the single clock row is the only subject.
    pub fn to_sql_any(&self, schema: &dyn SchemaAbstraction) -> Result<String, GuardEvalError> {
        Ok(format!(
            "SELECT g.binding FROM (\n{}\n) g LIMIT 1",
            self.to_sql(schema)?
        ))
    }
}

impl Pattern {
    /// Compile to a 2-valued SQL boolean fragment (see module docs).
    fn to_sql(&self, ctx: &SqlCtx) -> Result<String, GuardEvalError> {
        let block_alias = |predicate: &'static str| match &ctx.subject {
            SqlSubject::Block { alias } => Ok(alias.clone()),
            SqlSubject::Clock { .. } => Err(GuardEvalError::NeedsBlockSubject {
                predicate,
                subject: "clock".to_string(),
            }),
            SqlSubject::Relation { .. } => Err(GuardEvalError::NeedsBlockSubject {
                predicate,
                subject: "a relation".to_string(),
            }),
        };
        Ok(match self {
            Pattern::Field {
                field: FieldRef::Column { relation, name },
                op,
                rhs,
            } => {
                let SqlSubject::Relation { alias } = &ctx.subject else {
                    return Err(GuardEvalError::WrongWorld {
                        relation: relation.clone(),
                    });
                };
                // The column is checked against the relation's declared field
                // list when the guard parses, so it is an identifier here.
                let rhs = operand_sql(rhs, ctx)?;
                cmp_2valued_sql(&format!("{alias}.{name}"), *op, &rhs)
            }
            Pattern::Field { field, op, rhs } => {
                let alias = block_alias("a field comparison")?;
                let lhs = match field {
                    FieldRef::Name => ctx.schema.name_column(&alias),
                    FieldRef::Property(k) => ctx.schema.property_expr(&alias, k),
                    FieldRef::Column { .. } => unreachable!("matched by the arm above"),
                };
                let rhs = operand_sql(rhs, ctx)?;
                cmp_2valued_sql(&lhs, *op, &rhs)
            }
            Pattern::HasTag(tag) => {
                let alias = block_alias("has_tag")?;
                ctx.schema.has_tag_exists(&alias, tag)
            }
            Pattern::BlockExists(path) => block_exists_sql(path, ctx)?,
            Pattern::Parent(inner) => {
                let alias = block_alias("parent")?;
                let (par, up) = ctx.up();
                format!(
                    "EXISTS (SELECT 1 FROM {} {par} WHERE {} = {} AND {})",
                    ctx.schema.block_relation(),
                    ctx.schema.id_column(&par),
                    ctx.schema.parent_id_column(&alias),
                    inner.to_sql(&up)?,
                )
            }
            Pattern::And(ps) => {
                if ps.is_empty() {
                    return Ok("1".to_string());
                }
                let parts = ps
                    .iter()
                    .map(|p| p.to_sql(ctx))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("({})", parts.join(" AND "))
            }
            Pattern::Or(ps) => {
                if ps.is_empty() {
                    return Ok("0".to_string());
                }
                let parts = ps
                    .iter()
                    .map(|p| p.to_sql(ctx))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("({})", parts.join(" OR "))
            }
            Pattern::Not(p) => format!("NOT ({})", p.to_sql(ctx)?),
        })
    }
}

fn operand_sql(op: &Operand, ctx: &SqlCtx) -> Result<String, GuardEvalError> {
    match op {
        Operand::Lit(v) => Ok(sql_value(v)),
        Operand::Builtin(b) => builtin_sql(b, ctx),
    }
}

fn builtin_sql(b: &BuiltinRef, ctx: &SqlCtx) -> Result<String, GuardEvalError> {
    match b {
        BuiltinRef::Today => match &ctx.subject {
            SqlSubject::Clock { today_sql } => Ok(today_sql.clone()),
            SqlSubject::Block { .. } => Err(GuardEvalError::NeedsClockSubject {
                subject: "block".to_string(),
            }),
            SqlSubject::Relation { .. } => Err(GuardEvalError::NeedsClockSubject {
                subject: "a relation".to_string(),
            }),
        },
    }
}

/// 2-valued comparison SQL: a NULL/missing lhs is never a match.
fn cmp_2valued_sql(lhs: &str, op: CmpOp, rhs: &str) -> String {
    match op {
        CmpOp::Eq => format!("({lhs} IS NOT NULL AND {lhs} = {rhs})"),
        CmpOp::Ne => format!("({lhs} IS NULL OR {lhs} <> {rhs})"),
        CmpOp::Gt => format!("({lhs} IS NOT NULL AND {lhs} > {rhs})"),
        CmpOp::Lt => format!("({lhs} IS NOT NULL AND {lhs} < {rhs})"),
        CmpOp::Gte => format!("({lhs} IS NOT NULL AND {lhs} >= {rhs})"),
        CmpOp::Lte => format!("({lhs} IS NOT NULL AND {lhs} <= {rhs})"),
    }
}

/// A `[NOT] EXISTS` anti-join over a chain of parent self-joins. The leaf alias
/// is `p0`; each ancestor is `p1`, `p2`, … up the `parent_id` chain.
fn block_exists_sql(path: &PathPattern, ctx: &SqlCtx) -> Result<String, GuardEvalError> {
    assert!(
        !path.segments.is_empty(),
        "block_exists_sql: empty path pattern is ill-formed (parser must reject)"
    );
    let block = ctx.schema.block_relation();
    // segments: nearest-parent last, so the leaf is the *last* segment → p0.
    // Each ancestor pN joins on pN.id = p{N-1}.parent_id.
    let n = path.segments.len();
    let mut from = format!("{block} p0");
    for i in 1..n {
        let child = format!("p{}", i - 1);
        from.push_str(&format!(
            " JOIN {block} p{i} ON {} = {}",
            ctx.schema.id_column(&format!("p{i}")),
            ctx.schema.parent_id_column(&child),
        ));
    }
    let mut wheres: Vec<String> = Vec::with_capacity(n);
    for (depth, seg) in path.segments.iter().rev().enumerate() {
        let alias = format!("p{depth}");
        let seg_sql = segment_sql(seg, ctx)?;
        wheres.push(format!("{} = {}", ctx.schema.name_column(&alias), seg_sql));
    }
    Ok(format!(
        "EXISTS (SELECT 1 FROM {from} WHERE {})",
        wheres.join(" AND ")
    ))
}

fn segment_sql(seg: &PathSegment, ctx: &SqlCtx) -> Result<String, GuardEvalError> {
    match seg {
        PathSegment::Lit(s) => Ok(sql_string(s)),
        PathSegment::Builtin(b) => builtin_sql(b, ctx),
    }
}

/// SQL literal for a [`Value`]. Cribbed from the PBT `query_ast::sql_value`.
fn sql_value(v: &Value) -> String {
    match v {
        Value::String(s) => sql_string(s),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Null => "NULL".to_string(),
        other => panic!("sql_value: unsupported literal variant for a guard operand: {other:?}"),
    }
}

pub fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A JSON path key inside `json_extract(..., '$.KEY')`. Rejects `'` so it
/// cannot break out of the surrounding SQL string literal
/// (parse-don't-validate: the key is an identifier position, validated not
/// escaped).
pub fn sql_ident(key: &str) -> String {
    assert!(
        !key.contains('\'') && !key.contains('$'),
        "sql_ident: property key {key:?} contains a reserved character"
    );
    key.to_string()
}

// ─── Guard-string parser (the `when:` sugar) ──────────────────────────────

/// A typed guard-parse error — carried (returned), never logged-and-dropped, so
/// a rule block can render its own status. Mirrors `AdviceRuleParseError`
/// style.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GuardParseError {
    #[error("empty guard expression")]
    Empty,
    #[error("unexpected token {token:?} in guard expression")]
    UnexpectedToken { token: String },
    #[error("unexpected end of guard expression (expected {expected})")]
    UnexpectedEnd { expected: String },
    #[error("unknown guard function {name:?} (expected `block_exists`, `has_tag` or `parent`)")]
    UnknownFunction { name: String },
    #[error("unknown builtin {name:?} (expected `today`)")]
    UnknownBuiltin { name: String },
    #[error("malformed path {path:?}: {reason}")]
    MalformedPath { path: String, reason: String },
    #[error(
        "guard mixes a builtin ({{today}}) with a subject-block predicate (has_tag/field): a \
         builtin makes the guard clock-driven, which has no subject block to test"
    )]
    MixedSubject,
    #[error(
        "column reference {field:?} names no relation; write it as `relation.column` (the \
         spelling `#[reads]`/`#[emits]` use)"
    )]
    UnqualifiedColumn { field: String },
    #[error(
        "guard compares columns of two relations ({first:?} and {second:?}); a guard iterates one \
         relation"
    )]
    MixedRelations { first: String, second: String },
    #[error(
        "guard mixes a column comparison on {relation:?} with a block predicate \
         (has_tag/block_exists/parent/{{today}}), which has no row of {relation:?} to test"
    )]
    RelationAndBlockPredicate { relation: String },
    #[error("unknown relation {relation:?} in a column comparison; known relations are {known}")]
    UnknownRelation { relation: String, known: String },
    #[error("relation {relation:?} declares no column {column:?}; its columns are {known}")]
    UnknownColumn {
        relation: String,
        column: String,
        known: String,
    },
    #[error(
        "relation {relation:?} has no table a guard can iterate, so a column comparison on it \
         cannot be evaluated"
    )]
    UnboundRelation { relation: String },
}

impl Guard {
    /// Parse a `when:` guard string (the sugar surface) into a [`Guard`],
    /// inferring the [`Subject`]: any [`BuiltinRef`] ⇒ clock-driven, else
    /// block-driven. Rejects mixing a builtin with a block predicate.
    ///
    /// Grammar: `not`, `and`, `or`, parentheses, and the guard functions
    /// `block_exists("path")`, `has_tag("tag")` and `parent(<expr>)`.
    pub fn parse(input: &str) -> Result<Guard, GuardParseError> {
        Guard::from_body(parse_guard_body(input)?)
    }

    /// Wrap a body [`Pattern`] into a [`Guard`], inferring the [`Subject`] from
    /// builtin usage (any [`BuiltinRef`] ⇒ [`Subject::Clock`]) and rejecting a
    /// body that mixes a builtin with a subject-block predicate. The single
    /// source of truth both the `when:` sugar and the canonical arc form use,
    /// so the two authoring surfaces yield identical guards.
    pub fn from_body(body: Pattern) -> Result<Guard, GuardParseError> {
        let uses_builtin = pattern_uses_builtin(&body);
        let uses_block = pattern_uses_block_predicate(&body);

        if let Some(relation) = sole_column_relation(&body)? {
            if uses_builtin || uses_block {
                return Err(GuardParseError::RelationAndBlockPredicate { relation });
            }
            validate_columns(&relation, &body)?;
            return Ok(Guard {
                subject: Subject::Relation(relation),
                body,
            });
        }

        if uses_builtin && uses_block {
            return Err(GuardParseError::MixedSubject);
        }
        let subject = if uses_builtin {
            Subject::Clock
        } else {
            Subject::Block
        };
        Ok(Guard { subject, body })
    }
}

/// The one relation this body's column comparisons name, or `None` when it has
/// none. Two relations is a refusal — a guard iterates one.
fn sole_column_relation(p: &Pattern) -> Result<Option<String>, GuardParseError> {
    let mut found: Option<String> = None;
    collect_column_relation(p, &mut found)?;
    Ok(found)
}

fn collect_column_relation(p: &Pattern, found: &mut Option<String>) -> Result<(), GuardParseError> {
    match p {
        Pattern::Field {
            field: FieldRef::Column { relation, .. },
            ..
        } => match found {
            Some(first) if first != relation => Err(GuardParseError::MixedRelations {
                first: first.clone(),
                second: relation.clone(),
            }),
            Some(_) => Ok(()),
            None => {
                *found = Some(relation.clone());
                Ok(())
            }
        },
        Pattern::Field { .. } | Pattern::HasTag(_) | Pattern::BlockExists(_) => Ok(()),
        Pattern::And(ps) | Pattern::Or(ps) => {
            for inner in ps {
                collect_column_relation(inner, found)?;
            }
            Ok(())
        }
        Pattern::Not(inner) | Pattern::Parent(inner) => collect_column_relation(inner, found),
    }
}

/// The relation must be declared, carry a queryable binding, and declare every
/// column the body names.
fn validate_columns(relation: &str, body: &Pattern) -> Result<(), GuardParseError> {
    let entity = crate::schema::builtin_entity(relation).ok_or_else(|| {
        GuardParseError::UnknownRelation {
            relation: relation.to_string(),
            known: crate::schema::BUILTIN_SCHEMAS
                .iter()
                .map(|s| s.relation)
                .collect::<Vec<_>>()
                .join(", "),
        }
    })?;
    if entity.binding.is_none() {
        return Err(GuardParseError::UnboundRelation {
            relation: relation.to_string(),
        });
    }
    for column in column_names(body) {
        if entity.field(&column).is_none() {
            return Err(GuardParseError::UnknownColumn {
                relation: relation.to_string(),
                column,
                known: entity
                    .fields
                    .iter()
                    .map(|f| f.name)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    Ok(())
}

fn column_names(p: &Pattern) -> Vec<String> {
    match p {
        Pattern::Field {
            field: FieldRef::Column { name, .. },
            ..
        } => vec![name.clone()],
        Pattern::Field { .. } | Pattern::HasTag(_) | Pattern::BlockExists(_) => vec![],
        Pattern::And(ps) | Pattern::Or(ps) => ps.iter().flat_map(column_names).collect(),
        Pattern::Not(inner) | Pattern::Parent(inner) => column_names(inner),
    }
}

fn pattern_uses_builtin(p: &Pattern) -> bool {
    match p {
        Pattern::Field { rhs, .. } => matches!(rhs, Operand::Builtin(_)),
        Pattern::HasTag(_) => false,
        Pattern::BlockExists(path) => path
            .segments
            .iter()
            .any(|s| matches!(s, PathSegment::Builtin(_))),
        Pattern::And(ps) | Pattern::Or(ps) => ps.iter().any(pattern_uses_builtin),
        Pattern::Not(p) | Pattern::Parent(p) => pattern_uses_builtin(p),
    }
}

fn pattern_uses_block_predicate(p: &Pattern) -> bool {
    match p {
        Pattern::Field {
            field: FieldRef::Column { .. },
            ..
        } => false,
        Pattern::Field { .. } | Pattern::HasTag(_) | Pattern::Parent(_) => true,
        Pattern::BlockExists(_) => false,
        Pattern::And(ps) | Pattern::Or(ps) => ps.iter().any(pattern_uses_block_predicate),
        Pattern::Not(p) => pattern_uses_block_predicate(p),
    }
}

/// Parse the desugared `block_exists`/`has_tag`/boolean guard body into a
/// [`Pattern`], leaving [`Subject`] inference to [`Guard::parse`].
pub fn parse_guard_body(input: &str) -> Result<Pattern, GuardParseError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(GuardParseError::Empty);
    }
    let mut parser = TokenParser { tokens, pos: 0 };
    let pat = parser.parse_or()?;
    if parser.pos != parser.tokens.len() {
        return Err(GuardParseError::UnexpectedToken {
            token: parser.tokens[parser.pos].clone(),
        });
    }
    Ok(pat)
}

fn tokenize(input: &str) -> Result<Vec<String>, GuardParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' | ')' | ',' => {
                tokens.push(c.to_string());
                chars.next();
            }
            '=' | '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(format!("{c}="));
                } else {
                    return Err(GuardParseError::UnexpectedToken {
                        token: c.to_string(),
                    });
                }
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '"' {
                        closed = true;
                        break;
                    }
                    s.push(ch);
                }
                if !closed {
                    return Err(GuardParseError::UnexpectedEnd {
                        expected: "closing `\"`".to_string(),
                    });
                }
                tokens.push(format!("\"{s}"));
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' => {
                let mut s = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' || ch == '.' {
                        s.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(s);
            }
            other => {
                return Err(GuardParseError::UnexpectedToken {
                    token: other.to_string(),
                });
            }
        }
    }
    Ok(tokens)
}

struct TokenParser {
    tokens: Vec<String>,
    pos: usize,
}

impl TokenParser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(|s| s.as_str())
    }

    fn peek_ahead(&self, n: usize) -> Option<&str> {
        self.tokens.get(self.pos + n).map(|s| s.as_str())
    }

    fn bump(&mut self) -> Option<String> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Result<Pattern, GuardParseError> {
        let mut terms = vec![self.parse_and()?];
        while self.peek() == Some("or") {
            self.bump();
            terms.push(self.parse_and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Pattern::Or(terms)
        })
    }

    fn parse_and(&mut self) -> Result<Pattern, GuardParseError> {
        let mut terms = vec![self.parse_not()?];
        while self.peek() == Some("and") {
            self.bump();
            terms.push(self.parse_not()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Pattern::And(terms)
        })
    }

    fn parse_not(&mut self) -> Result<Pattern, GuardParseError> {
        if self.peek() == Some("not") {
            self.bump();
            let inner = self.parse_not()?;
            Ok(Pattern::Not(Box::new(inner)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Pattern, GuardParseError> {
        match self.peek() {
            Some("(") => {
                self.bump();
                let inner = self.parse_or()?;
                if self.bump().as_deref() != Some(")") {
                    return Err(GuardParseError::UnexpectedEnd {
                        expected: "closing `)`".to_string(),
                    });
                }
                Ok(inner)
            }
            Some(_) if matches!(self.peek_ahead(1), Some("==") | Some("!=")) => {
                self.parse_comparison()
            }
            Some(_) => self.parse_call(),
            None => Err(GuardParseError::UnexpectedEnd {
                expected: "a guard function".to_string(),
            }),
        }
    }

    /// `relation.column == "literal"` — the relation-scoped comparison.
    ///
    /// The relation is spelled in the guard text so a parsed [`Guard`] names
    /// its own subject and survives a serde round trip.
    fn parse_comparison(&mut self) -> Result<Pattern, GuardParseError> {
        let lhs = self.bump().ok_or(GuardParseError::UnexpectedEnd {
            expected: "a `relation.column` reference".to_string(),
        })?;
        let (relation, name) = lhs
            .split_once('.')
            .filter(|(r, n)| !r.is_empty() && !n.is_empty())
            .ok_or_else(|| GuardParseError::UnqualifiedColumn { field: lhs.clone() })?;

        let op = match self.bump().as_deref() {
            Some("==") => CmpOp::Eq,
            Some("!=") => CmpOp::Ne,
            other => {
                return Err(GuardParseError::UnexpectedToken {
                    token: other.unwrap_or("").to_string(),
                });
            }
        };

        let raw = self.bump().ok_or(GuardParseError::UnexpectedEnd {
            expected: "a string or integer literal".to_string(),
        })?;
        Ok(Pattern::Field {
            field: FieldRef::Column {
                relation: relation.to_string(),
                name: name.to_string(),
            },
            op,
            rhs: Operand::Lit(parse_literal(&raw)?),
        })
    }

    fn parse_call(&mut self) -> Result<Pattern, GuardParseError> {
        let name = self.bump().ok_or(GuardParseError::UnexpectedEnd {
            expected: "a guard function".to_string(),
        })?;
        if self.bump().as_deref() != Some("(") {
            return Err(GuardParseError::UnexpectedToken { token: name });
        }
        // `parent` takes a nested predicate, not a string argument.
        if name == "parent" {
            let inner = self.parse_or()?;
            if self.bump().as_deref() != Some(")") {
                return Err(GuardParseError::UnexpectedEnd {
                    expected: "closing `)`".to_string(),
                });
            }
            return Ok(Pattern::Parent(Box::new(inner)));
        }
        let arg = self.bump().ok_or(GuardParseError::UnexpectedEnd {
            expected: "a string argument".to_string(),
        })?;
        let arg = arg
            .strip_prefix('"')
            .ok_or_else(|| GuardParseError::UnexpectedToken { token: arg.clone() })?
            .to_string();
        if self.bump().as_deref() != Some(")") {
            return Err(GuardParseError::UnexpectedEnd {
                expected: "closing `)`".to_string(),
            });
        }
        match name.as_str() {
            "block_exists" => Ok(Pattern::BlockExists(parse_path(&arg)?)),
            "has_tag" => Ok(Pattern::HasTag(arg)),
            other => Err(GuardParseError::UnknownFunction {
                name: other.to_string(),
            }),
        }
    }
}

/// A comparison's right-hand side, typed at the point it is read.
///
/// The tokenizer marks a string literal with a leading `"` (an empty literal
/// is that marker alone). Anything that is neither that nor a run of digits is
/// refused by name.
fn parse_literal(raw: &str) -> Result<Value, GuardParseError> {
    if let Some(text) = raw.strip_prefix('"') {
        return Ok(Value::String(text.to_string()));
    }
    raw.parse::<i64>()
        .map(Value::Integer)
        .map_err(|_| GuardParseError::UnexpectedToken {
            token: raw.to_string(),
        })
}

/// Parse a `"Journals/{today}"` path into a [`PathPattern`]. `{name}` segments
/// are builtins; bare segments are literals.
pub fn parse_path(raw: &str) -> Result<PathPattern, GuardParseError> {
    if raw.is_empty() {
        return Err(GuardParseError::MalformedPath {
            path: raw.to_string(),
            reason: "empty".to_string(),
        });
    }
    let mut segments = Vec::new();
    for part in raw.split('/') {
        if part.is_empty() {
            return Err(GuardParseError::MalformedPath {
                path: raw.to_string(),
                reason: "empty path segment".to_string(),
            });
        }
        if let Some(inner) = part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
            segments.push(PathSegment::Builtin(parse_builtin(inner)?));
        } else if part.contains('{') || part.contains('}') {
            return Err(GuardParseError::MalformedPath {
                path: raw.to_string(),
                reason: format!("malformed interpolation in segment {part:?}"),
            });
        } else {
            segments.push(PathSegment::Lit(part.to_string()));
        }
    }
    Ok(PathPattern { segments })
}

/// Parse a builtin name (`today` / `clock.today`) into a [`BuiltinRef`].
pub fn parse_builtin(name: &str) -> Result<BuiltinRef, GuardParseError> {
    match name {
        "today" | "clock.today" => Ok(BuiltinRef::Today),
        other => Err(GuardParseError::UnknownBuiltin {
            name: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relation-scoped column comparison is its own guard subject.
    #[test]
    fn a_column_comparison_parses_into_a_relation_guard() {
        let g = Guard::parse("integration.config_status == \"unconfigured\"")
            .expect("a column comparison must parse");
        assert_eq!(g.subject, Subject::Relation("integration".to_string()));
        assert_eq!(
            g.body,
            Pattern::Field {
                field: FieldRef::Column {
                    relation: "integration".to_string(),
                    name: "config_status".to_string(),
                },
                op: CmpOp::Eq,
                rhs: Operand::Lit(Value::String("unconfigured".to_string())),
            }
        );

        let n = Guard::parse("integration.configurable != 1").expect("`!=` and an integer parse");
        assert_eq!(
            n.body,
            Pattern::Field {
                field: FieldRef::Column {
                    relation: "integration".to_string(),
                    name: "configurable".to_string(),
                },
                op: CmpOp::Ne,
                rhs: Operand::Lit(Value::Integer(1)),
            }
        );

        let empty = Guard::parse("integration.configure_progress == \"\"")
            .expect("the empty-string literal is a value, not junk");
        assert_eq!(
            empty.body,
            Pattern::Field {
                field: FieldRef::Column {
                    relation: "integration".to_string(),
                    name: "configure_progress".to_string(),
                },
                op: CmpOp::Eq,
                rhs: Operand::Lit(Value::String(String::new())),
            }
        );

        let conj = Guard::parse(
            "integration.config_status == \"unconfigured\" and integration.configurable == 1",
        )
        .expect("column comparisons combine with `and`");
        assert_eq!(conj.subject, Subject::Relation("integration".to_string()));
    }

    #[test]
    fn the_existing_guard_forms_are_unchanged() {
        let tag = Guard::parse("has_tag(\"task\")").expect("has_tag parses");
        assert_eq!(tag.subject, Subject::Block);
        assert_eq!(tag.body, Pattern::HasTag("task".to_string()));

        let exists =
            Guard::parse("not block_exists(\"Journals/{today}\")").expect("block_exists parses");
        assert_eq!(exists.subject, Subject::Clock);

        let parent = Guard::parse("parent(has_tag(\"Page\"))").expect("parent parses");
        assert_eq!(parent.subject, Subject::Block);
    }

    #[test]
    fn junk_on_either_side_of_the_comparison_is_refused() {
        // (guard text, a fragment the refusal must carry)
        let cases = [
            ("config_status == \"x\"", "config_status"),
            (
                "integration.config_status == \"a\" and clock.today == \"b\"",
                "clock",
            ),
            ("integration.config_status == 1.5", "1.5"),
            (
                "integration.config_status == \"a\" and has_tag(\"task\")",
                "has_tag",
            ),
            ("integration.not_a_column == \"a\"", "not_a_column"),
            ("nonesuch.field == \"a\"", "nonesuch"),
            ("block.content == \"a\"", "block"),
            ("integration.config_status = \"a\"", "="),
        ];
        for (text, fragment) in cases {
            let err =
                Guard::parse(text).expect_err(&format!("{text:?} must be refused, not accepted"));
            let msg = err.to_string();
            assert!(
                msg.contains(fragment),
                "the refusal for {text:?} must name {fragment:?}, got: {msg}"
            );
        }
    }

    fn wb(id: &str, name: &str, parent: Option<&str>, tags: &[&str]) -> WorldBlock {
        WorldBlock {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent.map(|s| s.to_string()),
            properties: HashMap::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn journal_guard_parses_to_clock_inhibitor() {
        let g = Guard::parse("not block_exists(\"Journals/{today}\")").unwrap();
        assert_eq!(g.subject, Subject::Clock);
        assert_eq!(
            g.body,
            Pattern::Not(Box::new(Pattern::BlockExists(PathPattern {
                segments: vec![
                    PathSegment::Lit("Journals".to_string()),
                    PathSegment::Builtin(BuiltinRef::Today),
                ],
            })))
        );
    }

    #[test]
    fn has_tag_guard_is_block_driven() {
        let g = Guard::parse("has_tag(\"project\")").unwrap();
        assert_eq!(g.subject, Subject::Block);
        assert_eq!(g.body, Pattern::HasTag("project".to_string()));
    }

    #[test]
    fn mixed_subject_is_rejected() {
        let err = Guard::parse("has_tag(\"x\") and block_exists(\"A/{today}\")").unwrap_err();
        assert_eq!(err, GuardParseError::MixedSubject);
    }

    #[test]
    fn journal_guard_evaluates_absent_then_present() {
        let g = Guard::parse("not block_exists(\"Journals/{today}\")").unwrap();

        let empty = InMemoryWorld::new(vec![wb("j", "Journals", None, &[])], "2026-07-10");
        assert!(
            g.evaluate(&empty)
                .expect("a clock guard evaluates")
                .enabled(),
            "no journal today ⇒ enabled"
        );

        let present = InMemoryWorld::new(
            vec![
                wb("j", "Journals", None, &[]),
                wb("d", "2026-07-10", Some("j"), &[]),
            ],
            "2026-07-10",
        );
        assert!(
            !g.evaluate(&present)
                .expect("a clock guard evaluates")
                .enabled(),
            "journal exists today ⇒ disabled"
        );

        // Day rollover re-enables (the clock relation drives re-fire, P5).
        let tomorrow = InMemoryWorld::new(
            vec![
                wb("j", "Journals", None, &[]),
                wb("d", "2026-07-10", Some("j"), &[]),
            ],
            "2026-07-11",
        );
        assert!(
            g.evaluate(&tomorrow)
                .expect("a clock guard evaluates")
                .enabled(),
            "next day ⇒ enabled again"
        );
    }

    #[test]
    fn journal_guard_to_sql_reads_clock_not_now() {
        let g = Guard::parse("not block_exists(\"Journals/{today}\")").unwrap();
        let sql = g
            .to_sql(&CurrentSchema)
            .expect("a block/clock guard compiles");
        assert!(sql.contains("FROM clock c"), "clock-driven FROM: {sql}");
        assert!(sql.contains("c.today"), "leaf reads the clock: {sql}");
        assert!(sql.contains("NOT (EXISTS"), "inhibitor anti-join: {sql}");
        assert!(
            !sql.contains("date('now')"),
            "deterministic, no date('now')"
        );
    }

    #[test]
    fn has_tag_to_sql_is_block_exists() {
        let g = Guard::parse("has_tag(\"project\")").unwrap();
        let sql = g
            .to_sql(&CurrentSchema)
            .expect("a block/clock guard compiles");
        assert!(sql.contains("FROM block b"), "block-driven: {sql}");
        assert!(sql.contains("block_tags"), "tag anti-join: {sql}");
    }

    #[test]
    fn field_eq_is_two_valued_under_negation() {
        // A property-eq that is missing must be *false* (not NULL) so `Not` agrees.
        let g = Guard {
            subject: Subject::Block,
            body: Pattern::Not(Box::new(Pattern::Field {
                field: FieldRef::Property("kind".to_string()),
                op: CmpOp::Eq,
                rhs: Operand::Lit(Value::String("note".to_string())),
            })),
        };
        let sql = g
            .to_sql(&CurrentSchema)
            .expect("a block/clock guard compiles");
        assert!(sql.contains("IS NOT NULL"), "2-valued eq: {sql}");
        let world = InMemoryWorld::new(vec![wb("a", "A", None, &[])], "d");
        // kind is missing ⇒ eq is false ⇒ NOT is true ⇒ block matches.
        assert!(g.evaluate(&world).expect("a guard evaluates").enabled());
    }

    #[test]
    fn unknown_function_is_typed_error() {
        let err = Guard::parse("frobnicate(\"x\")").unwrap_err();
        assert_eq!(
            err,
            GuardParseError::UnknownFunction {
                name: "frobnicate".to_string()
            }
        );
    }

    /// `page_under_non_page` in the guard grammar. The predicate the shared
    /// chokepoint
    /// `holon_core::block_op_catalog::page_under_non_page_prohibited`
    /// computes from two booleans, expressed relationally.
    const PAGE_UNDER_NON_PAGE: &str = "has_tag(\"Page\") and parent(not has_tag(\"Page\"))";

    #[test]
    fn parent_guard_parses_and_is_block_driven() {
        let g = Guard::parse(PAGE_UNDER_NON_PAGE).unwrap();
        assert_eq!(g.subject, Subject::Block);
        assert_eq!(
            g.body,
            Pattern::And(vec![
                Pattern::HasTag("Page".to_string()),
                Pattern::Parent(Box::new(Pattern::Not(Box::new(Pattern::HasTag(
                    "Page".to_string()
                ))))),
            ])
        );
    }

    /// The relational guard reproduces the chokepoint truth table, INCLUDING
    /// the parentless case: a root page has no parent, so `parent(..)` is
    /// existentially false — legal, exactly as `parent_is_page = None` is.
    #[test]
    fn parent_guard_reproduces_the_chokepoint_truth_table() {
        let g = Guard::parse(PAGE_UNDER_NON_PAGE).unwrap();
        // (child tags, parent) → prohibited?
        let cases: [(&[&str], Option<&[&str]>, bool); 6] = [
            (&["Page"], Some(&[]), true),        // page under non-page
            (&["Page"], Some(&["Page"]), false), // page under page
            (&[], Some(&[]), false),             // non-page under non-page
            (&[], Some(&["Page"]), false),       // non-page under page
            (&["Page"], None, false),            // root page
            (&[], None, false),                  // root non-page
        ];
        for (child_tags, parent_tags, prohibited) in cases {
            let mut blocks = vec![WorldBlock {
                id: "c".to_string(),
                name: "child".to_string(),
                parent_id: parent_tags.map(|_| "p".to_string()),
                properties: HashMap::new(),
                tags: child_tags.iter().map(|s| s.to_string()).collect(),
            }];
            if let Some(pt) = parent_tags {
                blocks.push(WorldBlock {
                    id: "p".to_string(),
                    name: "parent".to_string(),
                    parent_id: None,
                    properties: HashMap::new(),
                    tags: pt.iter().map(|s| s.to_string()).collect(),
                });
            }
            let world = InMemoryWorld::new(blocks, "2026-08-10");
            let bound = g
                .evaluate(&world)
                .expect("a guard evaluates")
                .bindings
                .contains(&Binding::Block("c".to_string()));
            assert_eq!(
                bound, prohibited,
                "child {child_tags:?} under parent {parent_tags:?}"
            );
        }
    }

    #[test]
    fn parent_to_sql_is_a_two_valued_exists_on_the_parent_row() {
        let g = Guard::parse(PAGE_UNDER_NON_PAGE).unwrap();
        let sql = g
            .to_sql(&CurrentSchema)
            .expect("a block/clock guard compiles");
        assert!(sql.contains("FROM block par1"), "ancestor alias: {sql}");
        assert!(
            sql.contains("par1.id = b.parent_id"),
            "correlated on the subject's parent: {sql}"
        );
        assert!(
            sql.contains("EXISTS ("),
            "existential, so a root never matches: {sql}"
        );
    }

    /// Nested hops must not collide on one alias.
    #[test]
    fn nested_parent_hops_get_distinct_aliases() {
        let g = Guard::parse("parent(parent(has_tag(\"Page\")))").unwrap();
        let sql = g
            .to_sql(&CurrentSchema)
            .expect("a block/clock guard compiles");
        assert!(sql.contains("par1.id = b.parent_id"), "{sql}");
        assert!(sql.contains("par2.id = par1.parent_id"), "{sql}");
    }

    #[test]
    fn op_guard_none_round_trips_as_a_stated_fact() {
        let json = serde_json::to_string(&OpGuard::None).unwrap();
        assert_eq!(json, r#"{"kind":"none"}"#);
        let back: OpGuard = serde_json::from_str(&json).unwrap();
        assert_eq!(back, OpGuard::None);
        assert!(back.guard().is_none());
    }

    #[test]
    fn empty_guard_is_typed_error() {
        assert_eq!(Guard::parse("   ").unwrap_err(), GuardParseError::Empty);
    }
}
