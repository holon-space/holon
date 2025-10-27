//! In-memory query AST for PBT ground-truth (Track 1E).
//!
//! Goal: a single declarative description of "which blocks should be in the
//! result set" that can be (a) **evaluated** against the PBT reference state
//! (a `HashMap<EntityUri, Block>`) and (b) **compiled** to the canonical
//! `holon_sql` form the live SUT consumes today.
//!
//! Both outputs are produced from the same AST so they cannot drift. The PBT
//! invariant runs the SUT-rendered query result through the same set
//! comparison as `evaluate(ast, ref_state)` — any divergence is a real bug
//! in either the SQL pipeline or our reference model.
//!
//! ## Minimal surface
//!
//! The kill condition is "more than 10 operations". This module ships with:
//!   - `QueryAst` (single struct; FROM + filters + sort + limit)
//!   - `Predicate` enum with 7 variants: `PropEq`, `PropNe`, `Membership`,
//!     `EdgeExists`, `And`, `Or`, `Not`
//!   - `EdgeRef`, `SortKey` (helpers, not ops)
//!
//! That's enough to express the canonical now-query target verbatim.

use std::collections::HashMap;

use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

/// Reference (block-relative) on which a property/edge predicate is evaluated.
///
/// `Self` = "this row". `Edge { name }` = "the row currently bound by an
/// EdgeExists predicate to traverse `name`". The AST is intentionally
/// shallow — only one level of correlation is needed for the now-query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Alias {
    /// Outer block alias (`b` in the canonical SQL).
    Outer,
    /// The block on the *target* side of the edge currently being evaluated
    /// (the `bl` alias in the now-query NOT EXISTS clause).
    EdgeTarget,
}

impl Alias {
    fn sql_outer(&self) -> &'static str {
        match self {
            Alias::Outer => "b",
            Alias::EdgeTarget => "bl",
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// One sort key. `key` is either a property name (`"priority"`) or the
/// pseudo-key `"id"` (the canonical now-query uses both).
#[derive(Debug, Clone)]
pub struct SortKey {
    pub key: String,
    pub dir: SortDir,
}

/// Reference to an edge — i.e. a junction table relating blocks to other
/// blocks (`task_blockers`) or to scalar values (`block_tags`).
///
/// `Block` edges traverse to another block on the target side; `Scalar`
/// edges store a literal payload (e.g. a tag string).
#[derive(Debug, Clone)]
pub enum EdgeRef {
    /// `task_blockers(blocked_id, blocker_id)` — the inner row is the
    /// *blocker* block, accessible through `Alias::EdgeTarget`.
    BlockedBy,
    /// `block_tags(block_id, tag)` — the inner side is a tag value, not a
    /// block; `inner` predicates may not use `Alias::EdgeTarget`.
    Tag(String),
}

/// Predicate language. Seven variants — each maps 1:1 to the canonical SQL.
///
/// Stays narrow on purpose: the `gql-to-sql` compiler's job is to
/// expand a richer surface into SQL. This AST is the *PBT ground truth*,
/// not a general query engine.
#[derive(Debug, Clone)]
pub enum Predicate {
    /// `json_extract(<alias>.properties, '$.<key>') = <value>`
    PropEq {
        alias: Alias,
        key: String,
        value: Value,
    },
    /// `COALESCE(json_extract(<alias>.properties, '$.<key>'), '') <> <value>`
    /// — the `COALESCE` mirrors the canonical now-query's NULL handling.
    PropNe {
        alias: Alias,
        key: String,
        value: Value,
    },
    /// Tag membership — `EXISTS (SELECT 1 FROM block_tags ...)`. Inner side
    /// is structural (no nested predicates); the tag value lives on the
    /// `EdgeRef`.
    Membership {
        negated: bool,
        edge: EdgeRef,
    },
    /// Edge-traversal exists clause. Maps to `[NOT] EXISTS (SELECT 1 FROM
    /// <junction> JOIN block bl ON ... WHERE <inner>)` — i.e. the now-query's
    /// blocker subquery shape.
    EdgeExists {
        negated: bool,
        edge: EdgeRef,
        inner: Option<Box<Predicate>>,
    },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}

/// The query itself. Single struct so we don't burn an enum variant on the
/// "FROM" / "ORDER BY" / "LIMIT" clauses (each is a *field*, not an op).
///
/// Produces the canonical SQL form — `b.*` projection, ORDER BY, LIMIT.
/// Add `Project` later if multiple projections are needed.
#[derive(Debug, Clone)]
pub struct QueryAst {
    /// Always `"block"` for now — kept as a field so the AST can later
    /// extend without a breaking change.
    pub entity: String,
    pub filter: Option<Predicate>,
    pub sort: Vec<SortKey>,
    pub limit: Option<usize>,
}

impl QueryAst {
    /// Construct an empty query over `block` with no filter / sort / limit.
    pub fn from_block() -> Self {
        Self {
            entity: "block".to_string(),
            filter: None,
            sort: Vec::new(),
            limit: None,
        }
    }

    pub fn with_filter(mut self, pred: Predicate) -> Self {
        self.filter = Some(pred);
        self
    }

    pub fn with_sort(mut self, sort: Vec<SortKey>) -> Self {
        self.sort = sort;
        self
    }

    pub fn with_limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }
}

// ─── SQL compilation ────────────────────────────────────────────────────

fn sql_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Null => "NULL".to_string(),
        other => panic!(
            "sql_value: unsupported value variant for AST literal: {other:?} \
             (extend QueryAst::sql_value if a new shape is needed)"
        ),
    }
}

