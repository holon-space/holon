//! Dual-evaluated guard `Pattern` AST — ADR 0024 Phase-2 risk-elimination spike.
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
//! 2-valued. Naively, `json_extract(...) = 'v'` on a *missing* property is NULL,
//! and `NOT NULL` is NULL (row excluded), whereas in-memory `!(missing == v)`
//! is `true` (row kept) — a divergence exactly under negation, which is the
//! inhibitor arc this ADR leans on. We eliminate the class at the source: every
//! leaf compiles to a **2-valued** SQL fragment (`x IS NOT NULL AND x = v`,
//! `EXISTS(...)`, `NOT EXISTS(...)` are all never-NULL), so boolean algebra over
//! leaves agrees with the in-memory evaluator under arbitrary `And/Or/Not`.
//!
//! # Builtins are environment references, not pattern variables
//!
//! `{today}` is ambient time (ADR 0024 P5, Amendment). It is [`BuiltinRef::Today`]
//! — resolved in memory to [`InMemoryWorld::today`], and in SQL to a read of the
//! deterministic `clock` relation (never `date('now')`). A guard that references
//! a builtin is *clock-driven* (fires per tick, re-fires on rollover):
//! [`Subject::Clock`]. A guard over block fields/tags is *block-driven*:
//! [`Subject::Block`].
//!
//! # Convergence with the scalar `Predicate` (ADR 0024 Q4)
//!
//! The committed end state is `Pattern::Scalar(crate::Predicate)` — the scalar
//! subset re-expressed as a Pattern variant, so no third predicate-ish type
//! appears. It is intentionally **not** implemented in this spike (it would
//! perturb the `flutter_rust_bridge:non_opaque` bridge that `Predicate` crosses);
//! it is recorded here as the direction, per the plan §7.2 done-criteria.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Value;

// ─── AST ────────────────────────────────────────────────────────────────

/// An ambient environment value, interpolated at compile time — *not* a pattern
/// variable. Desugars to a `clock`-relation read (SQL) or [`InMemoryWorld::today`]
/// (in memory), never to a non-deterministic `date('now')`.
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
}

/// The right-hand side of a field comparison: a literal or an environment builtin.
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
    /// Some block in the world matches `path` (parent + name). Uncorrelated with
    /// the subject row; `Not(BlockExists(..))` is the inhibitor / anti-join arc.
    BlockExists(PathPattern),
    And(Vec<Pattern>),
    Or(Vec<Pattern>),
    Not(Box<Pattern>),
}

/// The relation a guard iterates.
///
/// `Clock` = the single `today` row — drives inhibitor rules that must re-fire on
/// day rollover (any guard referencing a [`BuiltinRef`]). `Block` = each block —
/// drives anchors/advice (guards over the subject block's own fields/tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Clock,
    Block,
}

/// A guard = the relation it iterates + the predicate applied to each row. This
/// is the public dual-evaluated surface: [`Guard::evaluate`] / [`Guard::to_sql`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guard {
    pub subject: Subject,
    pub body: Pattern,
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
/// value (ADR 0024 P1b standalone path; the clock is a cache of ambient time, P5).
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

