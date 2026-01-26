use anyhow::{Context, Result};
use std::collections::HashMap;

use super::backend_engine::{BackendEngine, QueryContext};
use holon_api::{EntityUri, QueryLanguage, RenderExpr, Value, uri_from_row};

use crate::storage::turso::RowChangeStream;

const BLOCK_PATH_LOOKUP_SQL: &str = include_str!("../../sql/queries/block_path_lookup.sql");
const TASK_BLOCKS_FOR_PETRI_SQL: &str = include_str!("../../sql/queries/task_blocks_for_petri.sql");
const BLOCK_WITH_QUERY_SOURCE_SQL: &str =
    include_str!("../../sql/queries/block_with_query_source.sql");

pub use holon_api::ROOT_LAYOUT_BLOCK_ID;

/// Walk a `RenderExpr` and substitute `virtual_parent: Bool(true)` (the DSL
/// sentinel) with `virtual_parent: String(<parent_id>)` so the tree builder's
/// trailing-slot construction sees the resolved id.
///
/// Mirrors `holon_frontend::render_interpreter::resolve_virtual_parent` but
/// lives in the `holon` crate so the live_block path
/// (`collection_render_from_profile`) can use it without violating the crate
/// dependency direction (`holon-frontend → holon → holon-api`). One level
/// deep — the only place `virtual_parent` legitimately appears today.
fn resolve_virtual_parent(expr: RenderExpr, parent_id: &str) -> RenderExpr {
    use holon_api::render_types::Arg;
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            let mut substituted = false;
            let args = args
                .into_iter()
                .map(|arg| {
                    if arg.name.as_deref() == Some("virtual_parent")
                        && matches!(
                            &arg.value,
                            RenderExpr::Literal {
                                value: Value::Boolean(true)
                            }
                        )
                    {
                        substituted = true;
                        Arg {
                            name: arg.name,
                            value: RenderExpr::Literal {
                                value: Value::String(parent_id.to_string()),
                            },
                        }
                    } else {
                        arg
                    }
                })
                .collect();
            tracing::info!(
                "[resolve_virtual_parent] name={name} parent_id={parent_id} substituted={substituted}"
            );
            RenderExpr::FunctionCall { name, args }
        }
        other => other,
    }
}

/// Domain layer for block-specific operations.
///
/// Wraps a `BackendEngine` reference and provides methods that encode
/// domain knowledge about blocks: layout discovery, rendering, task ranking,
/// and database initialization. The underlying `BackendEngine` remains a
/// reusable, domain-agnostic query engine.
pub struct BlockDomain<'a> {
    engine: &'a BackendEngine,
}

impl<'a> BlockDomain<'a> {
    pub(crate) fn new(engine: &'a BackendEngine) -> Self {
        Self { engine }
    }

    /// Look up a block's path from the block_with_path materialized view.
    #[tracing::instrument(skip(self))]
    pub async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String> {
        let mut params = HashMap::new();
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        let rows = self
            .engine
            .execute_query(BLOCK_PATH_LOOKUP_SQL.to_string(), params, None)
            .await?;

        if let Some(row) = rows.first() {
            if let Some(Value::String(path)) = row.get("path") {
                return Ok(path.clone());
            }
        }

        // Block not in block_with_path yet - use block_id as fallback path
        Ok(format!("/{}", block_id))
    }

    /// Render a block by its ID.
    ///
    /// Given a block ID, finds its query source child, compiles and executes the query,
    /// parses any render sibling into a RenderExpr, and returns the render expression
    /// plus a CDC stream (whose first batch contains the initial query results).
    #[tracing::instrument(skip(self), fields(block_id = %block_id, is_root))]
    pub async fn render_entity(
        &self,
        block_id: &EntityUri,
        preferred_variant: &Option<String>,
    ) -> Result<(RenderExpr, RowChangeStream)> {
        let block_info = match self.load_block_with_query_source(block_id).await {
            Ok(info) => info,
            Err(_) => return self.render_leaf_block(block_id).await,
        };

        let query_source = block_info
            .get("query_source")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("Block '{block_id}' has no query source child"))?
            .to_string();

