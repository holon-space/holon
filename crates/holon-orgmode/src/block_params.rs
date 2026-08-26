use std::collections::HashSet;
use std::sync::LazyLock;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::ContentType;

use crate::models::OrgBlockExt;

/// Build command parameters for a block create/update operation.
///
/// Converts a parsed `Block` into a flat `StorageEntity` suitable
/// for passing to `OperationProvider::execute_operation` (create/update).
///
/// The `document_uri` is inserted under `ROUTING_DOC_URI_KEY` as the
/// param-side routing hint. `SqlOperationProvider` lifts the value onto
/// the typed `Event::routing_doc_uri` field at its boundary; the consumer
/// (`FileSyncController`) reads the typed field, so it can route the
/// operation to the correct document regardless of where `parent_id`
/// points.
///
/// `previous` is the block as the file previously declared it. It makes the
/// file authoritative for the block's drawer: a key `previous` carried and
/// `block` no longer does is emitted as `Value::REMOVED` — the writer's
/// removal sentinel — so a renamed or deleted drawer key is cleared from the
/// store instead of surviving an insert-only merge forever. `None` for a
/// create, and for any caller not reconciling a file against its own prior
/// state.
pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
    previous: Option<&Block>,
) -> holon_api::StorageEntity {
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(block.id.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    // Routing metadata: tells FileSyncController which document this block
    // belongs to, even when parent_id is another block (not a document).
    params.insert(
        holon_api::ROUTING_DOC_URI_KEY.into(),
        Value::String(document_uri.to_string()),
    );
    params.insert("content".into(), Value::String(block.content.clone()));
    params.insert(
        "content_type".into(),
        Value::String(block.content_type.to_string()),
    );

    // Project Block.marks → SQL `marks` TEXT column as a JSON string, mirroring
    // the canonical Loro writer (`loro_sync_controller::block_to_params`). The
    // org parser extracts inline `[[…]]`/`*bold*` markup into `block.marks` and
    // stores the rendered label in `content`; dropping marks here re-emits the
    // stripped label on writeback and destroys the user's link syntax on disk.
    // `None` → omit (NULL); `Some` → JSON-encode. The column discriminator on
    // readback is `marks IS NOT NULL`.
    if let Some(ref marks) = block.marks {
        params.insert(
            "marks".into(),
            Value::String(holon_api::marks_to_json(marks)),
        );
    }

    // Timestamps must be provided explicitly as integers (millis).
    // The blocks table DDL has `DEFAULT (datetime('now'))` which produces TEXT,
    // but Block::from_entity expects i64. Always provide integer timestamps
    // to avoid this mismatch.
    let now = holon_api::clock::now_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".into(), Value::Integer(created));
    params.insert("updated_at".into(), Value::Integer(now));

    // Edge-typed fields — `SqlOperationProvider`'s edge partition routes these
    // to their junction tables (see schema_modules.rs::edge_fields). Emit EVERY
    // field, even when empty, so an empty Vec clears stale junction rows on
    // update and strict row parsing downstream always sees the full column set.
    for field in holon_api::EdgeField::ALL {
        params.insert(field.column().into(), field.param_value(block));
    }

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert("source_language".into(), Value::String(lang.to_string()));
        }
        if let Some(ref name) = block.source_name {
            params.insert("source_name".into(), Value::String(name.clone()));
        }
        let header_args = block.get_source_header_args();
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                params.insert("source_header_args".into(), Value::String(json));
            }
        }
    }

    if let Some(task_state) = block.task_state() {
        params.insert("task_state".into(), Value::String(task_state.to_string()));
        // `cycle_task_state` writes this sidecar in the same statement as
        // `task_state` (`sql_operation_provider.rs`'s `category_str_for_keyword`
        // pairing) so category-filtering queries can see the state without a
        // keyword-list join. The org parser already derived the category from
        // `#+TODO:` config (`TaskState::from_keyword_with_done_list`) — mirror
        // it here so file-originated tasks pair the same way, one source of
        // truth (`TaskState.category`) instead of two.
        params.insert(
            "task_state_category".into(),
            Value::String(task_state.category.as_str().to_string()),
        );
    }
    if let Some(priority) = block.priority() {
        params.insert("priority".into(), Value::Integer(priority.to_int() as i64));
    }
    // Tags are already serialized into the `tags` JSON-array param above
    // (lines 53-57); the legacy CSV-via-properties shape is gone. Skip the
    // OrgBlockExt::tags() shim here so we don't overwrite the JSON list with
    // a comma-separated string.
    if let Some(scheduled) = block.scheduled() {
        params.insert("scheduled".into(), Value::String(scheduled.to_string()));
    }
    if let Some(deadline) = block.deadline() {
        params.insert("deadline".into(), Value::String(deadline.to_string()));
    }

    // `collapsed` is document state (2026-07-11 ruling): parsed from the
    // `:COLLAPSED:` drawer into the typed Block field. Always emit (like
    // `tags`) so an update from a file edit that REMOVED the drawer property
    // correctly clears the column back to 0.
    params.insert("collapsed".into(), Value::Boolean(block.collapsed));
    params.insert("widget_only".into(), Value::Boolean(block.widget_only));

    params.insert("sequence".into(), Value::Integer(block.sequence()));

    // sort_key is intentionally NOT emitted here. The org parser's
    // `gen_n_keys` value used to land in the sink via this map and competed
    // with the consolidator's auto-assigned order key — two generators in
    // disjoint string spaces, producing the seed=42 SplitBlock ordering
    // panic (devlog 2026-05-14). The single authoritative order writer is
    // the consolidator's outbound projection, which materializes its order
    // key into the sink's `sort_key` column. Position intent enters the
    // system via `after_block_id` (lifted to `Event::position_after_block_id`
    // at the provider boundary) and drives the consolidator's move op.

    // Include org drawer properties (flat in block.properties)
    let id = block
        .get_block_id()
        .unwrap_or_else(|| block.id.id().to_string());
    params.insert("ID".into(), Value::String(id));

    for (k, v) in block.drawer_properties() {
        if is_edge_drawer_key(&k) || is_typed_field_drawer_key(&k) {
            continue;
        }
        if is_storage_column_key(&k) {
            warn_unrepresentable_drawer_key(&k, block);
            continue;
        }
        params.insert(k.into(), Value::String(v));
    }

    // `drawer_properties()` hides the `_`-prefixed authored-order carrier, so
    // the loop above cannot reach it. The renderer reads it back from the store
    // to replay the order the drawer was authored in.
    if let Some(order) = block.get_property(crate::models::org_props::DRAWER_ORDER) {
        params.insert(crate::models::org_props::DRAWER_ORDER.into(), order);
    }

    // The file is authoritative for its own drawer: a key it USED to declare
    // and no longer does must be cleared from the store, not merged forward.
    // `drawer_properties()` never yields `_`-prefixed keys, so store-managed
    // system keys — which no file can express — are structurally out of reach
    // here and survive untouched.
    if let Some(previous) = previous {
        for k in previous.drawer_properties().into_keys() {
            // A storage-column name is refused SILENTLY here. The emit loop
            // above already disclosed it for any key the file still declares,
            // and a removal is not a loss of authored data — it is a refusal to
            // write `SET <column> = NULL` over row state this builder does not
            // own (`sort_key` is the consolidator's order key).
            if is_edge_drawer_key(&k)
                || is_typed_field_drawer_key(&k)
                || is_storage_column_key(&k)
                || params.contains_key(&*k)
            {
                continue;
            }
            params.insert(k.into(), Value::REMOVED);
        }
    }

    params
}

