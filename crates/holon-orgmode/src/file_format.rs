//! `OrgFormatAdapter` — implements `holon_core::FileFormatAdapter` for `.org`.
//!
//! Stateless wrapper: delegates to `holon_org_format::parser::parse_org_file`
//! and `holon_org_format::org_renderer::OrgRenderer` so the sync controller
//! can call parse/render through the trait without knowing the format.

use std::path::Path;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;

use crate::block_params::build_block_params;
use crate::models::OrgBlockExt;
use crate::models::OrgDocumentExt;
use crate::org_renderer::OrgRenderer;
use crate::parser::parse_doc_id;
use crate::parser::parse_org_file;

pub struct OrgFormatAdapter;

impl OrgFormatAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OrgFormatAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormatAdapter for OrgFormatAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["org"]
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let result = parse_org_file(path, content, parent_dir_id, root)?;
        Ok(FileFormatParseResult {
            document: result.document,
            blocks: result.blocks,
            blocks_needing_ids: result.headlines_needing_ids,
        })
    }

    fn render_document(
        &self,
        document: &Block,
        blocks: &[Block],
        file_path: &Path,
        file_id: &EntityUri,
    ) -> String {
        OrgRenderer::render_document(document, blocks, file_path, file_id)
    }

    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String {
        OrgRenderer::render_entitys(blocks, file_path, file_id)
    }

    fn doc_id_from_content(&self, content: &str) -> Option<String> {
        parse_doc_id(content)
    }

    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
    ) -> StorageEntity {
        build_block_params(block, parent_id, document_uri)
    }

    fn content_differs(&self, a: &Block, b: &Block) -> bool {
        a.content != b.content
            || a.parent_id != b.parent_id
            || a.content_type != b.content_type
            || a.source_language != b.source_language
            || a.source_name != b.source_name
            || a.task_state() != b.task_state()
            || a.priority() != b.priority()
            || a.tags() != b.tags()
            || a.scheduled() != b.scheduled()
            || a.deadline() != b.deadline()
            || a.drawer_properties() != b.drawer_properties()
            || a.sequence() != b.sequence()
        // Sibling order is no longer a per-block field (ADR 0005): it is
        // derived from document position and applied via `place_all`,
        // so it is not part of this content-equivalence check.
    }

    fn sync_document_metadata(&self, parsed: &Block, persisted: &mut Block) -> bool {
        let mut changed = false;
        let parsed_kws = parsed.todo_keywords();
        if parsed_kws != persisted.todo_keywords() {
            persisted.set_todo_keywords(parsed_kws);
            changed = true;
        }
        // The doc-root's OWN content — its title line plus the
        // pre-first-headline body — is what write-back renders above the first
        // headline. Left unsynced, the persisted root keeps its
        // filename-derived name and carries no body, so every write-back
        // deletes the user's root text and rewrites `#+TITLE:` to the file stem.
        // Only the BODY half. A doc-root's first content line is its name, and
        // `authoritative_name_chain` builds the file path from it — overwriting
        // it with the `#+TITLE:` value would rename every file whose title
        // differs from its stem. The title round-trips through `file_title`
        // below instead.
        let parsed_body = parsed
            .content
            .split_once('\n')
            .map(|(_, b)| b)
            .unwrap_or("");
        let name = persisted.title();
        let merged = if parsed_body.is_empty() {
            name
        } else {
            format!("{name}\n{parsed_body}")
        };
        if merged != persisted.content {
            persisted.content = merged;
            changed = true;
        }
        if parsed.file_title() != persisted.file_title() {
            persisted.set_file_title(parsed.file_title());
            changed = true;
        }
        changed
    }

    fn check_writeback_lossless(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sibling_renders: &[(&Path, &str)],
        sanctioned_removals: &std::collections::HashSet<String>,
        root: &Path,
    ) -> Result<()> {
        let mut surviving =
            crate::writeback_guard::SurvivingProjection::from_rendered(path, rendered, root)?;
        for (sibling_path, sibling_rendered) in sibling_renders {
            surviving.union_rendered(sibling_path, sibling_rendered, root)?;
        }
        crate::writeback_guard::ensure_ingest_lossless(
            path,
            source,
            &surviving,
            sanctioned_removals,
            root,
        )
    }

    fn writeback_drops(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sibling_renders: &[(&Path, &str)],
        sanctioned_removals: &std::collections::HashSet<String>,
        root: &Path,
    ) -> Result<holon_core::file_format::WritebackDropVerdict> {
        let mut surviving =
            crate::writeback_guard::SurvivingProjection::from_rendered(path, rendered, root)?;
        for (sibling_path, sibling_rendered) in sibling_renders {
            surviving.union_rendered(sibling_path, sibling_rendered, root)?;
        }
        let drops = crate::writeback_guard::writeback_drops(
            path,
            source,
            &surviving,
            sanctioned_removals,
            root,
        )?;
        Ok(holon_core::file_format::WritebackDropVerdict {
            dropped: drops.dropped,
            source_block_count: drops.source_block_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parse_returns_same_document_and_blocks_as_underlying_parser() {
        let adapter = OrgFormatAdapter::new();
        let path = PathBuf::from("/tmp/test.org");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "* Hello World\n:PROPERTIES:\n:ID: block-1\n:END:\n";

        let via_adapter = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_direct = parse_org_file(&path, content, &parent, &root).unwrap();

        assert_eq!(via_adapter.blocks.len(), via_direct.blocks.len());
        assert_eq!(via_adapter.document.id, via_direct.document.id);
        assert_eq!(
            via_adapter.blocks_needing_ids,
            via_direct.headlines_needing_ids
        );
    }

    /// Inc 2/3 org materialization: a mount PAGE (adopt-and-collapse page
    /// share) materializes to an actual `.org` file that (a) contains the
    /// shared content, and (b) round-trips the share markers so Inc 3's
    /// ingest guard recognizes it as a projection sink (never re-ingested).
    /// Asserts the real rendered file text, not just the SQL projection.
    #[test]
    fn mount_page_materializes_to_org_and_round_trips_share_markers() {
        use holon_api::share_props::SHARE_ROLE_MOUNT;
        use holon_api::share_props::SHARE_ROLE_PROPERTY;
        use holon_api::share_props::SHARED_TREE_ID_PROPERTY;

        let adapter = OrgFormatAdapter::new();
        let path = PathBuf::from("/tmp/My Shared Page.org");
        let root = PathBuf::from("/tmp");
        let doc_uri = EntityUri::block("mount-1");

        let mut mount = Block::new_text(doc_uri.clone(), EntityUri::no_parent(), "My Shared Page");
        mount.set_page(true);
        mount.set_property(SHARE_ROLE_PROPERTY, SHARE_ROLE_MOUNT);
        mount.set_property(SHARED_TREE_ID_PROPERTY, "stid-x");
        // ID drawer keeps identity stable across the render→parse round trip.
        mount.set_property("ID", "mount-1");

        let mut child = Block::new_text(
            EntityUri::block("p-child"),
            doc_uri.clone(),
            "Child under P",
        );
        child.set_property(SHARED_TREE_ID_PROPERTY, "stid-x");
        child.set_property("ID", "p-child");

        let org = adapter.render_document(&mount, &[child], &path, &doc_uri);
        // (a) the shared content is actually on disk.
        assert!(
            org.contains("Child under P"),
            "child content materializes:\n{org}"
        );

        // (b) re-parsing the materialized file detects it as a shared projection
        // (the Inc 3 guard predicate), so it is never re-ingested as global intent.
        let reparsed = adapter
            .parse(&path, &org, &EntityUri::no_parent(), &root)
            .unwrap();
        let detected = reparsed.document.is_share_mount()
            || reparsed
                .blocks
                .iter()
                .any(|b| b.is_share_mount() || b.shared_tree_id().is_some());
        assert!(
            detected,
            "materialized mount file must round-trip a share marker so the ingest guard skips \
             it; rendered:\n{org}"
        );
    }

    #[test]
    fn render_blocks_matches_underlying_renderer() {
        let adapter = OrgFormatAdapter::new();
        let path = PathBuf::from("/tmp/test.org");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "* Hello World\n:PROPERTIES:\n:ID: block-1\n:END:\n";

        let parsed = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_adapter = adapter.render_blocks(&parsed.blocks, &path, &parsed.document.id);
        let via_direct = OrgRenderer::render_entitys(&parsed.blocks, &path, &parsed.document.id);
        assert_eq!(via_adapter, via_direct);
    }

    #[test]
    fn extensions_returns_org() {
        let adapter = OrgFormatAdapter::new();
        assert_eq!(adapter.extensions(), &["org"]);
    }

    /// `content_differs` is the sole gate that decides whether a file-parsed
    /// block gets written back over the persisted one. A false negative
    /// silently DROPS a user's edit (worst bug class); a mangled comparison
    /// operator or an `||`→`&&` in the OR chain produces exactly that. This
    /// pins the two required behaviours: identity ⇒ no diff, and a change
    /// in ANY single compared dimension ⇒ diff. Each variant isolates one
    /// clause, so together they kill the whole-function `-> true`/`->
    /// false`, every `!= -> ==`, and every `|| -> &&` mutant in
    /// `content_differs`.
    #[test]
    fn content_differs_flags_each_compared_field_and_not_identity() {
        use holon_api::types::ContentType;
        use holon_api::types::Priority;
        use holon_api::types::SourceLanguage;
        use holon_api::types::Tags;
        use holon_api::types::TaskState;
        use holon_api::types::Timestamp;

        let adapter = OrgFormatAdapter::new();
        let path = PathBuf::from("/vault/doc.org");
        let root = PathBuf::from("/vault");
        let parent = EntityUri::no_parent();
        let parsed = adapter
            .parse(&path, "* Base headline\nBody line\n", &parent, &root)
            .unwrap();
        let base = parsed.blocks[0].clone();

        // Identity ⇒ not different (kills `content_differs -> true`).
        assert!(
            !adapter.content_differs(&base, &base),
            "a block must not differ from itself"
        );

        let mut v;

        v = base.clone();
        v.content = "Different content".into();
        assert!(
            adapter.content_differs(&base, &v),
            "content change undetected"
        );

        v = base.clone();
        v.parent_id = EntityUri::block("other-parent");
        assert!(
            adapter.content_differs(&base, &v),
            "parent_id change undetected"
        );

        v = base.clone();
        v.content_type = ContentType::Source;
        assert!(
            adapter.content_differs(&base, &v),
            "content_type change undetected"
        );

        v = base.clone();
        v.source_language = Some(SourceLanguage::Other("python".into()));
        assert!(
            adapter.content_differs(&base, &v),
            "source_language change undetected"
        );

        v = base.clone();
        v.source_name = Some("snippet".into());
        assert!(
            adapter.content_differs(&base, &v),
            "source_name change undetected"
        );

        v = base.clone();
        v.set_task_state(Some(TaskState::from_keyword("TODO")));
        assert!(
            adapter.content_differs(&base, &v),
            "task_state change undetected"
        );

        v = base.clone();
        v.set_priority(Some(Priority::from_int(1).unwrap()));
        assert!(
            adapter.content_differs(&base, &v),
            "priority change undetected"
        );

        v = base.clone();
        v.set_tags(Tags::from_tag_iter(["urgent".to_string()]));
        assert!(adapter.content_differs(&base, &v), "tags change undetected");

        v = base.clone();
        v.set_scheduled(Some(Timestamp::parse("<2026-02-21>").unwrap()));
        assert!(
            adapter.content_differs(&base, &v),
            "scheduled change undetected"
        );

        v = base.clone();
        v.set_deadline(Some(Timestamp::parse("<2026-03-01>").unwrap()));
        assert!(
            adapter.content_differs(&base, &v),
            "deadline change undetected"
        );

        v = base.clone();
        v.properties
            .insert("Custom".into(), holon_api::Value::String("x".into()));
        assert!(
            adapter.content_differs(&base, &v),
            "drawer property change undetected"
        );

        v = base.clone();
        v.set_sequence(base.sequence() + 1);
        assert!(
            adapter.content_differs(&base, &v),
            "sequence change undetected"
        );
    }
}
