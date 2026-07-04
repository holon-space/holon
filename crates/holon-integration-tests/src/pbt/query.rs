//! Language-neutral query representation for PBT testing.
//!
//! @pbt kind oracle
//! @pbt gen `QuerySource` has 5 variants but the file generator only mints 3
//!   (AllBlocks, DirectChildren, DescendantsOfAny); FocusRootDescendants and
//!   PageBlocks are seeded by the default layout / start_app only, never by a
//!   user-authored index.org override — so a layout-override bug specific to
//!   the focus-root or page-blocks traversal is not reachable via WriteOrgFile
//! @pbt gen watched-query generator (`generate_test_query`) is fixed: always
//!   AllBlocks + the same 6 columns + 0..=2 preds drawn from a 4-element
//!   `generate_predicate` set (Ne/Eq×2/IsNotNull) — no Lt/Gt/Contains, no
//!   custom-property predicates, no negation; the predicate compiler's untested
//!   surface is the AST in query_ast.rs, not this one
//!
//! `TestQuery` compiles to PRQL, SQL, or GQL and evaluates against the
//! reference model. Uses `holon_api::Predicate` directly — no separate
//! TestPredicate type.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::str::FromStr;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::Region;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::predicate::Predicate;

/// Backward-compat alias — old code that references `TestPredicate` still
/// compiles.
pub type TestPredicate = Predicate;

/// A watched query specification: query + language.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WatchSpec {
    pub query: TestQuery,
    pub language: QueryLanguage,
}

/// Which table a TestQuery targets.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum QueryTable {
    Blocks,
}

/// The traversal/source form a query reads from — the dimension that
/// distinguishes a flat watched query (`AllBlocks`) from the LAYOUT queries
/// that drive what the main panel renders.
///
/// Each variant is faithful to a real backend query form: the same PRQL stdlib
/// virtual table (`children`/`descendants`), GQL `CHILD_OF*` traversal, or
/// `focus_root`-bound navigation-aware match. `evaluate`/`base_block_ids`
/// reproduce each form against the reference block map so the reference's
/// rendered set agrees with what the SUT's compiled query returns.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QuerySource {
    /// Every block. PRQL `from block`, SQL `SELECT … FROM block`, GQL
    /// `MATCH (n:block) RETURN n`. The watched-query default.
    AllBlocks,
    /// Direct non-source children of `context` — the PRQL stdlib `children`
    /// virtual table (`parent_id == $context_id | content_type != 'source'`).
    /// Navigation-blind: bound to the layout block that owns the query.
    DirectChildren { context: EntityUri },
    /// Transitive descendants (depth in `min_depth..=max_depth`) of ANY block —
    /// GQL `MATCH (root:block)<-[:CHILD_OF*min..max]-(d:block) RETURN d` with
    /// an unbound root. Navigation-blind. `max_depth == 0` means unbounded.
    DescendantsOfAny { min_depth: u32, max_depth: u32 },
    /// Navigation-aware: descendants (including self, depth `0..=max_depth`) of
    /// the focus-root(s) for `region`. The default layout's panel form
    /// (`MATCH (fr:focus_root),(root:block)<-[:CHILD_OF*0..max]-(d:block)
    /// WHERE fr.region = R AND root.id = fr.root_id RETURN d`).
    FocusRootDescendants {
        region: String,
        max_depth: u32,
        /// When true, the recursive descent stops at non-root pages (page
        /// identity via `block_tags.tag = 'Page'`). The new holon_sql layout
        /// queries (Phase 3 data half of embedded-page collapse+lazy) set
        /// this; the legacy GQL form and the no-user-index-org default do
        /// not.
        stop_at_pages: bool,
    },
    /// The production seeded left-sidebar watch from
    /// `assets/default/index.org`: every page block except the
    /// `__default__` seed page. SQL-only — `SELECT b.* FROM block b JOIN
    /// block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' AND b.id
    /// != 'block:__default__'`. No PRQL/GQL surface (the sidebar is
    /// authored in holon_sql).
    PageBlocks,
}