/// True for the drawer keys `drawer_properties()` emits for org RENDERING that
/// are really typed edge fields, already carried as `Value::Array` params.
/// Re-inserting one as a flat string would pollute `block.properties` with a
/// stray key the reference model never has.
///
/// The drawer spelling and the column name differ (`:contributes-to:` vs
/// `contributes_to`), so this compares against both.
fn is_edge_drawer_key(key: &str) -> bool {
    holon_api::EdgeField::ALL.iter().any(|f| {
        let column = f.column();
        key.eq_ignore_ascii_case(column) || key.eq_ignore_ascii_case(&column.replace('_', "-"))
    })
}

/// True for the drawer keys `drawer_properties()` reconstructs from a typed
/// SCALAR `Block` field, the way [`is_edge_drawer_key`] covers the ones it
/// reconstructs from a typed EDGE field. Both are already carried as typed
/// params (`collapsed` / `widget_only`, emitted above); re-ingesting the drawer
/// spelling would ALSO park a stray uppercase string in `block.properties`,
/// which the reference model never has.
///
/// This is a narrow allowlist of the two keys Holon itself serializes, NOT a
/// case-insensitive match against the schema: matching case-insensitively would
/// over-refuse an ordinary user property such as `:Sort_Key:`.
fn is_typed_field_drawer_key(key: &str) -> bool {
    matches!(key, "COLLAPSED" | "WIDGET_ONLY")
}

