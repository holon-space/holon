//! Turso implementation of the query capability, plus the Turso-private
//! raw-SQL extension trait.
//!
//! The storage-agnostic core trait lives in [`holon_api::query_engine`]
//! (storage de-leak Stage 10); this module implements it for the concrete
//! Turso [`BackendEngine`] and adds [`SqlQueryEngine`] for the callers that
//! legitimately speak SQL: MCP debug tools, integration tests, and
//! holon-internal code. holon-frontend must never see this extension trait.

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::EnrichedChangeStream;
use holon_api::EntityUri;
use holon_api::LinkCandidate;
use holon_api::QueryContext;
use holon_api::QueryLanguage;
use holon_api::Value;
pub use holon_api::query_engine::QueryEngine;

use crate::api::BackendEngine;
use crate::storage::turso::RowChangeStream;

/// Raw-SQL extension of [`QueryEngine`] — Turso-typed (`RowChangeStream`) and
/// SQL-string-typed. Implemented by [`BackendEngine`] only; deliberately NOT
/// part of holon-api so the storage-agnostic layers cannot reach it.
#[async_trait]
pub trait SqlQueryEngine: QueryEngine {
    /// Compile a query in any supported language (PRQL/GQL/SQL) to final SQL.
    fn compile_to_sql(&self, query: &str, language: QueryLanguage) -> Result<String>;

    /// Execute a SQL query, set up CDC streaming, and return a stream whose
    /// first batch is the initial results, followed by CDC deltas.
    async fn query_and_watch(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream>;

    /// Execute a SQL query once and return all rows.
    async fn execute_query(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<holon_api::StorageEntity>>;
}

#[async_trait]
impl QueryEngine for BackendEngine {
    async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String> {
        self.blocks().lookup_block_path(block_id).await
    }

    async fn watch_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
        renderer: holon_api::render_requirements::RenderRequirements,
    ) -> Result<EnrichedChangeStream> {
        let sql = BackendEngine::compile_to_sql(self, query, language)?;
        let raw = BackendEngine::query_and_watch(self, sql, params, context).await?;
        Ok(crate::api::ui_watcher::enrich_stream(
            raw,
            self.profile_resolver().clone(),
            renderer,
        ))
    }

    fn ordering_spec(&self, query: &str, language: QueryLanguage) -> Result<Option<String>> {
        BackendEngine::query_ordering_spec(self, query, language)
    }

    async fn search_link_candidates(&self, filter: &str) -> Result<Vec<LinkCandidate>> {
        use crate::storage::BLOCK_READ_TABLE;
        let m = SearchMatch::new(filter);
        let (bare, qualified) = (m.contained_in("content"), m.contained_in("b.content"));
        // Subquery wrapping required — Turso rejects bare UNION.
        // The two branches are DISJOINT so no entity is listed twice: the
        // content branch excludes Page-tagged blocks (which the page branch
        // owns). A Page-tagged block matching both branches otherwise appeared
        // twice in the `[[` popup — once as a block, once as a page.
        // Page rows surface the first content line (the title) as the label.
        let sql = format!(
            "SELECT * FROM (SELECT id, content AS label FROM {BLOCK_READ_TABLE} WHERE {bare} AND \
             id NOT IN (SELECT block_id FROM block_tags WHERE tag = 'Page') LIMIT 15) UNION ALL \
             SELECT * FROM (SELECT b.id, substr(b.content, 1, instr(b.content || char(10), \
             char(10)) - 1) AS label FROM {BLOCK_READ_TABLE} b WHERE b.id IN (SELECT block_id \
             FROM block_tags WHERE tag = 'Page') AND {qualified} LIMIT 5)"
        );
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        parse_link_candidates(rows)
    }

    async fn quick_open_search(&self, filter: &str) -> Result<holon_api::QuickOpenResults> {
        use crate::storage::BLOCK_READ_TABLE;
        let trimmed = filter.trim();
        if trimmed.is_empty() {
            return Ok(holon_api::QuickOpenResults::default());
        }
        let m = SearchMatch::new(trimmed);
        let (anywhere, prefix) = (m.contained_in("b.content"), m.prefix_of("b.content"));

        // Pages: blocks carrying the 'Page' tag whose content matches. Label is
        // the first content line (the page title). Prefix matches rank first.
        //
        // The Page predicate is an `IN` subquery, never a JOIN against the
        // `block` matview: the joined spelling costs 10.7s on a 2257-block
        // vault against 53ms for this one (measured), which put every keystroke
        // past the newest-response guard and rendered search permanently empty.
        let pages_sql = format!(
            "SELECT b.id AS id, substr(b.content, 1, instr(b.content || char(10), char(10)) - 1) \
             AS label FROM {BLOCK_READ_TABLE} b WHERE b.id IN (SELECT block_id FROM block_tags \
             WHERE tag = 'Page') AND {anywhere} ORDER BY ({prefix}) DESC, length(b.content) ASC \
             LIMIT 20"
        );
        // Content: non-page blocks whose content matches. Label is the matched
        // content (full block content — the modal truncates for display).
        let content_sql = format!(
            "SELECT b.id AS id, b.content AS label FROM {BLOCK_READ_TABLE} b WHERE {anywhere} AND \
             b.id NOT IN (SELECT block_id FROM block_tags WHERE tag = 'Page') ORDER BY ({prefix}) \
             DESC, length(b.content) ASC LIMIT 30"
        );

        let pages = parse_link_candidates(
            BackendEngine::execute_query(self, pages_sql, HashMap::new(), None).await?,
        )?;
        let content = parse_link_candidates(
            BackendEngine::execute_query(self, content_sql, HashMap::new(), None).await?,
        )?;
        Ok(holon_api::QuickOpenResults { pages, content })
    }

    async fn region_view_root(&self, region: holon_api::Region) -> Result<Option<EntityUri>> {
        let sql = format!(
            "SELECT fr.root_id AS root_id FROM focus_roots fr JOIN navigation_cursor nc ON \
             nc.history_id = fr.history_id WHERE fr.region = '{}' AND nc.region = '{}'",
            region.as_str(),
            region.as_str(),
        );
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let raw = row
            .get("root_id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                anyhow::anyhow!("region_view_root: focus_roots.root_id is not a string")
            })?;
        EntityUri::parse(raw).map(Some).map_err(|e| {
            anyhow::anyhow!("region_view_root: root_id {raw:?} is not a block URI: {e}")
        })
    }