impl Default for QuerySource {
    fn default() -> Self {
        QuerySource::AllBlocks
    }
}

impl QuerySource {
    /// Recover the `QuerySource` of a LAYOUT query from its source string —
    /// the inverse of [`TestQuery::compile_layout_for`]. The reference uses
    /// this to compute the rendered set of a user `index.org` whose main-panel
    /// query it parsed from disk. `layout_block` is the block that owns the
    /// query (the navigation-blind context for `children`/SQL forms). The
    /// recognised grammar is exactly what `compile_layout_for` emits.
    pub fn recognize(query: &str, lang: QueryLanguage, layout_block: &EntityUri) -> QuerySource {
        let q = query.trim();
        match lang {
            QueryLanguage::HolonPrql => {
                if q.starts_with("from children") {
                    QuerySource::DirectChildren {
                        context: layout_block.clone(),
                    }
                } else if q.starts_with("from descendants") {
                    QuerySource::DescendantsOfAny {
                        min_depth: 1,
                        max_depth: 0,
                    }
                } else if q.starts_with("from focused_children") {
                    QuerySource::FocusRootDescendants {
                        region: "main".to_string(),
                        max_depth: 20,
                        stop_at_pages: false,
                    }
                } else {
                    QuerySource::AllBlocks
                }
            }
            QueryLanguage::HolonGql => {
                if q.contains("focus_root") {
                    QuerySource::FocusRootDescendants {
                        region: gql_focus_region(q),
                        max_depth: 20,
                        stop_at_pages: false,
                    }
                } else if let Some((min_depth, max_depth)) = gql_childof_star_bounds(q) {
                    QuerySource::DescendantsOfAny {
                        min_depth,
                        max_depth,
                    }
                } else if q.contains("<-[:CHILD_OF]-") {
                    QuerySource::DirectChildren {
                        context: layout_block.clone(),
                    }
                } else {
                    QuerySource::AllBlocks
                }
            }
            QueryLanguage::HolonSql => {
                if q.contains("focus_roots") && q.contains("focus_descendants") {
                    // Canonical prod keys (`Region::as_str()`): a focus SQL
                    // filters `navigation_history.region`, whose values are
                    // exactly what `focus_pin` writes. Default to `Main` when no
                    // region predicate is present.
                    let region = if q.contains("region = 'right_sidebar'") {
                        Region::RightSidebar
                    } else if q.contains("region = 'left_sidebar'") {
                        Region::LeftSidebar
                    } else {
                        Region::Main
                    };
                    QuerySource::FocusRootDescendants {
                        region: region.as_str().to_string(),
                        max_depth: 20,
                        stop_at_pages: true,
                    }
                } else if q.contains("parent_id") && q.contains("content_type") {
                    QuerySource::DirectChildren {
                        context: layout_block.clone(),
                    }
                } else {
                    QuerySource::AllBlocks
                }
            }
        }
    }
}

/// Parse the region literal out of a GQL `WHERE fr.region = '<region>'` clause
/// into the canonical [`Region`] key prod uses. Parse-don't-validate at the
/// boundary: the returned string is `Region::as_str()` (the exact value
/// `focus_pin` writes to `navigation_history.region` and the focus matview
/// keys by), NOT whatever literal the seed happens to carry. A focus-root
/// query with no `fr.region` clause defaults to `Region::Main`; an UNKNOWN
/// region literal (e.g. a stale `'right'` instead of `'right_sidebar'`) is a
/// loud parse error here — never a silently-empty filter that mirrors a broken
/// seed and hides the divergence from prod.
fn gql_focus_region(gql: &str) -> String {
    let literal = gql
        .split_once("fr.region")
        .and_then(|(_, rest)| rest.split_once('\''))
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(region, _)| region);
    match literal {
        None => Region::Main.as_str().to_string(),
        Some(lit) => Region::from_str(lit)
            .unwrap_or_else(|e| {
                panic!(
                    "focus-root GQL carries an unknown region literal {lit:?} — the reference \
                     interpreter parses region literals into the prod Region enum so a seed that \
                     drifts from Region::as_str() fails loud instead of silently rendering an \
                     empty region: {e}"
                )
            })
            .as_str()
            .to_string(),
    }
}