fn json_extract(alias: &Alias, key: &str) -> String {
    format!(
        "json_extract({}.properties, '$.{}')",
        alias.sql_outer(),
        key
    )
}

fn pred_to_sql(pred: &Predicate) -> String {
    match pred {
        Predicate::PropEq { alias, key, value } => {
            format!("{} = {}", json_extract(alias, key), sql_value(value))
        }
        Predicate::PropNe { alias, key, value } => {
            format!(
                "COALESCE({}, '') <> {}",
                json_extract(alias, key),
                sql_value(value)
            )
        }
        Predicate::Membership { negated, edge } => match edge {
            EdgeRef::Tag(tag) => {
                let inner = format!(
                    "SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = {}",
                    sql_value(&Value::String(tag.clone()))
                );
                if *negated {
                    format!("NOT EXISTS ({inner})")
                } else {
                    format!("EXISTS ({inner})")
                }
            }
            EdgeRef::BlockedBy => panic!(
                "Membership predicate is only meaningful for scalar edges (Tag); \
                 use EdgeExists for BlockedBy"
            ),
        },
        Predicate::EdgeExists {
            negated,
            edge,
            inner,
        } => {
            let body = match edge {
                EdgeRef::BlockedBy => {
                    let where_clause = match inner {
                        Some(p) => format!(" AND {}", pred_to_sql(p)),
                        None => String::new(),
                    };
                    format!(
                        "SELECT 1 FROM task_blockers tb \
                         JOIN block bl ON bl.id = tb.blocker_id \
                         WHERE tb.blocked_id = b.id{where_clause}"
                    )
                }
                EdgeRef::Tag(_) => {
                    panic!("EdgeExists is for block-typed edges; use Membership for tags")
                }
            };
            if *negated {
                format!("NOT EXISTS ({body})")
            } else {
                format!("EXISTS ({body})")
            }
        }
        Predicate::And(preds) => {
            let parts: Vec<String> = preds.iter().map(pred_to_sql).collect();
            format!("({})", parts.join(" AND "))
        }
        Predicate::Or(preds) => {
            let parts: Vec<String> = preds.iter().map(pred_to_sql).collect();
            format!("({})", parts.join(" OR "))
        }
        Predicate::Not(p) => format!("NOT ({})", pred_to_sql(p)),
    }
}

fn sort_key_to_sql(sk: &SortKey) -> String {
    let inner = if sk.key == "id" {
        "b.id".to_string()
    } else {
        json_extract(&Alias::Outer, &sk.key)
    };
    match sk.dir {
        SortDir::Asc => inner,
        SortDir::Desc => format!("{inner} DESC"),
    }
}

impl QueryAst {
    /// Compile this AST to the canonical `holon_sql` form. Whitespace is
    /// significant for round-trip *equality* tests but the structure is
    /// what matters at runtime — `mcp__holon-direct__compile_query`
    /// accepts either form. The string here is structured to match the
    /// canonical example in the plan verbatim (modulo whitespace).
    pub fn compile_to_sql(&self) -> String {
        assert_eq!(
            self.entity, "block",
            "compile_to_sql: only `block` entity supported today"
        );
        let mut sql = String::from("SELECT b.*\nFROM block b");

        if let Some(filter) = &self.filter {
            // The top-level And renders without outer parens to match the
            // canonical form exactly (avoids a bracketed wrapper).
            let body = match filter {
                Predicate::And(preds) => {
                    let parts: Vec<String> = preds.iter().map(pred_to_sql).collect();
                    parts.join("\n  AND ")
                }
                _ => pred_to_sql(filter),
            };
            sql.push_str("\nWHERE ");
            sql.push_str(&body);
        }

        if !self.sort.is_empty() {
            let parts: Vec<String> = self.sort.iter().map(sort_key_to_sql).collect();
            sql.push_str("\nORDER BY\n  ");
            sql.push_str(&parts.join(",\n  "));
        }

        if let Some(n) = self.limit {
            sql.push_str(&format!("\nLIMIT {n}"));
        }
        sql
    }
}