    async fn region_open_tabs(&self, region: holon_api::Region) -> Result<holon_api::RegionTabs> {
        // Open `navigation_history` rows, NOT `focus_roots`: that matview drops
        // NULL-block rows, which is exactly what a blank tab is. LEFT JOIN
        // because a blank tab has no block to take a caption from.
        let tabs_sql = format!(
            "SELECT nh.id AS history_id, nh.block_id AS block_id, substr(b.content, 1, \
             instr(b.content || char(10), char(10)) - 1) AS caption FROM navigation_history nh \
             LEFT JOIN block b ON b.id = nh.block_id WHERE nh.region = '{}' AND nh.closed_at IS \
             NULL ORDER BY nh.id",
            region.as_str(),
        );
        let rows = BackendEngine::execute_query(self, tabs_sql, HashMap::new(), None).await?;

        let mut tabs = Vec::with_capacity(rows.len());
        for row in &rows {
            let history_id = row
                .get("history_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| {
                    anyhow::anyhow!("region_open_tabs: navigation_history.id is not an integer")
                })?;
            let block_id = match row.get("block_id").and_then(|v| v.as_string()) {
                Some(raw) => Some(EntityUri::parse(raw).map_err(|e| {
                    anyhow::anyhow!("region_open_tabs: block_id {raw:?} is not a block URI: {e}")
                })?),
                None => None,
            };
            let caption = row
                .get("caption")
                .and_then(|v| v.as_string())
                .map(str::to_string);
            tabs.push(holon_api::OpenTab {
                history_id,
                block_id,
                caption,
            });
        }

        let cursor_sql = format!(
            "SELECT history_id FROM navigation_cursor WHERE region = '{}'",
            region.as_str(),
        );
        let cursor_rows =
            BackendEngine::execute_query(self, cursor_sql, HashMap::new(), None).await?;
        let active_history_id = cursor_rows
            .first()
            .and_then(|row| row.get("history_id"))
            .and_then(|v| v.as_i64());