/// The `block_raw` storage columns, as one set built once.
static BLOCK_STORAGE_COLUMNS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| holon_api::schema::BLOCK.columns().into_iter().collect());

/// True for a drawer key that spells a `block_raw` STORAGE COLUMN.
///
/// Such a key is not a property at all:
/// `SqlOperationProvider::partition_params` routes any param whose key names a
/// column straight to that column, so emitting one overwrites real row state
/// (`:id:` rewrites the primary key, `:content:` the block's text,
/// `:properties:` merges the drawer's own value into the property map) and
/// nulling one on removal emits `SET sort_key = NULL`, destroying the order key
/// the consolidator owns.
///
/// Matched case-sensitively against the schema, exactly as `partition_params`
/// matches, so `:Sort_Key:` stays an ordinary property instead of being
/// over-refused.
fn is_storage_column_key(key: &str) -> bool {
    BLOCK_STORAGE_COLUMNS.contains(key)
}

/// Disclose a refused drawer key. The value IS being dropped, so this must be
/// audible — a silent drop is the one outcome the repo's error ladder forbids
/// outright.
fn warn_unrepresentable_drawer_key(key: &str, block: &Block) {
    tracing::warn!(
        block = %block.id,
        key = %key,
        "org drawer key names a `block_raw` storage column and cannot be stored as a property \
         — dropping it from this block's ingest params. Rename the drawer key."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_org_file;

    /// Regression: org-ingested TODO/DONE blocks must carry BOTH `task_state`
    /// and its `task_state_category` sidecar in the params sent to
    /// create/update — otherwise category-filtering queries never see
    /// file-originated tasks (only ones cycled through the UI, which pairs
    /// them via `cycle_task_state`). The category is already derived by the
    /// parser (`TaskState::from_keyword_with_done_list` off `#+TODO:`
    /// config); this boundary must not drop it.
    #[test]
    fn ingested_todo_and_done_blocks_carry_task_state_category() {
        let org = "\
#+TODO: TODO | DONE

* TODO Buy milk
* DONE Ship it
";
        let parent_dir_id = EntityUri::no_parent();
        let path = std::path::Path::new("/vault/doc.org");
        let root = std::path::Path::new("/vault");
        let parsed = parse_org_file(path, org, &parent_dir_id, root).expect("parse org fixture");

        let headlines: Vec<&Block> = parsed
            .blocks
            .iter()
            .filter(|b| b.task_state().is_some())
            .collect();
        assert_eq!(
            headlines.len(),
            2,
            "expected exactly the TODO and DONE headlines, got {:?}",
            parsed.blocks
        );

        for block in headlines {
            let params = build_block_params(block, &parsed.document.id, &parsed.document.id, None);
            let task_state = params
                .get("task_state")
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| panic!("task_state missing from ingest params for {block:?}"));
            let category = params
                .get("task_state_category")
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| {
                    panic!("task_state_category missing from ingest params for {block:?}")
                });

            let expected_category = if task_state == "DONE" {
                "done"
            } else {
                "active"
            };
            assert_eq!(
                category, expected_category,
                "wrong task_state_category for keyword {task_state:?}"
            );
        }
    }

    /// `:COLLAPSED:` / `:WIDGET_ONLY:` are the drawer spellings
    /// `drawer_properties()` reconstructs from typed SCALAR `Block` fields for
    /// org WRITEBACK. The ingest leg must refuse them — they are already
    /// carried as typed params, so re-ingesting the drawer key parks a
    /// stray uppercase string in `properties` that the reference model
    /// never has.
    ///
    /// The refusal is a NARROW allowlist, and this test pins that: `:Sort_Key:`
    /// — an ordinary user property that differs from the `sort_key` storage
    /// column only in case — must SURVIVE. A case-insensitive match against the
    /// schema (the tempting one-liner) would swallow it, which is why the
    /// allowlist names the two keys Holon itself serializes instead.
    #[test]
    fn typed_field_drawer_keys_are_refused_but_case_variant_user_keys_survive() {
        let org = "* Folded\n:PROPERTIES:\n:ID: f1\n:COLLAPSED: t\n:WIDGET_ONLY: t\n:Sort_Key: \
                   zzz\n:END:\n";
        let parent_dir_id = EntityUri::no_parent();
        let path = std::path::Path::new("/vault/fold.org");
        let root = std::path::Path::new("/vault");
        let parsed = parse_org_file(path, org, &parent_dir_id, root).expect("parse org fixture");
        let block = parsed
            .blocks
            .iter()
            .find(|b| b.id.id() == "f1")
            .expect("the fixture must parse one headline");
        assert!(
            block.collapsed && block.widget_only,
            "control: the parser must lift both fold markers into their typed fields"
        );

        let params = build_block_params(block, &parsed.document.id, &parsed.document.id, None);

        assert_eq!(
            params.get("collapsed"),
            Some(&Value::Boolean(true)),
            "the typed param is what carries the fold state"
        );
        assert_eq!(
            params.get("widget_only"),
            Some(&Value::Boolean(true)),
            "same for the widget-only flag"
        );
        assert!(
            !params.contains_key("COLLAPSED") && !params.contains_key("WIDGET_ONLY"),
            "the drawer spellings must NOT also ride along as untyped properties: {params:?}"
        );
        assert_eq!(
            params.get("Sort_Key"),
            Some(&Value::String("zzz".to_string())),
            "`:Sort_Key:` is an ordinary user property — the refusal must be a narrow allowlist, \
             not a case-insensitive schema match that over-refuses it: {params:?}"
        );
    }

    /// Regression (dogfood 2026-07-10, on-disk data loss): org ingest extracts
    /// inline `[[…]]`/`*bold*` markup into `block.marks` and stores the
    /// rendered LABEL as `content`. The store-write params MUST carry
    /// `marks` (JSON, the exact shape `Block::try_from` reads back from the
    /// `marks` column) — otherwise readback yields `marks = None`,
    /// writeback re-emits the stripped label, and the user's link syntax is
    /// permanently destroyed on disk.
    ///
    /// Mirrors the canonical Loro write path
    /// (`loro_sync_controller::block_to_params`, which DOES emit `marks`); the
    /// org-ingest path here was the sole writer that dropped it.
    #[test]
    fn ingested_inline_marks_survive_store_write_params() {
        use crate::models::ToOrg;

        let org = "* See [[Linked Page]] here\n";
        let parent_dir_id = EntityUri::no_parent();
        let path = std::path::Path::new("/vault/doc.org");
        let root = std::path::Path::new("/vault");
        let parsed = parse_org_file(path, org, &parent_dir_id, root).expect("parse org fixture");

        let block = parsed
            .blocks
            .iter()
            .find(|b| b.marks.as_ref().is_some_and(|m| !m.is_empty()))
            .unwrap_or_else(|| {
                panic!(
                    "parser must extract a Link mark from `[[Linked Page]]`; got {:?}",
                    parsed.blocks
                )
            });

        // Parse side: the raw `[[…]]` was stripped to its label in content.
        assert!(
            block.content.contains("Linked Page") && !block.content.contains("[["),
            "expected stripped label in content, got {:?}",
            block.content
        );

        // Store-write side (org-ingest → SQL create/update params).
        let params = build_block_params(block, &parsed.document.id, &parsed.document.id, None);
        let marks_param = params.get("marks").unwrap_or_else(|| {
            panic!(
                "build_block_params dropped `marks` — org-ingested link syntax is lost on store \
                 write; params={params:?}"
            )
        });
        let stored_marks = holon_api::marks_from_json(
            marks_param
                .as_string()
                .expect("`marks` store param must be a JSON string"),
        )
        .expect("`marks` store param must hold valid marks JSON");
        assert_eq!(
            &stored_marks,
            block.marks.as_ref().unwrap(),
            "store-write marks diverge from parsed marks"
        );

        // Readback side: a block reconstructed with those marks re-emits the
        // link with its human-readable label — NOT stripped to plain text. The
        // parser resolves the bare wiki-target to a deterministic entity id
        // (`link_parser::deterministic_entity_id`), so writeback emits the
        // resolved `[[block:…][Linked Page]]` form; the label is what the user
        // sees and it survives. (The block_links junction is increment 2.)
        let mut restored = block.clone();
        restored.marks = Some(stored_marks);
        let render1 = restored.to_org();
        assert!(
            render1.contains("[[") && render1.contains("Linked Page"),
            "writeback must re-emit the link preserving its `Linked Page` label, got {render1:?}"
        );

        // Echo-stability (the writeback-loop hazard): re-parsing the writeback
        // and rendering again is a fixed point — the resolved target id is
        // deterministic, so there is no render↔parse churn on disk.
        let reparsed =
            parse_org_file(path, &render1, &parent_dir_id, root).expect("re-parse writeback");
        let rblock = reparsed
            .blocks
            .iter()
            .find(|b| b.marks.as_ref().is_some_and(|m| !m.is_empty()))
            .expect("re-parsed writeback must retain its marks");
        assert_eq!(
            rblock.to_org(),
            render1,
            "writeback must be byte-stable across repeated render/parse cycles"
        );
    }

    fn base_source_block() -> Block {
        let parent_dir_id = EntityUri::no_parent();
        let path = std::path::Path::new("/vault/doc.org");
        let root = std::path::Path::new("/vault");
        let parsed = parse_org_file(path, "* Base\n", &parent_dir_id, root).expect("parse fixture");
        let mut b = parsed.blocks[0].clone();
        b.content_type = ContentType::Source;
        b.source_language = Some(holon_api::types::SourceLanguage::Other("python".into()));
        b.source_name = Some("snippet".into());
        b
    }

    /// The `block.content_type == ContentType::Source` gate must admit source
    /// blocks so their `source_language`/`source_name` reach the store. If the
    /// comparison flips (`== -> !=`), Source blocks silently lose their
    /// language and name on write — a data-integrity regression for every
    /// code block.
    #[test]
    fn source_block_params_carry_language_and_name() {
        let block = base_source_block();
        let parent = EntityUri::block("parent-1");
        let params = build_block_params(&block, &parent, &parent, None);
        assert_eq!(
            params.get("source_language").and_then(|v| v.as_string()),
            Some("python"),
            "Source block dropped source_language: {params:?}"
        );
        assert_eq!(
            params.get("source_name").and_then(|v| v.as_string()),
            Some("snippet"),
            "Source block dropped source_name: {params:?}"
        );
    }

    /// A Source block with NO header args must omit `source_header_args`
    /// entirely (NULL), not emit an empty-map JSON. Deleting the `!` in
    /// `if !header_args.is_empty()` would serialize `{}` and pollute the
    /// column, producing spurious writeback churn on every save.
    #[test]
    fn source_block_without_header_args_omits_the_column() {
        let block = base_source_block();
        let parent = EntityUri::block("parent-1");
        let params = build_block_params(&block, &parent, &parent, None);
        assert!(
            !params.contains_key("source_header_args"),
            "empty header args must not emit source_header_args, got {:?}",
            params.get("source_header_args")
        );
    }

    /// `created_at` must be PRESERVED when the block already carries one
    /// (`> 0`), and only defaulted to `now` when absent (`0`). The comparison
    /// mutants (`> -> ==`, `> -> <`, `> -> >=`) each corrupt one of these arms:
    /// a real creation timestamp gets overwritten with `now`, or a `0` sentinel
    /// gets persisted verbatim.
    #[test]
    fn created_at_is_preserved_when_present_and_defaulted_when_zero() {
        let parent = EntityUri::block("parent-1");

        let mut with_ts = base_source_block();
        with_ts.created_at = 12345;
        let params = build_block_params(&with_ts, &parent, &parent, None);
        assert_eq!(
            params.get("created_at").and_then(|v| v.as_i64()),
            Some(12345),
            "existing created_at must be preserved verbatim: {params:?}"
        );

        let mut without_ts = base_source_block();
        without_ts.created_at = 0;
        let params = build_block_params(&without_ts, &parent, &parent, None);
        let created = params
            .get("created_at")
            .and_then(|v| v.as_i64())
            .expect("created_at must be present");
        assert!(
            created > 0,
            "absent created_at must be defaulted to a positive `now`, got {created}"
        );
    }

    /// A drawer key the file dropped is emitted as the `Value::REMOVED` removal
    /// sentinel; a key it still declares is emitted as its value; and the typed
    /// edge drawers (`REQUIRES`/`ADVICE_SUPPRESSED`) never take part — they
    /// travel as `Value::Array` params, so nulling them would clear a junction
    /// the file still populates.
    #[test]
    fn a_dropped_drawer_key_is_emitted_as_the_removal_sentinel() {
        let parent = EntityUri::no_parent();
        let previous = parse_one(
            "* Problem\n:PROPERTIES:\n:ID: p0\n:compass: problem\n:leads-to: m1\n:REQUIRES: \
             b1\n:END:\n",
        );
        let current = parse_one(
            "* Problem\n:PROPERTIES:\n:ID: p0\n:compass: problem\n:contributes-to: m1\n:END:\n",
        );

        let params = build_block_params(&current, &parent, &parent, Some(&previous));

        assert_eq!(
            params.get("leads-to"),
            Some(&Value::REMOVED),
            "a dropped drawer key must carry the removal sentinel: {params:?}"
        );
        assert_eq!(
            params.get("contributes_to"),
            Some(&Value::Array(vec![Value::String("block:m1".to_string())])),
            "the edge the file now declares must carry its targets: {params:?}"
        );
        assert_eq!(
            params.get("compass").and_then(|v| v.as_string()),
            Some("problem"),
            "an unchanged key must carry its value: {params:?}"
        );
        assert!(
            matches!(params.get("requires"), Some(Value::Array(_))),
            "the edge drawer must stay a typed array param, never a sentinel: {params:?}"
        );
    }

    /// A drawer key that spells a `block_raw` STORAGE COLUMN is refused in BOTH
    /// directions — never emitted, never nulled.
    ///
    /// Emitting one lets `partition_params` route the drawer's own value into
    /// the column (`:id:` rewrites the primary key, `:properties:` merges into
    /// the property map). Nulling one on removal is worse: the update nulls the
    /// `sort_key` column outright, destroying the order key the consolidator
    /// owns — the exact competing-writer class the `sort_key` comment at this
    /// function's call site warns about.
    #[test]
    fn a_drawer_key_naming_a_storage_column_never_reaches_the_params() {
        let parent = EntityUri::no_parent();
        // Every one of these is in `holon_api::schema::BLOCK.columns()` and in
        // NEITHER `drawer_properties`'s INTERNAL_KEYS nor the `_` namespace, so
        // without the guard each reaches the params.
        let columns = [
            "sort_key",
            "completed",
            "block_type",
            "write_seq",
            "source_name",
            "properties",
            "id",
            "content",
        ];
        let drawer: String = columns.iter().map(|c| format!(":{c}: x\n")).collect();
        let previous = parse_one(&format!(
            "* P\n:PROPERTIES:\n:ID: p0\n{drawer}:keep: v\n:END:\n"
        ));

        // EMIT direction: the authored value must not reach the column.
        let params = build_block_params(&previous, &parent, &parent, None);
        for c in columns {
            assert_ne!(
                params.get(c).and_then(|v| v.as_string()),
                Some("x"),
                "drawer key `{c}` names a storage column — its authored value must never \
                 reach the params: {params:?}"
            );
        }

        // REMOVAL direction: the file drops every one of them. None may be
        // nulled — `SET sort_key = NULL` is the destructive outcome.
        let current = parse_one("* P\n:PROPERTIES:\n:ID: p0\n:keep: v\n:END:\n");
        let params = build_block_params(&current, &parent, &parent, Some(&previous));
        for c in columns {
            assert_ne!(
                params.get(c),
                Some(&Value::REMOVED),
                "drawer key `{c}` names a storage column — removing it must never emit a \
                 NULL column write: {params:?}"
            );
        }
        // Non-vacuity: an ordinary key IS still removable in the same call.
        let current = parse_one("* P\n:PROPERTIES:\n:ID: p0\n:END:\n");
        let params = build_block_params(&current, &parent, &parent, Some(&previous));
        assert_eq!(
            params.get("keep"),
            Some(&Value::REMOVED),
            "the guard must not have disabled removal generally: {params:?}"
        );
    }

    /// The refusal's claim to "falls back visibly" rather than "silently
    /// degrades" rests entirely on the WARN, so the WARN is pinned, not just
    /// the params shape — deleting it would turn this into a silent drop.
    ///
    /// Captured through a LOCAL subscriber (`with_default`) rather than the
    /// `SpanCollector::global()` discipline: that collector lives in
    /// `holon-integration-tests`, which this crate cannot depend on. A local
    /// subscriber needs no touch-before-SUT ordering and leaks no global state.
    ///
    /// KNOWN INTERMITTENT under parallel test threads (measured 2/12 and 4/20;
    /// 15/15 in isolation) — `tracing` caches callsite interest
    /// PROCESS-globally, so a sibling thread running without a subscriber
    /// can re-cache this WARN as "never" and the capture buffer comes back
    /// empty. Pre-existing and not closed by the `rebuild_interest_cache()`
    /// below. Run with `--test-threads=1` to confirm the LOGIC this test is
    /// about. Real fix (a process-global test subscriber, or serializing
    /// the capture tests) is tracked as an orchestrator follow-up,
    /// IMPROVEMENTS 2026-08-22 "tracing interest-cache flake".
    #[test]
    fn the_refusal_is_disclosed_exactly_once_per_key() {
        let parent = EntityUri::no_parent();
        let block = parse_one("* P\n:PROPERTIES:\n:ID: p0\n:sort_key: zzz\n:keep: v\n:END:\n");

        let captured = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            // Does not fix the race; a sibling thread with no subscriber can
            // re-cache the callsite as `never` between the rebuild and the call.
            tracing::callsite::rebuild_interest_cache();
            build_block_params(&block, &parent, &parent, Some(&block));
        });

        let logged = captured.contents();
        assert!(
            logged.contains("sort_key"),
            "the refused key must be named in the WARN, or the drop is silent: {logged:?}"
        );
        assert!(
            logged.contains("p0"),
            "the WARN must name the block so the user can find it: {logged:?}"
        );
        assert_eq!(
            logged.matches("sort_key").count(),
            1,
            "exactly one WARN per refused key — the removal loop must not re-log what the \
             emit loop already disclosed: {logged:?}"
        );
    }

    /// Collects a local subscriber's output so a test can assert on it.
    #[derive(Clone, Default)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().expect("capture buffer").clone())
                .expect("tracing output is UTF-8")
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture buffer")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The org-ingest params builder is where the Compass contribution edge
    /// either reaches its junction or silently degrades into a drawer string in
    /// the properties blob — invisible to every edge-driven query. Pinned
    /// against `requires`, the other arc direction of the same relation
    /// (docs/Reference/CompassConventions.md), so the two cannot drift apart.
    #[test]
    fn contributes_to_is_emitted_as_a_typed_edge_not_a_drawer_string() {
        let parent = EntityUri::no_parent();
        let current = parse_one(
            "* Goal\n:PROPERTIES:\n:ID: p0\n:REQUIRES: blk-b\n:contributes-to: m1\n:END:\n",
        );

        let params = build_block_params(&current, &parent, &parent, None);

        assert_eq!(
            params.get("requires"),
            Some(&Value::Array(vec![Value::String(
                "block:blk-b".to_string()
            )])),
            "control: `requires` already routes to its junction: {params:?}"
        );
        assert_eq!(
            params.get("contributes_to"),
            Some(&Value::Array(vec![Value::String("block:m1".to_string())])),
            "`contributes-to` must route to the block_contributes_to junction as a typed array: \
             {params:?}"
        );
        assert!(
            !params.contains_key("contributes-to"),
            "the drawer spelling must not ALSO land in the properties blob: {params:?}"
        );
    }

    /// Without a `previous` there is no authority over peer keys, so nothing is
    /// nulled — the create path and every non-reconciling caller keep the
    /// insert-only merge.
    #[test]
    fn without_a_previous_block_no_key_is_nulled() {
        let parent = EntityUri::no_parent();
        let current = parse_one(
            "* Problem\n:PROPERTIES:\n:ID: p0\n:compass: problem\n:contributes-to: m1\n:END:\n",
        );
        let params = build_block_params(&current, &parent, &parent, None);
        assert!(
            !params.values().any(|v| v.is_removed()),
            "a create emits no removal sentinel: {params:?}"
        );
    }

    /// The single headline of a one-block org source.
    fn parse_one(source: &str) -> Block {
        let parsed = parse_org_file(
            std::path::Path::new("/vault/doc.org"),
            source,
            &EntityUri::no_parent(),
            std::path::Path::new("/vault"),
        )
        .expect("parse org source");
        parsed
            .blocks
            .into_iter()
            .find(|b| b.id.id() == "p0")
            .expect("the parse carries the `p0` headline")
    }
}
