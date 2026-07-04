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
pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
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
    // to the `block_tags`/`block_requires`/`advice_suppressed` junctions (see
    // schema_modules.rs::edge_fields). Always emit (even when empty) so an
    // empty Vec correctly clears stale junction rows on update, and so strict
    // row parsing downstream always sees all three columns.
    let arr: Vec<Value> = block
        .tags
        .iter()
        .map(|t| Value::String(t.clone()))
        .collect();
    params.insert("tags".into(), Value::Array(arr));

    let arr: Vec<Value> = block
        .requires
        .iter()
        .map(|r| Value::String(r.to_string()))
        .collect();
    params.insert("requires".into(), Value::Array(arr));

    let arr: Vec<Value> = block
        .advice_suppressed
        .iter()
        .map(|r| Value::String(r.to_string()))
        .collect();
    params.insert("advice_suppressed".into(), Value::Array(arr));

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
        // `drawer_properties()` emits `REQUIRES` for org *rendering* (the
        // org-edna dependency drawer). Here it must be skipped: `requires` is
        // an edge field already emitted as the typed `Value::Array` param above
        // (routed to the `block_requires` junction). Re-inserting it as a flat
        // string property would pollute `block.properties` with a stray
        // uppercase `REQUIRES` key that the reference model never has.
        if k.eq_ignore_ascii_case("requires") {
            continue;
        }
        // Same shape for `:ADVICE_SUPPRESSED:` (ADR 0021): typed edge field
        // already emitted as a `Value::Array` param above (routed to the
        // `advice_suppressed` junction); the drawer string must not leak into
        // `block.properties`.
        if k.eq_ignore_ascii_case("advice_suppressed") {
            continue;
        }
        params.insert(k.into(), Value::String(v));
    }

    params
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
            let params = build_block_params(block, &parsed.document.id, &parsed.document.id);
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
        let params = build_block_params(block, &parsed.document.id, &parsed.document.id);
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
        let params = build_block_params(&block, &parent, &parent);
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
        let params = build_block_params(&block, &parent, &parent);
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
        let params = build_block_params(&with_ts, &parent, &parent);
        assert_eq!(
            params.get("created_at").and_then(|v| v.as_i64()),
            Some(12345),
            "existing created_at must be preserved verbatim: {params:?}"
        );

        let mut without_ts = base_source_block();
        without_ts.created_at = 0;
        let params = build_block_params(&without_ts, &parent, &parent);
        let created = params
            .get("created_at")
            .and_then(|v| v.as_i64())
            .expect("created_at must be present");
        assert!(
            created > 0,
            "absent created_at must be defaulted to a positive `now`, got {created}"
        );
    }
}