/// Parse `a..b` out of a GQL `CHILD_OF*a..b` clause.
fn gql_childof_star_bounds(gql: &str) -> Option<(u32, u32)> {
    let after = gql.split_once("CHILD_OF*")?.1;
    let (min_s, rest) = after.split_once("..")?;
    let max_s: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let min = min_s
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    let max = max_s.parse().ok()?;
    Some((min, max))
}

/// A language-neutral query that can compile to PRQL, SQL, or GQL and also
/// evaluate against the reference model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestQuery {
    pub table: QueryTable,
    pub columns: Vec<String>,
    pub predicates: Vec<Predicate>,
    /// The traversal/source form. Defaults to `AllBlocks` so existing
    /// serialized regression seeds (which predate this field) still load.
    #[serde(default)]
    pub source: QuerySource,
}

/// Format a Value for embedding in a SQL string.
pub fn value_to_sql_literal(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{s}'"),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Null => "NULL".to_string(),
        other => format!("'{:?}'", other),
    }
}

/// Format a Value for PRQL (uses double-quotes for strings).
pub fn value_to_prql_literal(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Null => "null".to_string(),
        other => format!("\"{:?}\"", other),
    }
}

fn pred_to_prql(pred: &Predicate) -> String {
    match pred {
        Predicate::Eq { field, value } => {
            format!("| filter {field} == {} ", value_to_prql_literal(value))
        }
        Predicate::Ne { field, value } if value.is_null() => {
            format!("| filter {field} != null ")
        }
        Predicate::Ne { field, value } => {
            format!("| filter {field} != {} ", value_to_prql_literal(value))
        }
        Predicate::IsNotNull(field) => format!("| filter {field} != null "),
        _ => String::new(),
    }
}

fn pred_to_sql_where(pred: &Predicate) -> String {
    match pred {
        Predicate::Eq { field, value } => {
            format!("{field} = {}", value_to_sql_literal(value))
        }
        Predicate::Ne { field, value } if value.is_null() => {
            format!("{field} IS NOT NULL")
        }
        Predicate::Ne { field, value } => {
            format!("{field} != {}", value_to_sql_literal(value))
        }
        Predicate::IsNotNull(field) => format!("{field} IS NOT NULL"),
        _ => "1=1".to_string(),
    }
}

fn pred_to_gql_where(pred: &Predicate) -> String {
    match pred {
        Predicate::Eq { field, value } => {
            format!("n.{field} = {}", value_to_sql_literal(value))
        }
        Predicate::Ne { field, value } if value.is_null() => {
            format!("n.{field} IS NOT NULL")
        }
        Predicate::Ne { field, value } => {
            format!("n.{field} <> {}", value_to_sql_literal(value))
        }
        Predicate::IsNotNull(field) => format!("n.{field} IS NOT NULL"),
        _ => "1=1".to_string(),
    }
}

impl TestQuery {
    /// Construct a layout query over a non-default [`QuerySource`] (no
    /// predicates, all columns). These are the forms that drive what the main
    /// panel renders.
    pub fn layout(source: QuerySource) -> Self {
        TestQuery {
            table: QueryTable::Blocks,
            columns: all_block_columns(),
            predicates: Vec::new(),
            source,
        }
    }