        let query_language: QueryLanguage = block_info
            .get("query_language")
            .and_then(|v| v.as_string())
            .map(|s| s.parse::<QueryLanguage>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Block '{block_id}' has invalid query_language: {e}"))?
            .unwrap_or(QueryLanguage::HolonPrql);

        let parent_id = match block_info.get("parent_id") {
            Some(holon_api::Value::Null) | None => None,
            Some(_) => Some(
                uri_from_row(&block_info, "parent_id")
                    .context("Block has parent_id but it's malformed")?,
            ),
        };

        let block_path = self.lookup_block_path(block_id).await?;

        let context = QueryContext::for_block_with_path(block_id, parent_id, block_path);

        let sql = self.engine.compile_to_sql(&query_source, query_language)?;

        let change_stream = self
            .engine
            .query_and_watch(sql, HashMap::new(), Some(context))
            .await?;

        let has_render_source = block_info
            .get("render_source")
            .is_some_and(|v| !v.is_null());

        let result_expr = if has_render_source {
            Self::parse_render_source(&block_info)
        } else {
            self.collection_render_from_profile(block_id)
        };

        let render_expr = Self::wrap_in_query_source_switcher(
            block_id,
            result_expr,
            &query_source,
            query_language,
        );

        Ok((render_expr, change_stream))
    }

    /// Resolve collection-level render expression from entity profile variants.
    ///
    /// When no explicit `#+BEGIN_SRC render` block exists, the entity profile's
    /// `collection` section provides the default + variant render expressions.
    /// Wraps them in a `view_mode_switcher` widget so frontends can switch layouts.
    fn collection_render_from_profile(&self, entity_uri: &holon_api::EntityUri) -> RenderExpr {
        use holon_api::render_types::Arg;

        let variants = self.engine.profile_resolver().resolve_collection_variants();

        tracing::info!(
            "[collection_render_from_profile] entity_uri={entity_uri}, variants_count={}, variant_names={:?}",
            variants.len(),
            variants.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );

        if variants.is_empty() {
            tracing::warn!(
                "[collection_render_from_profile] No collection variants found, falling back to table()"
            );
            return RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: Vec::new(),
            };
        }

        // If only one variant (the Always default), use it directly — no switcher needed
        if variants.len() == 1 {
            return resolve_virtual_parent(variants[0].render.clone(), &entity_uri.to_string());
        }

        // Build a view_mode_switcher with entity_uri + mode_* template args
        let mut args = Vec::new();

        // entity_uri so each collection has independent view mode state
        args.push(Arg {
            name: Some("entity_uri".to_string()),
            value: RenderExpr::Literal {
                value: holon_api::Value::String(entity_uri.to_string()),
            },
        });

        // modes JSON
        let modes_json = serde_json::to_string(
            &variants
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "name": v.name,
                        "icon": collection_icon_for(&v.name),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());

        args.push(Arg {
            name: Some("modes".to_string()),
            value: RenderExpr::Literal {
                value: holon_api::Value::String(modes_json),
            },
        });

        for variant in &variants {
            args.push(Arg {
                name: Some(format!("mode_{}", variant.name)),
                value: resolve_virtual_parent(variant.render.clone(), &entity_uri.to_string()),
            });
        }