        Ok(holon_api::RegionTabs {
            tabs,
            active_history_id,
        })
    }

    async fn breadcrumb_trail(&self, block_id: &EntityUri) -> Result<Vec<LinkCandidate>> {
        use crate::storage::BLOCK_READ_TABLE;
        let escaped_id = block_id.to_string().replace('\'', "''");

        // 1. Ancestor path from the matview (`/rootId/childId/.../blockId`).
        let path_sql =
            format!("SELECT path FROM block_with_path WHERE id = '{escaped_id}' LIMIT 1");
        let path_rows = BackendEngine::execute_query(self, path_sql, HashMap::new(), None).await?;
        let path = path_rows
            .first()
            .and_then(|r| r.get("path"))
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                anyhow::anyhow!("breadcrumb: block {block_id} has no path in block_with_path")
            })?
            .to_string();

        // Ordered ancestor ids, root → target.
        let ordered_ids: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if ordered_ids.is_empty() {
            anyhow::bail!("breadcrumb: empty ancestor path {path:?} for {block_id}");
        }

        // 2. Titles for the `Page`-tagged ancestors among those ids.
        let in_list = ordered_ids
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let titles_sql = format!(
            "SELECT b.id AS id, substr(b.content, 1, instr(b.content || char(10), char(10)) - 1) \
             AS label FROM {BLOCK_READ_TABLE} b WHERE b.id IN (SELECT block_id FROM block_tags \
             WHERE tag = 'Page') AND b.id IN ({in_list})"
        );
        let title_rows =
            BackendEngine::execute_query(self, titles_sql, HashMap::new(), None).await?;
        let page_titles: std::collections::HashMap<String, String> = title_rows
            .into_iter()
            .filter_map(|row| {
                let id = row.get("id").and_then(|v| v.as_string())?.to_string();
                let label = row
                    .get("label")
                    .and_then(|v| v.as_string())
                    .unwrap_or("(untitled)")
                    .to_string();
                Some((id, label))
            })
            .collect();

        // 3. Emit page ancestors in path order (root → current).
        let mut trail = Vec::new();
        for raw_id in &ordered_ids {
            if let Some(label) = page_titles.get(raw_id) {
                let id = EntityUri::parse(raw_id).map_err(|e| {
                    anyhow::anyhow!("breadcrumb: path id {raw_id:?} is not a valid EntityUri: {e}")
                })?;
                trail.push(LinkCandidate {
                    id,
                    label: label.clone(),
                });
            }
        }
        if trail.is_empty() {
            anyhow::bail!(
                "breadcrumb: no Page-tagged ancestors resolved for {block_id} (path {path:?})"
            );
        }
        Ok(trail)
    }

    /// One-shot compile + execute (the advice weave's canonical read, ADR
    /// 0022): no matview, no CDC — delegates to the inherent
    /// `execute_query` retry path and re-keys `StorageEntity` (`Arc<str>`
    /// keys) into `DataRow` (`String`).
    async fn execute_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<holon_api::widget_spec::DataRow>> {
        let sql = BackendEngine::compile_to_sql(self, query, language)?;
        let rows = BackendEngine::execute_query(self, sql, params, context).await?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
            .collect())
    }

    async fn block_content_by_id(&self, id: &EntityUri) -> Result<Option<String>> {
        use crate::storage::BLOCK_WRITE_TABLE;
        let escaped = id.to_string().replace('\'', "''");
        let sql = format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{escaped}'");
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        Ok(rows.into_iter().next().and_then(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        }))
    }

    async fn block_task_state_by_id(&self, id: &EntityUri) -> Result<Option<String>> {
        use crate::storage::BLOCK_WRITE_TABLE;
        let escaped = id.to_string().replace('\'', "''");
        let sql = format!(
            "SELECT json_extract(properties, '$.task_state') AS task_state FROM \
             {BLOCK_WRITE_TABLE} WHERE id = '{escaped}'"
        );
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        Ok(rows.into_iter().next().and_then(|r| {
            r.get("task_state")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        }))
    }

    async fn block_editor_source_by_id(
        &self,
        id: &EntityUri,
    ) -> Result<holon_api::query_engine::EditorSource> {
        use crate::storage::BLOCK_WRITE_TABLE;
        let escaped = id.to_string().replace('\'', "''");
        let sql = format!(
            "SELECT content, marks, json_extract(properties, '$.task_state') AS task_state FROM \
             {BLOCK_WRITE_TABLE} WHERE id = '{escaped}'"
        );
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(holon_api::query_engine::EditorSource::default());
        };
        let field = |key: &str| row.get(key).and_then(|v| v.as_string()).map(str::to_string);
        // Parse at the boundary: a `marks` column this editor cannot read is an
        // Err naming the row, never an empty span set — that would seed the
        // surface with markup-free text and look exactly like a block that has
        // no marks.
        let marks = match field("marks").filter(|m| !m.is_empty() && m != "[]") {
            Some(raw) => holon_api::marks_from_json(&raw)
                .map_err(anyhow::Error::from)
                .with_context(|| {
                    format!(
                        "editor-source read for {id}: block row carries unreadable marks {raw:?}"
                    )
                })?,
            None => Vec::new(),
        };
        Ok(holon_api::query_engine::EditorSource {
            content: field("content"),
            marks,
            task_state: field("task_state"),
        })
    }

    async fn block_todo_keywords(
        &self,
        id: &EntityUri,
    ) -> Result<Option<Vec<holon_api::TaskState>>> {
        crate::api::task_vocabulary_source::SqlTaskVocabularySource::new(
            self.db_handle().clone(),
            crate::storage::BLOCK_WRITE_TABLE,
        )
        .declared_keywords(id.as_str())
        .await
    }
}