    pub fn to_prql(&self) -> String {
        // The traversal source determines the FROM (a PRQL stdlib virtual
        // table); predicates append as `| filter …`.
        let from = match &self.source {
            QuerySource::AllBlocks => "from block".to_string(),
            QuerySource::DirectChildren { .. } => "from children".to_string(),
            QuerySource::DescendantsOfAny { .. } => "from descendants".to_string(),
            QuerySource::FocusRootDescendants { .. } => "from focused_children".to_string(),
            QuerySource::PageBlocks => {
                unreachable!("PageBlocks is SQL-only (seeded sidebar watch)")
            }
        };
        let cols = self.columns.join(", ");
        let mut q = format!("{from} | select {{{cols}}} ");
        for pred in &self.predicates {
            q.push_str(&pred_to_prql(pred));
        }
        q
    }

    /// The seeded-sidebar watch SQL over `table` (`block` for the matview
    /// read-path, `block_raw` for the lag-classifier truth-path). Reproduces
    /// the production `index.org` left-sidebar query, projecting
    /// `self.columns` qualified with the `b.` alias (both tables carry
    /// `id`, so the JOIN needs disambiguation). Single source for both
    /// [`to_sql`](Self::to_sql) and
    /// [`to_block_raw_sql`](Self::to_block_raw_sql).
    fn page_blocks_sql(&self, table: &str) -> String {
        let cols = self
            .columns
            .iter()
            .map(|c| format!("b.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut wheres = vec![
            "bt.tag = 'Page'".to_string(),
            format!(
                "b.id != {}",
                value_to_sql_literal(&Value::String("block:__default__".to_string()))
            ),
        ];
        wheres.extend(self.predicates.iter().map(pred_to_sql_where));
        format!(
            "SELECT {cols} FROM {table} b JOIN block_tags bt ON bt.block_id = b.id WHERE {}",
            wheres.join(" AND ")
        )
    }

    pub fn to_sql(&self) -> String {
        if let QuerySource::PageBlocks = &self.source {
            return self.page_blocks_sql("block");
        }
        let cols = self.columns.join(", ");
        let mut wheres: Vec<String> = Vec::new();
        let from = match &self.source {
            QuerySource::AllBlocks => "block".to_string(),
            QuerySource::DirectChildren { context } => {
                wheres.push(format!(
                    "parent_id = {} AND content_type != 'source'",
                    value_to_sql_literal(&Value::String(context.as_str().to_string()))
                ));
                "block".to_string()
            }
            QuerySource::DescendantsOfAny { .. } | QuerySource::FocusRootDescendants { .. } => {
                // These transitive/navigation-aware forms have no flat-SQL
                // surface in the PBT; only PRQL/GQL emit them. `to_sql` is the
                // watched-query path (always AllBlocks/DirectChildren).
                "block".to_string()
            }
            QuerySource::PageBlocks => unreachable!("handled by early return above"),
        };
        let mut q = format!("SELECT {cols} FROM {from}");
        wheres.extend(self.predicates.iter().map(pred_to_sql_where));
        if !wheres.is_empty() {
            q.push_str(" WHERE ");
            q.push_str(&wheres.join(" AND "));
        }
        q
    }

    /// SQL truth-check variant: reads from `block_raw` (the write-side base
    /// table) instead of the `block` matview. Used by
    /// inv-watch-rows-match-ref's lag classifier to distinguish a CDC
    /// delivery race (matview state correct, stream stale) from a real
    /// write-pipeline divergence.
    ///
    /// `block` is a matview hydrated from `block_raw` + junction tables for
    /// `tags`/`requires`. For predicates that don't touch the hydrated
    /// columns (and the watch in question filters on `content_type`), the
    /// IDs returned by `block_raw` and `block` are identical, so this is a
    /// safe truth source.
    pub fn to_block_raw_sql(&self) -> String {
        if let QuerySource::PageBlocks = &self.source {
            // block_tags is a base table in both wirings, so the seeded-sidebar
            // JOIN reads straight from `block_raw`.
            return self.page_blocks_sql("block_raw");
        }
        let cols = self.columns.join(", ");
        let mut q = format!("SELECT {cols} FROM block_raw");
        let wheres: Vec<String> = self.predicates.iter().map(pred_to_sql_where).collect();
        if !wheres.is_empty() {
            q.push_str(" WHERE ");
            q.push_str(&wheres.join(" AND "));
        }
        q
    }

    pub fn to_gql(&self) -> String {
        // The bound variable that carries the returned node differs by form:
        // `n` for a flat match, `d` for a traversal target.
        let (pattern, bind) = match &self.source {
            QuerySource::AllBlocks => ("MATCH (n:block)".to_string(), "n"),
            QuerySource::DirectChildren { context } => (
                format!(
                    "MATCH (root:block)<-[:CHILD_OF]-(d:block) WHERE root.id = {} AND \
                     d.content_type != 'source'",
                    value_to_sql_literal(&Value::String(context.as_str().to_string()))
                ),
                "d",
            ),
            QuerySource::DescendantsOfAny {
                min_depth,
                max_depth,
            } => (
                format!("MATCH (root:block)<-[:CHILD_OF*{min_depth}..{max_depth}]-(d:block)"),
                "d",
            ),
            QuerySource::FocusRootDescendants {
                region, max_depth, ..
            } => (
                format!(
                    "MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..{max_depth}]-(d:block) \
                     WHERE fr.region = '{region}' AND root.id = fr.root_id"
                ),
                "d",
            ),
            QuerySource::PageBlocks => {
                unreachable!("PageBlocks is SQL-only (seeded sidebar watch)")
            }
        };
        let returns = self
            .columns
            .iter()
            .map(|c| format!("{bind}.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let preds: Vec<String> = self
            .predicates
            .iter()
            .map(|p| pred_to_gql_where(p).replace("n.", &format!("{bind}.")))
            .collect();
        // Fold predicates into the pattern's WHERE (adding one if absent).
        let mut q = pattern;
        if !preds.is_empty() {
            if q.contains(" WHERE ") {
                q.push_str(" AND ");
                q.push_str(&preds.join(" AND "));
            } else {
                q.push_str(" WHERE ");
                q.push_str(&preds.join(" AND "));
            }
        }
        format!("{q} RETURN {returns}")
    }

    /// Compile to the given language.
    pub fn compile_for(&self, lang: QueryLanguage) -> (String, QueryLanguage) {
        match lang {
            QueryLanguage::HolonPrql => (self.to_prql(), QueryLanguage::HolonPrql),
            QueryLanguage::HolonSql => (self.to_sql(), QueryLanguage::HolonSql),
            QueryLanguage::HolonGql => (self.to_gql(), QueryLanguage::HolonGql),
        }
    }

    /// Compile a LAYOUT query (`index.org` main-panel source) to the given
    /// language. Unlike [`compile_for`](Self::compile_for), this returns
    /// WHOLE rows (`from children`, `RETURN d`, `SELECT *`) rather than a fixed
    /// column projection — render templates read columns beyond the canonical
    /// six (`task_state`, `properties`, …). The row-selection (FROM/WHERE/
    /// traversal) is identical to `compile_for`, so the rendered SET is the
    /// same (pinned by the query-equivalence PBT). The reference recovers
    /// the `QuerySource` from the emitted string via
    /// [`QuerySource::recognize`].
    pub fn compile_layout_for(&self, lang: QueryLanguage) -> (String, QueryLanguage) {
        let q = match (&self.source, lang) {
            (QuerySource::AllBlocks, QueryLanguage::HolonPrql) => "from block".to_string(),
            (QuerySource::AllBlocks, QueryLanguage::HolonSql) => "SELECT * FROM block".to_string(),
            (QuerySource::AllBlocks, QueryLanguage::HolonGql) => {
                "MATCH (n:block) RETURN n".to_string()
            }
            (QuerySource::DirectChildren { .. }, QueryLanguage::HolonPrql) => {
                "from children".to_string()
            }
            (QuerySource::DirectChildren { context }, QueryLanguage::HolonSql) => format!(
                "SELECT * FROM block WHERE parent_id = {} AND content_type != 'source'",
                value_to_sql_literal(&Value::String(context.as_str().to_string()))
            ),
            (QuerySource::DirectChildren { context }, QueryLanguage::HolonGql) => format!(
                "MATCH (root:block)<-[:CHILD_OF]-(d:block) WHERE root.id = {} AND d.content_type \
                 != 'source' RETURN d",
                value_to_sql_literal(&Value::String(context.as_str().to_string()))
            ),
            (
                QuerySource::DescendantsOfAny {
                    min_depth,
                    max_depth,
                },
                _,
            ) => format!(
                "MATCH (root:block)<-[:CHILD_OF*{min_depth}..{max_depth}]-(d:block) RETURN d"
            ),
            (
                QuerySource::FocusRootDescendants {
                    region, max_depth, ..
                },
                _,
            ) => format!(
                "MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..{max_depth}]-(d:block) WHERE \
                 fr.region = '{region}' AND root.id = fr.root_id RETURN d"
            ),
            (QuerySource::PageBlocks, _) => {
                unreachable!("PageBlocks is a watched query, not a layout query")
            }
        };
        // DescendantsOfAny / FocusRootDescendants only have a GQL surface.
        let effective = match &self.source {
            QuerySource::DescendantsOfAny { .. } | QuerySource::FocusRootDescendants { .. } => {
                QueryLanguage::HolonGql
            }
            _ => lang,
        };
        (q, effective)
    }

    /// Evaluate this query against the reference model's blocks (watched-query
    /// path: projects columns for each matching row). Equivalent to
    /// [`rendered_block_ids`](Self::rendered_block_ids) projected to rows.
    pub fn evaluate(&self, blocks: &BTreeMap<EntityUri, Block>) -> Vec<HashMap<String, Value>> {
        self.evaluate_with_focus(blocks, &BTreeMap::new())
    }

    /// Like [`evaluate`](Self::evaluate) but supplies the per-region focus
    /// roots needed by the navigation-aware
    /// [`QuerySource::FocusRootDescendants`] form. Watched queries (always
    /// `AllBlocks`) ignore it.
    pub fn evaluate_with_focus(
        &self,
        blocks: &BTreeMap<EntityUri, Block>,
        focus_roots: &BTreeMap<String, BTreeSet<EntityUri>>,
    ) -> Vec<HashMap<String, Value>> {
        self.rendered_block_ids(blocks, focus_roots)
            .into_iter()
            .filter_map(|id| blocks.get(&id))
            .map(|b| self.project_columns(b))
            .collect()
    }

    /// The set of block ids this query's [`QuerySource`] selects (before
    /// predicates), against the reference block map. `focus_roots` maps a
    /// region name to its focus-root block ids (only consumed by
    /// [`QuerySource::FocusRootDescendants`]).
    pub fn base_block_ids(
        &self,
        blocks: &BTreeMap<EntityUri, Block>,
        focus_roots: &BTreeMap<String, BTreeSet<EntityUri>>,
    ) -> Vec<EntityUri> {
        match &self.source {
            QuerySource::AllBlocks => blocks.keys().cloned().collect(),
            QuerySource::DirectChildren { context } => blocks
                .values()
                .filter(|b| &b.parent_id == context && !b.is_source_block())
                .map(|b| b.id.clone())
                .collect(),
            QuerySource::DescendantsOfAny { min_depth, .. } => blocks
                .values()
                // With an UNBOUND root, `CHILD_OF*min..max` matches `d` iff some
                // ancestor sits at a distance in [min,max]. The nearest ancestor
                // is at distance 1, so for any `min ≤ max` the binding condition
                // reduces to `depth(d) ≥ min` (max is non-binding — a shorter
                // path always exists). Unlike the PRQL `children`/`descendants`
                // stdlib tables, the raw GQL `CHILD_OF*` traversal does NOT filter
                // `content_type = 'source'`, so source blocks are included.
                // Both facts are pinned by the query-equivalence PBT.
                .filter(|b| depth_from_some_root(blocks, &b.id) >= *min_depth)
                .map(|b| b.id.clone())
                .collect(),
            QuerySource::FocusRootDescendants {
                region,
                max_depth,
                stop_at_pages,
            } => {
                let roots = focus_roots.get(region).cloned().unwrap_or_default();
                if *stop_at_pages {
                    blocks
                        .values()
                        .filter(|b| {
                            descendant_within_stopping_at_pages(blocks, &b.id, &roots, *max_depth)
                        })
                        .map(|b| b.id.clone())
                        .collect()
                } else {
                    blocks
                        .values()
                        .filter(|b| descendant_within(blocks, &b.id, &roots, *max_depth))
                        .map(|b| b.id.clone())
                        .collect()
                }
            }
            QuerySource::PageBlocks => {
                let default_page = EntityUri::block("__default__");
                blocks
                    .values()
                    .filter(|b| b.is_page() && b.id != default_page)
                    .map(|b| b.id.clone())
                    .collect()
            }
        }
    }

    /// The rendered set: the [`QuerySource`] base intersected with the
    /// predicates. These are the block ids the SUT's compiled layout query
    /// would surface — the candidate universe for block-interaction
    /// transitions.
    pub fn rendered_block_ids(
        &self,
        blocks: &BTreeMap<EntityUri, Block>,
        focus_roots: &BTreeMap<String, BTreeSet<EntityUri>>,
    ) -> Vec<EntityUri> {
        self.base_block_ids(blocks, focus_roots)
            .into_iter()
            .filter(|id| {
                blocks
                    .get(id)
                    .is_some_and(|b| self.predicates.iter().all(|p| predicate_matches(p, b)))
            })
            .collect()
    }

    fn project_columns(&self, block: &Block) -> HashMap<String, Value> {
        let mut row = HashMap::new();
        for col in &self.columns {
            let val = match col.as_str() {
                "id" => Value::String(block.id.to_string()),
                "content" => Value::String(block.content.clone()),
                "content_type" => block.content_type.into(),
                "parent_id" => block.parent_id.clone().into(),
                "source_language" => match &block.source_language {
                    Some(sl) => Value::String(sl.to_string()),
                    None => Value::Null,
                },
                "source_name" => match &block.source_name {
                    Some(sn) => Value::String(sn.clone()),
                    None => Value::Null,
                },
                _ => block.properties.get(col).cloned().unwrap_or(Value::Null),
            };
            row.insert(col.clone(), val);
        }
        row
    }
}

/// Evaluate a Predicate against a Block (for PBT reference model).
pub fn predicate_matches(pred: &Predicate, block: &Block) -> bool {
    let get_field = |field: &str| -> Option<Value> {
        match field {
            "id" => Some(Value::String(block.id.to_string())),
            "content" => Some(Value::String(block.content.clone())),
            "content_type" => Some(block.content_type.into()),
            "parent_id" => Some(block.parent_id.clone().into()),
            "source_language" => block
                .source_language
                .as_ref()
                .map(|sl| Value::String(sl.to_string())),
            "source_name" => block
                .source_name
                .as_ref()
                .map(|sn| Value::String(sn.clone())),
            other => block.properties.get(other).cloned(),
        }
    };

    let compare_numeric = |field: &str, value: &Value, cmp: fn(f64, f64) -> bool| -> bool {
        let Some(lhs) = get_field(field) else {
            return false;
        };
        let (Some(l), Some(r)) = (lhs.as_f64(), value.as_f64()) else {
            return false;
        };
        cmp(l, r)
    };

    match pred {
        Predicate::Eq { field, value } => get_field(field).as_ref() == Some(value),
        Predicate::Ne { field, value } => {
            match (get_field(field), value) {
                (None, Value::Null) => false, // NULL != NULL → false in SQL
                (None, _) => true,            // NULL != non-null → true
                (Some(v), _) => &v != value,
            }
        }
        Predicate::Gt { field, value } => compare_numeric(field, value, |l, r| l > r),
        Predicate::Lt { field, value } => compare_numeric(field, value, |l, r| l < r),
        Predicate::Gte { field, value } => compare_numeric(field, value, |l, r| l >= r),
        Predicate::Lte { field, value } => compare_numeric(field, value, |l, r| l <= r),
        Predicate::IsNotNull(field) => get_field(field).is_some(),
        Predicate::Var(field) => get_field(field).is_some_and(|v| !v.is_null()),
        Predicate::Not(inner) => !predicate_matches(inner, block),
        Predicate::And(preds) => preds.iter().all(|p| predicate_matches(p, block)),
        Predicate::Or(preds) => preds.iter().any(|p| predicate_matches(p, block)),
        Predicate::Always => true,
    }
}

/// The standard column projection for a block layout/watched query
/// (`RETURN *` equivalent). Mirrors the columns `generate_test_query` uses.
pub fn all_block_columns() -> Vec<String> {
    [
        "id",
        "content",
        "content_type",
        "source_language",
        "source_name",
        "parent_id",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Depth of `id` below its top-most ancestor within `blocks`: a block whose
/// parent is absent / `no_parent` / a sentinel root is depth 0; each
/// further hop up adds 1. Walks the parent chain (bounded to 50 hops to match
/// `ReferenceState::is_descendant_of_any`).
fn depth_from_some_root(blocks: &BTreeMap<EntityUri, Block>, id: &EntityUri) -> u32 {
    let mut depth = 0u32;
    let mut current = id.clone();
    for _ in 0..50 {
        let Some(block) = blocks.get(&current) else {
            return depth;
        };
        let parent = &block.parent_id;
        if parent.is_no_parent() || parent.is_sentinel() || !blocks.contains_key(parent) {
            return depth;
        }
        depth += 1;
        current = parent.clone();
    }
    depth
}

/// Whether `id` is within `max_depth` CHILD_OF hops of some block in `roots`
/// (depth 0 = `id` itself is a root). `max_depth == 0` still includes the roots
/// themselves; the default layout uses `0..20`.
fn descendant_within(
    blocks: &BTreeMap<EntityUri, Block>,
    id: &EntityUri,
    roots: &BTreeSet<EntityUri>,
    max_depth: u32,
) -> bool {
    if roots.contains(id) {
        return true;
    }
    let mut current = id.clone();
    for _ in 0..max_depth.min(50) {
        let Some(block) = blocks.get(&current) else {
            return false;
        };
        let parent = block.parent_id.clone();
        if roots.contains(&parent) {
            return true;
        }
        if parent.is_no_parent() || parent.is_sentinel() {
            return false;
        }
        current = parent;
    }
    false
}

/// Like [`descendant_within`] but the traversal stops at any non-root page:
/// a block whose nearest-root ancestor path includes a page that is not itself
/// a root is excluded. This mirrors the Phase 3 holon_sql recursive CTE's
/// `LEFT JOIN block_tags ... WHERE fd._depth = 0 OR bt.block_id IS NULL`
/// guard, which descends into children of the root (depth 0) unconditionally
/// but into children of non-root nodes only if those nodes are not pages.
fn descendant_within_stopping_at_pages(
    blocks: &BTreeMap<EntityUri, Block>,
    id: &EntityUri,
    roots: &BTreeSet<EntityUri>,
    max_depth: u32,
) -> bool {
    if roots.contains(id) {
        return true;
    }
    let mut current = id.clone();
    for _ in 0..max_depth.min(50) {
        let Some(block) = blocks.get(&current) else {
            return false;
        };
        let parent = block.parent_id.clone();
        if roots.contains(&parent) {
            return true;
        }
        // If the current node is a non-root page, the SQL query stops
        // descending here — so `id` (a descendant of this page) is excluded.
        if block.is_page() {
            return false;
        }
        if parent.is_no_parent() || parent.is_sentinel() {
            return false;
        }
        current = parent;
    }
    false
}