// ─── In-memory evaluation ───────────────────────────────────────────────

/// Storage shim — the reference state's HashMap of blocks plus the edge
/// relations it tracks. The PBT reference state already maintains
/// `properties.tags` as a CSV (via `Tags::from_csv`) and stores
/// `blocked_by` as an array property, so we read both back here.
///
/// Splitting evaluation from `ReferenceState` directly lets the AST be
/// unit-tested with hand-built block sets (see the `tests` module).
pub struct EvalContext<'a> {
    pub blocks: &'a HashMap<EntityUri, Block>,
}

impl<'a> EvalContext<'a> {
    pub fn new(blocks: &'a HashMap<EntityUri, Block>) -> Self {
        Self { blocks }
    }

    /// Tags for `block`. Reads `properties.tags` and parses as a
    /// comma-separated value (mirrors `Tags::from_csv`). Returns the
    /// tags as plain strings.
    fn tags_of(&self, block: &Block) -> Vec<String> {
        match block.properties.get("tags") {
            Some(Value::String(s)) => s
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_string().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// IDs blocking `block`. Reads `properties.blocked_by` as either an
    /// array of strings or a comma-separated string.
    fn blocker_ids_of(&self, block: &Block) -> Vec<EntityUri> {
        match block.properties.get("blocked_by") {
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_string().map(EntityUri::from_raw))
                .collect(),
            Some(Value::String(s)) => s
                .split(',')
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .map(EntityUri::from_raw)
                .collect(),
            _ => Vec::new(),
        }
    }

    fn get_prop_for(
        &self,
        alias: &Alias,
        outer: &Block,
        target: Option<&Block>,
        key: &str,
    ) -> Option<Value> {
        let target_block = match alias {
            Alias::Outer => outer,
            Alias::EdgeTarget => {
                target.expect("EdgeTarget alias used outside an EdgeExists predicate context")
            }
        };
        if key == "id" {
            return Some(Value::String(target_block.id.to_string()));
        }
        target_block.properties.get(key).cloned()
    }

    fn predicate_matches(&self, pred: &Predicate, outer: &Block, target: Option<&Block>) -> bool {
        match pred {
            Predicate::PropEq { alias, key, value } => self
                .get_prop_for(alias, outer, target, key)
                .as_ref()
                .is_some_and(|v| v == value),
            Predicate::PropNe { alias, key, value } => {
                // SQL `COALESCE(x, '') <> v` semantics: NULL → '' which only
                // equals an empty string, otherwise compare directly.
                let lhs = self
                    .get_prop_for(alias, outer, target, key)
                    .unwrap_or(Value::String(String::new()));
                &lhs != value
            }
            Predicate::Membership { negated, edge } => match edge {
                EdgeRef::Tag(tag) => {
                    let has = self.tags_of(outer).iter().any(|t| t == tag);
                    if *negated { !has } else { has }
                }
                EdgeRef::BlockedBy => {
                    panic!("Membership only valid for Tag; use EdgeExists for BlockedBy")
                }
            },
            Predicate::EdgeExists {
                negated,
                edge,
                inner,
            } => {
                let any_match = match edge {
                    EdgeRef::BlockedBy => {
                        let blockers = self.blocker_ids_of(outer);
                        blockers.iter().any(|bid| {
                            let Some(b) = self.blocks.get(bid) else {
                                return false;
                            };
                            match inner {
                                Some(p) => self.predicate_matches(p, outer, Some(b)),
                                None => true,
                            }
                        })
                    }
                    EdgeRef::Tag(_) => {
                        panic!("EdgeExists is for block-typed edges; use Membership for tags")
                    }
                };
                if *negated { !any_match } else { any_match }
            }
            Predicate::And(preds) => preds
                .iter()
                .all(|p| self.predicate_matches(p, outer, target)),
            Predicate::Or(preds) => preds
                .iter()
                .any(|p| self.predicate_matches(p, outer, target)),
            Predicate::Not(p) => !self.predicate_matches(p, outer, target),
        }
    }
}

/// Evaluate `ast` against the reference block map. Returns matching block
/// IDs in the order the AST's `sort` + `limit` clauses prescribe (or
/// arbitrary order if no sort key is given — callers should treat the
/// result as a multiset in that case).
pub fn evaluate(ast: &QueryAst, blocks: &HashMap<EntityUri, Block>) -> Vec<EntityUri> {
    assert_eq!(
        ast.entity, "block",
        "evaluate: only `block` entity supported"
    );
    let ctx = EvalContext::new(blocks);

    let mut hits: Vec<&Block> = blocks
        .values()
        .filter(|b| match &ast.filter {
            Some(p) => ctx.predicate_matches(p, b, None),
            None => true,
        })
        .collect();

    if !ast.sort.is_empty() {
        hits.sort_by(|a, b| compare_blocks(&ast.sort, a, b));
    }

    let trimmed: Vec<EntityUri> = hits.iter().map(|b| b.id.clone()).collect();
    match ast.limit {
        Some(n) => trimmed.into_iter().take(n).collect(),
        None => trimmed,
    }
}

fn compare_blocks(sort: &[SortKey], a: &Block, b: &Block) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    for sk in sort {
        let av = if sk.key == "id" {
            Some(Value::String(a.id.to_string()))
        } else {
            a.properties.get(&sk.key).cloned()
        };
        let bv = if sk.key == "id" {
            Some(Value::String(b.id.to_string()))
        } else {
            b.properties.get(&sk.key).cloned()
        };
        let ord = compare_values(av.as_ref(), bv.as_ref());
        let ord = match sk.dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    // SQLite NULL sort: NULL is smallest in ASC by default.
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(Value::Null), Some(Value::Null)) => Ordering::Equal,
        (Some(Value::Null), _) => Ordering::Less,
        (_, Some(Value::Null)) => Ordering::Greater,
        (Some(av), Some(bv)) => match (av, bv) {
            (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
            (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
            (Value::String(x), Value::String(y)) => x.cmp(y),
            // Mixed numeric coercion: int ⇄ float.
            (Value::Integer(x), Value::Float(y)) => {
                (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
            }
            (Value::Float(x), Value::Integer(y)) => {
                x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
            }
            (Value::Boolean(x), Value::Boolean(y)) => x.cmp(y),
            // Fallback: stringify and compare lexically. Same shape as
            // SQLite's affinity-mismatch ordering.
            (x, y) => format!("{x:?}").cmp(&format!("{y:?}")),
        },
    }
}

// ─── Canonical now-query helper ─────────────────────────────────────────

/// Build the canonical `now-query` AST exactly as specified in the Phase 0
/// plan section (matches the SQL form documented in
/// `please-read-users-martin-workspaces-pkm-fuzzy-hopper.md`).
pub fn now_query_ast() -> QueryAst {
    QueryAst::from_block()
        .with_filter(Predicate::And(vec![
            Predicate::PropEq {
                alias: Alias::Outer,
                key: "task_state".to_string(),
                value: Value::String("TODO".to_string()),
            },
            Predicate::PropEq {
                alias: Alias::Outer,
                key: "gate".to_string(),
                value: Value::String("G1".to_string()),
            },
            Predicate::EdgeExists {
                negated: true,
                edge: EdgeRef::BlockedBy,
                inner: Some(Box::new(Predicate::PropNe {
                    alias: Alias::EdgeTarget,
                    key: "task_state".to_string(),
                    value: Value::String("DONE".to_string()),
                })),
            },
            Predicate::Or(vec![
                Predicate::Membership {
                    negated: false,
                    edge: EdgeRef::Tag("agent".to_string()),
                },
                Predicate::Membership {
                    negated: true,
                    edge: EdgeRef::Tag("human-only".to_string()),
                },
            ]),
        ]))
        .with_sort(vec![
            SortKey {
                key: "priority".to_string(),
                dir: SortDir::Asc,
            },
            SortKey {
                key: "effort".to_string(),
                dir: SortDir::Asc,
            },
            SortKey {
                key: "id".to_string(),
                dir: SortDir::Asc,
            },
        ])
        .with_limit(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm_ws(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn now_query_compiles_to_canonical_sql() {
        let ast = now_query_ast();
        let actual = ast.compile_to_sql();
        let expected = "SELECT b.*
FROM block b
WHERE json_extract(b.properties, '$.task_state') = 'TODO'
  AND json_extract(b.properties, '$.gate') = 'G1'
  AND NOT EXISTS (
    SELECT 1 FROM task_blockers tb
    JOIN block bl ON bl.id = tb.blocker_id
    WHERE tb.blocked_id = b.id
      AND COALESCE(json_extract(bl.properties, '$.task_state'), '') <> 'DONE'
  )
  AND (
    EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'agent')
    OR NOT EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'human-only')
  )
ORDER BY
  json_extract(b.properties, '$.priority'),
  json_extract(b.properties, '$.effort'),
  b.id
LIMIT 10";
        assert_eq!(
            norm_ws(&actual),
            norm_ws(expected),
            "compiled SQL differs from canonical form\n--- actual ---\n{actual}\n--- expected ---\n{expected}"
        );
    }

    fn make_block(id: &str, props: &[(&str, Value)]) -> (EntityUri, Block) {
        let uri = EntityUri::block(id);
        let mut b = Block::new_text(uri.clone(), EntityUri::no_parent(), "");
        for (k, v) in props {
            b.properties.insert(k.to_string(), v.clone());
        }
        (uri, b)
    }

    #[test]
    fn evaluate_now_query_filters_unblocked_todos() {
        // Hand-built reference state covering every branch of the canonical
        // now-query: matches, blocked-by-active, blocked-by-done (still in),
        // wrong gate, wrong tag.
        let mut blocks: HashMap<EntityUri, Block> = HashMap::new();

        // ID 'a' — passes (TODO + G1 + no blockers + tagged agent).
        blocks.extend(std::iter::once(make_block(
            "a",
            &[
                ("task_state", Value::String("TODO".into())),
                ("gate", Value::String("G1".into())),
                ("priority", Value::Integer(1)),
                ("effort", Value::Integer(1)),
                ("tags", Value::String("agent".into())),
            ],
        )));

        // ID 'b' — passes (TODO + G1 + blocker is DONE → unblocked + no tags
        // → "not human-only" branch holds).
        let blocker_done = make_block(
            "blocker_done",
            &[("task_state", Value::String("DONE".into()))],
        );
        blocks.insert(blocker_done.0.clone(), blocker_done.1);
        blocks.extend(std::iter::once(make_block(
            "b",
            &[
                ("task_state", Value::String("TODO".into())),
                ("gate", Value::String("G1".into())),
                ("priority", Value::Integer(2)),
                ("effort", Value::Integer(0)),
                ("blocked_by", Value::String("block:blocker_done".into())),
            ],
        )));

        // ID 'c' — FAILS: blocked by an active TODO.
        let blocker_active = make_block(
            "blocker_active",
            &[("task_state", Value::String("TODO".into()))],
        );
        blocks.insert(blocker_active.0.clone(), blocker_active.1);
        blocks.extend(std::iter::once(make_block(
            "c",
            &[
                ("task_state", Value::String("TODO".into())),
                ("gate", Value::String("G1".into())),
                ("priority", Value::Integer(0)),
                ("effort", Value::Integer(0)),
                ("blocked_by", Value::String("block:blocker_active".into())),
            ],
        )));

        // ID 'd' — FAILS: tagged human-only AND not tagged agent.
        blocks.extend(std::iter::once(make_block(
            "d",
            &[
                ("task_state", Value::String("TODO".into())),
                ("gate", Value::String("G1".into())),
                ("priority", Value::Integer(0)),
                ("effort", Value::Integer(0)),
                ("tags", Value::String("human-only".into())),
            ],
        )));

        // ID 'e' — FAILS: wrong task_state (DOING).
        blocks.extend(std::iter::once(make_block(
            "e",
            &[
                ("task_state", Value::String("DOING".into())),
                ("gate", Value::String("G1".into())),
            ],
        )));

        let ast = now_query_ast();
        let result = evaluate(&ast, &blocks);

        // Expect ids 'a' and 'b' (sorted by priority asc, effort asc, id asc).
        let expected: Vec<EntityUri> = vec![EntityUri::block("a"), EntityUri::block("b")];
        assert_eq!(
            result,
            expected,
            "evaluate did not isolate the unblocked TODO set:\n  expected = {expected:?}\n  actual   = {result:?}\n  blocks   = {:?}",
            blocks.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn evaluate_respects_limit() {
        let mut blocks: HashMap<EntityUri, Block> = HashMap::new();
        for i in 0..5 {
            let id = format!("k{i}");
            blocks.extend(std::iter::once(make_block(
                &id,
                &[
                    ("task_state", Value::String("TODO".into())),
                    ("gate", Value::String("G1".into())),
                    ("priority", Value::Integer(i)),
                    ("tags", Value::String("agent".into())),
                ],
            )));
        }
        let ast = now_query_ast();
        let result = evaluate(&ast, &blocks);
        assert!(result.len() <= 10);
        assert_eq!(result.len(), 5);
        // priority-asc → ids in order k0..k4.
        let expected: Vec<EntityUri> = (0..5).map(|i| EntityUri::block(&format!("k{i}"))).collect();
        assert_eq!(result, expected);
    }
}