/// Unicode *simple* lowercase: the lowercase form when that is a single
/// character, else the character unchanged. Only simple folding is available
/// here because a `GLOB` character class holds single characters, so `ß` → `ss`
/// is inexpressible.
fn simple_lower(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// The `simple_lower` counterpart, so a fold is only applied where the two
/// round-trip: `ß` uppercases to `SS` and therefore folds to itself, and so
/// does `ẞ`, which would otherwise fold onto a `ß` the pattern never reaches.
fn simple_upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) if simple_lower(u) == c => u,
        _ => c,
    }
}

/// A user-typed search string parsed into a case-insensitive SQL `GLOB`
/// pattern in which every character the user typed matches only itself.
///
/// `GLOB` rather than `LIKE` because the fold has to live in the pattern:
/// `GLOB` is case-sensitive, so each cased letter carries its own two-element
/// character class and folding costs one flat class per character. Folding the
/// stored side instead — the `LIKE` spelling — nests one `replace()` per cased
/// letter, and that depth overflowed the stack on a Cyrillic or Greek phrase
/// (entry `search-folding-crashes-the-app-on-cyrillic-and-greek`).
struct SearchMatch {
    /// The pattern between the quotes, already escaped for both `GLOB` and the
    /// SQL string literal that carries it.
    body: String,
}

impl SearchMatch {
    fn new(query: &str) -> Self {
        let mut body = String::new();
        for c in query.chars() {
            let lower = simple_lower(c);
            let upper = simple_upper(lower);
            match c {
                _ if upper != lower => body.extend(['[', lower, upper, ']']),
                // `GLOB` has no escape character, so a one-element class is the
                // only way to spell its own metacharacters literally.
                '*' | '?' | '[' => body.extend(['[', c, ']']),
                '\'' => body.push_str("''"),
                _ => body.push(c),
            }
        }
        Self { body }
    }

    /// Predicate: `column` contains the query anywhere.
    fn contained_in(&self, column: &str) -> String {
        format!("{column} GLOB '*{}*'", self.body)
    }

    /// Predicate: `column` starts with the query — the prefix ranker.
    fn prefix_of(&self, column: &str) -> String {
        format!("{column} GLOB '{}*'", self.body)
    }
}

/// Parse `(id, label)` search rows into typed [`LinkCandidate`]s, failing loud
/// on a missing/invalid `id` (parse-don't-validate at the storage boundary).
/// Shared by [`QueryEngine::search_link_candidates`] and
/// [`QueryEngine::quick_open_search`].
fn parse_link_candidates(rows: Vec<holon_api::StorageEntity>) -> Result<Vec<LinkCandidate>> {
    rows.into_iter()
        .map(|row| {
            let raw_id = row
                .get("id")
                .and_then(|v| v.as_string())
                .ok_or_else(|| anyhow::anyhow!("link-search row missing 'id': {row:?}"))?
                .to_string();
            let id = EntityUri::parse(&raw_id).map_err(|e| {
                anyhow::anyhow!("link-search row id {raw_id:?} is not a valid EntityUri: {e}")
            })?;
            let label = row
                .get("label")
                .and_then(|v| v.as_string())
                .unwrap_or("(untitled)")
                .to_string();
            Ok(LinkCandidate { id, label })
        })
        .collect()
}

#[async_trait]
impl SqlQueryEngine for BackendEngine {
    fn compile_to_sql(&self, query: &str, language: QueryLanguage) -> Result<String> {
        BackendEngine::compile_to_sql(self, query, language)
    }

    async fn query_and_watch(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream> {
        BackendEngine::query_and_watch(self, sql, params, context).await
    }

    async fn execute_query(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<holon_api::StorageEntity>> {
        BackendEngine::execute_query(self, sql, params, context).await
    }
}