        RenderExpr::FunctionCall {
            name: "view_mode_switcher".to_string(),
            args,
        }
    }

    /// Add a `source` mode to a query-source block's render expression.
    ///
    /// If the underlying expression is already a `view_mode_switcher` (i.e. the
    /// collection has multiple variants like tree/table/board), `source` is
    /// appended as another mode so the user sees a single icon row and a
    /// single per-block view-mode state. Otherwise (single-variant collection,
    /// or explicit render source), a 2-mode (result + source) switcher wraps
    /// the expression.
    fn wrap_in_query_source_switcher(
        block_id: &holon_api::EntityUri,
        result_expr: RenderExpr,
        query_source: &str,
        query_language: QueryLanguage,
    ) -> RenderExpr {
        use holon_api::render_types::Arg;

        let mode_source_expr = RenderExpr::FunctionCall {
            name: "source_editor".to_string(),
            args: vec![
                Arg {
                    name: Some("language".to_string()),
                    value: RenderExpr::Literal {
                        value: Value::String(query_language.to_string()),
                    },
                },
                Arg {
                    name: Some("content".to_string()),
                    value: RenderExpr::Literal {
                        value: Value::String(query_source.to_string()),
                    },
                },
            ],
        };

        // Merge path: if result_expr is already a view_mode_switcher, append
        // `source` to its modes + add a `mode_source` template arg.
        if let RenderExpr::FunctionCall { name, mut args } = result_expr {
            if name == "view_mode_switcher" {
                for arg in args.iter_mut() {
                    if arg.name.as_deref() == Some("modes") {
                        if let RenderExpr::Literal {
                            value: Value::String(modes_json),
                        } = &mut arg.value
                        {
                            if let Ok(mut modes) =
                                serde_json::from_str::<Vec<serde_json::Value>>(modes_json)
                            {
                                modes.push(serde_json::json!({"name": "source", "icon": "code"}));
                                if let Ok(updated) = serde_json::to_string(&modes) {
                                    *modes_json = updated;
                                }
                            }
                        }
                    }
                }
                args.push(Arg {
                    name: Some("mode_source".to_string()),
                    value: mode_source_expr,
                });
                return RenderExpr::FunctionCall { name, args };
            }
            // Reconstruct so we can fall through to the wrap path.
            return Self::wrap_with_outer_switcher(
                block_id,
                RenderExpr::FunctionCall { name, args },
                mode_source_expr,
            );
        }
        Self::wrap_with_outer_switcher(block_id, result_expr, mode_source_expr)
    }

    /// Fallback for when the inner expression isn't a `view_mode_switcher`:
    /// wrap with a 2-mode (result, source) switcher. The `#qsrc` URI fragment
    /// keeps the wrap's state separate from any inner per-entity state.
    fn wrap_with_outer_switcher(
        block_id: &holon_api::EntityUri,
        result_expr: RenderExpr,
        mode_source_expr: RenderExpr,
    ) -> RenderExpr {
        use holon_api::render_types::Arg;

        let switcher_uri = format!("{}#qsrc", block_id);
        let modes_json =
            r#"[{"name":"result","icon":"list"},{"name":"source","icon":"code"}]"#.to_string();

        RenderExpr::FunctionCall {
            name: "view_mode_switcher".to_string(),
            args: vec![
                Arg {
                    name: Some("entity_uri".to_string()),
                    value: RenderExpr::Literal {
                        value: Value::String(switcher_uri),
                    },
                },
                Arg {
                    name: Some("modes".to_string()),
                    value: RenderExpr::Literal {
                        value: Value::String(modes_json),
                    },
                },
                Arg {
                    name: Some("mode_result".to_string()),
                    value: result_expr,
                },
                Arg {
                    name: Some("mode_source".to_string()),
                    value: mode_source_expr,
                },
            ],
        }
    }

    /// Render a leaf block (no query source child) via the `render_entity()` widget.
    ///
    /// Uses `query_and_watch` — the same live CDC path as non-leaf blocks — so
    /// property-only changes (e.g. `task_state` cycling) are picked up by the
    /// data matview and forwarded as `UiEvent::Data` without requiring a
    /// structural re-render.
    async fn render_leaf_block(
        &self,
        block_id: &EntityUri,
    ) -> Result<(RenderExpr, RowChangeStream)> {
        let sql = "SELECT * FROM block WHERE id = $block_id";
        let mut params = HashMap::new();
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        let change_stream = self
            .engine
            .query_and_watch(sql.to_string(), params, None)
            .await?;

        let render_expr = RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        Ok((render_expr, change_stream))
    }

    /// Load a block by ID and find its query source child + optional render sibling.
    #[tracing::instrument(skip(self))]
    async fn load_block_with_query_source(
        &self,
        block_id: &EntityUri,
    ) -> Result<HashMap<String, Value>> {
        let query_langs = QueryLanguage::sql_in_list();
        let sql = BLOCK_WITH_QUERY_SOURCE_SQL.replace("{query_langs}", &query_langs);

        let mut params = HashMap::new();
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        let rows = self.engine.execute_query(sql, params, None).await?;

        if rows.is_empty() {
            anyhow::bail!(
                "Block '{}' not found or has no query source child (prql/gql/sql)",
                block_id
            );
        }

        Ok(rows[0].clone())
    }

    /// Parse a render_source into a RenderExpr.
    fn parse_render_source(
        block_info: &HashMap<String, Value>,
    ) -> holon_api::render_types::RenderExpr {
        if let Some(Value::String(source)) = block_info.get("render_source") {
            match crate::render_dsl::parse_render_dsl(source) {
                Ok(expr) => return expr,
                Err(e) => {
                    tracing::warn!("Failed to parse render_source, defaulting to table(): {e}");
                }
            }
        }

        holon_api::render_types::RenderExpr::FunctionCall {
            name: "table".to_string(),
            args: Vec::new(),
        }
    }
}

fn collection_icon_for(name: &str) -> &'static str {
    match name {
        "table_view" | "table" => "table",
        "board_view" | "board" => "kanban",
        _ => "tree",
    }
}

impl<'a> BlockDomain<'a> {
    /// Rank all active task blocks using WSJF (Weighted Shortest Job First).
    pub async fn rank_tasks(&self) -> Result<crate::petri::RankResult> {
        let rows = self
            .engine
            .execute_query(TASK_BLOCKS_FOR_PETRI_SQL.to_string(), HashMap::new(), None)
            .await?;

        let blocks: Vec<holon_api::block::Block> = rows
            .into_iter()
            .map(holon_api::Block::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("rank_tasks: failed to parse block rows")?;

        Ok(crate::petri::rank_tasks(&blocks))
    }
}