impl Guard {
    /// Evaluate this guard in memory (ADR 0024 P1b standalone path). Returns the
    /// matched bindings, sorted for a stable comparison with the SQL evaluator.
    pub fn evaluate(&self, world: &InMemoryWorld) -> GuardResult {
        let mut bindings = match self.subject {
            Subject::Clock => {
                if self.body.matches(&SubjectRow::Clock, world) {
                    vec![Binding::Today(world.today.clone())]
                } else {
                    vec![]
                }
            }
            Subject::Block => world
                .blocks
                .iter()
                .filter(|b| self.body.matches(&SubjectRow::Block(b), world))
                .map(|b| Binding::Block(b.id.clone()))
                .collect(),
        };
        bindings.sort();
        GuardResult { bindings }
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
    fn matches(&self, row: &SubjectRow, world: &InMemoryWorld) -> bool {
        match self {
            Pattern::Field { field, op, rhs } => {
                let block = match row {
                    SubjectRow::Block(b) => b,
                    SubjectRow::Clock => {
                        panic!("Pattern::Field is block-driven; illegal under a Clock subject")
                    }
                };
                let lhs = field_value(block, field);
                let rhs = resolve_operand(rhs, world);
                compare_2valued(lhs.as_ref(), *op, &rhs)
            }
            Pattern::HasTag(tag) => {
                let block = match row {
                    SubjectRow::Block(b) => b,
                    SubjectRow::Clock => {
                        panic!("Pattern::HasTag is block-driven; illegal under a Clock subject")
                    }
                };
                block.tags.iter().any(|t| t == tag)
            }
            Pattern::BlockExists(path) => path_exists(path, world),
            Pattern::And(ps) => ps.iter().all(|p| p.matches(row, world)),
            Pattern::Or(ps) => ps.iter().any(|p| p.matches(row, world)),
            Pattern::Not(p) => !p.matches(row, world),
        }
    }
}

fn field_value(block: &WorldBlock, field: &FieldRef) -> Option<Value> {
    match field {
        FieldRef::Name => Some(Value::String(block.name.clone())),
        FieldRef::Property(k) => block.properties.get(k).cloned(),
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
/// the leaf's name equals the last segment, its parent's name the previous, etc.
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
/// of hardcoded shapes (the ADR caveat about the PBT `query_ast`'s `json_extract`
/// / `block_tags` literals). The projection implements this later; the spike
/// ships [`CurrentSchema`] targeting the current block/properties/tags shapes.
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

/// The spike's concrete schema: a `block(id, name, parent_id, properties)` table,
/// a `block_tags(block_id, tag)` junction, and a `clock(today)` relation. Mirrors
/// the shapes in the PBT `query_ast` (`json_extract` on `properties`, `block_tags`
/// EXISTS) plus `name`/`parent_id` for path patterns.
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
    subject: SqlSubject<'a>,
}

enum SqlSubject<'a> {
    /// Block-driven: `alias` binds the subject block; no builtin is resolvable.
    Block { alias: &'a str },
    /// Clock-driven: `today_sql` (e.g. `c.today`) resolves [`BuiltinRef::Today`].
    Clock { today_sql: String },
}

impl Guard {
    /// Compile this guard to a matview-eligible `SELECT` (ADR 0024 P1b reactive
    /// path). Returns exactly the rows the in-memory [`Guard::evaluate`] binds:
    /// the clock row (enabled/disabled) for a clock subject, matching block ids
    /// for a block subject. Deterministic — `{today}` reads the `clock` relation,
    /// never `date('now')`.
    pub fn to_sql(&self, schema: &dyn SchemaAbstraction) -> String {
        match self.subject {
            Subject::Clock => {
                let (clock, col) = schema.clock_relation();
                let ctx = SqlCtx {
                    schema,
                    subject: SqlSubject::Clock {
                        today_sql: format!("c.{col}"),
                    },
                };
                format!(
                    "SELECT c.{col} AS binding\nFROM {clock} c\nWHERE {}",
                    self.body.to_sql(&ctx)
                )
            }
            Subject::Block => {
                let block = schema.block_relation();
                let ctx = SqlCtx {
                    schema,
                    subject: SqlSubject::Block { alias: "b" },
                };
                format!(
                    "SELECT {} AS binding\nFROM {block} b\nWHERE {}",
                    schema.id_column("b"),
                    self.body.to_sql(&ctx)
                )
            }
        }
    }
}

impl Pattern {
    /// Compile to a 2-valued SQL boolean fragment (see module docs).
    fn to_sql(&self, ctx: &SqlCtx) -> String {
        match self {
            Pattern::Field { field, op, rhs } => {
                let SqlSubject::Block { alias } = &ctx.subject else {
                    panic!("Pattern::Field is block-driven; illegal under a Clock subject")
                };
                let lhs = match field {
                    FieldRef::Name => ctx.schema.name_column(alias),
                    FieldRef::Property(k) => ctx.schema.property_expr(alias, k),
                };
                let rhs = operand_sql(rhs, ctx);
                cmp_2valued_sql(&lhs, *op, &rhs)
            }
            Pattern::HasTag(tag) => {
                let SqlSubject::Block { alias } = &ctx.subject else {
                    panic!("Pattern::HasTag is block-driven; illegal under a Clock subject")
                };
                ctx.schema.has_tag_exists(alias, tag)
            }
            Pattern::BlockExists(path) => block_exists_sql(path, ctx),
            Pattern::And(ps) => {
                if ps.is_empty() {
                    return "1".to_string();
                }
                let parts: Vec<String> = ps.iter().map(|p| p.to_sql(ctx)).collect();
                format!("({})", parts.join(" AND "))
            }
            Pattern::Or(ps) => {
                if ps.is_empty() {
                    return "0".to_string();
                }
                let parts: Vec<String> = ps.iter().map(|p| p.to_sql(ctx)).collect();
                format!("({})", parts.join(" OR "))
            }
            Pattern::Not(p) => format!("NOT ({})", p.to_sql(ctx)),
        }
    }
}

fn operand_sql(op: &Operand, ctx: &SqlCtx) -> String {
    match op {
        Operand::Lit(v) => sql_value(v),
        Operand::Builtin(b) => builtin_sql(b, ctx),
    }
}

fn builtin_sql(b: &BuiltinRef, ctx: &SqlCtx) -> String {
    match b {
        BuiltinRef::Today => match &ctx.subject {
            SqlSubject::Clock { today_sql } => today_sql.clone(),
            SqlSubject::Block { .. } => panic!(
                "BuiltinRef::Today under a Block subject: guards referencing a builtin are \
                 clock-driven by construction (the parser infers Subject::Clock)"
            ),
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
fn block_exists_sql(path: &PathPattern, ctx: &SqlCtx) -> String {
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
        let seg_sql = segment_sql(seg, ctx);
        wheres.push(format!("{} = {}", ctx.schema.name_column(&alias), seg_sql));
    }
    format!(
        "EXISTS (SELECT 1 FROM {from} WHERE {})",
        wheres.join(" AND ")
    )
}

fn segment_sql(seg: &PathSegment, ctx: &SqlCtx) -> String {
    match seg {
        PathSegment::Lit(s) => sql_string(s),
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

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A JSON path key inside `json_extract(..., '$.KEY')`. Rejects `'` so it cannot
/// break out of the surrounding SQL string literal (parse-don't-validate: the
/// key is an identifier position, validated not escaped).
fn sql_ident(key: &str) -> String {
    assert!(
        !key.contains('\'') && !key.contains('$'),
        "sql_ident: property key {key:?} contains a reserved character"
    );
    key.to_string()
}

// ─── Guard-string parser (the `when:` sugar) ──────────────────────────────

/// A typed guard-parse error — carried (returned), never logged-and-dropped, so
/// a rule block can render its own status. Mirrors `AdviceRuleParseError` style.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GuardParseError {
    #[error("empty guard expression")]
    Empty,
    #[error("unexpected token {token:?} in guard expression")]
    UnexpectedToken { token: String },
    #[error("unexpected end of guard expression (expected {expected})")]
    UnexpectedEnd { expected: String },
    #[error("unknown guard function {name:?} (expected `block_exists` or `has_tag`)")]
    UnknownFunction { name: String },
    #[error("unknown builtin {name:?} (expected `today`)")]
    UnknownBuiltin { name: String },
    #[error("malformed path {path:?}: {reason}")]
    MalformedPath { path: String, reason: String },
    #[error(
        "guard mixes a builtin ({{today}}) with a subject-block predicate \
         (has_tag/field): a builtin makes the guard clock-driven, which has no \
         subject block to test"
    )]
    MixedSubject,
}

impl Guard {
    /// Parse a `when:` guard string (the sugar surface) into a [`Guard`],
    /// inferring the [`Subject`]: any [`BuiltinRef`] ⇒ clock-driven, else
    /// block-driven. Rejects mixing a builtin with a block predicate.
    ///
    /// Grammar: `not`, `and`, `or`, parentheses, and the guard functions
    /// `block_exists("path")` and `has_tag("tag")`.
    pub fn parse(input: &str) -> Result<Guard, GuardParseError> {
        Guard::from_body(parse_guard_body(input)?)
    }

    /// Wrap a body [`Pattern`] into a [`Guard`], inferring the [`Subject`] from
    /// builtin usage (any [`BuiltinRef`] ⇒ [`Subject::Clock`]) and rejecting a
    /// body that mixes a builtin with a subject-block predicate. The single
    /// source of truth both the `when:` sugar and the canonical arc form use, so
    /// the two authoring surfaces yield identical guards.
    pub fn from_body(body: Pattern) -> Result<Guard, GuardParseError> {
        let uses_builtin = pattern_uses_builtin(&body);
        let uses_block = pattern_uses_block_predicate(&body);
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

fn pattern_uses_builtin(p: &Pattern) -> bool {
    match p {
        Pattern::Field { rhs, .. } => matches!(rhs, Operand::Builtin(_)),
        Pattern::HasTag(_) => false,
        Pattern::BlockExists(path) => path
            .segments
            .iter()
            .any(|s| matches!(s, PathSegment::Builtin(_))),
        Pattern::And(ps) | Pattern::Or(ps) => ps.iter().any(pattern_uses_builtin),
        Pattern::Not(p) => pattern_uses_builtin(p),
    }
}

fn pattern_uses_block_predicate(p: &Pattern) -> bool {
    match p {
        Pattern::Field { .. } | Pattern::HasTag(_) => true,
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
            Some(_) => self.parse_call(),
            None => Err(GuardParseError::UnexpectedEnd {
                expected: "a guard function".to_string(),
            }),
        }
    }

    fn parse_call(&mut self) -> Result<Pattern, GuardParseError> {
        let name = self.bump().ok_or(GuardParseError::UnexpectedEnd {
            expected: "a guard function".to_string(),
        })?;
        if self.bump().as_deref() != Some("(") {
            return Err(GuardParseError::UnexpectedToken { token: name });
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
        assert!(g.evaluate(&empty).enabled(), "no journal today ⇒ enabled");

        let present = InMemoryWorld::new(
            vec![
                wb("j", "Journals", None, &[]),
                wb("d", "2026-07-10", Some("j"), &[]),
            ],
            "2026-07-10",
        );
        assert!(
            !g.evaluate(&present).enabled(),
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
        assert!(g.evaluate(&tomorrow).enabled(), "next day ⇒ enabled again");
    }

    #[test]
    fn journal_guard_to_sql_reads_clock_not_now() {
        let g = Guard::parse("not block_exists(\"Journals/{today}\")").unwrap();
        let sql = g.to_sql(&CurrentSchema);
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
        let sql = g.to_sql(&CurrentSchema);
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
        let sql = g.to_sql(&CurrentSchema);
        assert!(sql.contains("IS NOT NULL"), "2-valued eq: {sql}");
        let world = InMemoryWorld::new(vec![wb("a", "A", None, &[])], "d");
        // kind is missing ⇒ eq is false ⇒ NOT is true ⇒ block matches.
        assert!(g.evaluate(&world).enabled());
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

    #[test]
    fn empty_guard_is_typed_error() {
        assert_eq!(Guard::parse("   ").unwrap_err(), GuardParseError::Empty);
    }
}
