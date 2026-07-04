//! Creation of the lazily-created child containers that hang off a tree node's
//! `meta` map (`content_raw`, `source_code`, `properties`, …).
//!
//! Every such child is created through [`ensure_text`] / [`ensure_map`], which
//! give two peers creating the same key concurrently *one* container instead of
//! two competing ones — without that, one peer's first write is silently lost
//! to the parent map's LWW.
//!
//! Loro refuses to place a mergeable child on a key that already holds a legacy
//! op-id child. Holon's migration is fresh-start, so hitting that refusal means
//! the on-disk state predates the migration; [`enrich_legacy_state_error`]
//! turns loro's `ArgErr` into the operator instruction that resolves it. The
//! concrete paths come from [`disclose_migration_paths`], registered at the
//! frontend wiring seam; without it the message names the defaults instead.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;
use loro::LoroMap;
use loro::LoroText;

/// The two artifacts an operator must delete to restart from mergeable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPaths {
    /// The Turso/SQLite database; its `-wal`/`-shm` siblings go with it.
    pub db_path: PathBuf,
    /// The Loro CRDT storage directory (`<vault>/.loro` by default).
    pub crdt_storage_dir: PathBuf,
}

static MIGRATION_PATHS: OnceLock<MigrationPaths> = OnceLock::new();

/// Register the paths this process would have to delete. Insert-only: the first
/// registration wins, so a second frontend in the same process cannot rewrite
/// the instruction under the first one.
pub fn disclose_migration_paths(paths: MigrationPaths) {
    let _ = MIGRATION_PATHS.set(paths);
}

fn deletion_instruction() -> String {
    match MIGRATION_PATHS.get() {
        Some(p) => format!(
            "delete {}* and {}",
            p.db_path.display(),
            p.crdt_storage_dir.display()
        ),
        None => "delete the holon database (`holon.db*` under the holon config dir) \
                 and the vault's `.loro` directory"
            .to_string(),
    }
}

fn enrich_legacy_state_error(key: &str, kind: &str, err: loro::LoroError) -> anyhow::Error {
    anyhow::anyhow!(
        "loro state predates the mergeable migration — {} and restart; \
         org files and their :ID:s are kept (mergeable {kind} at meta key {key:?}: {err:?})",
        deletion_instruction()
    )
}

/// Open (creating if absent) the mergeable `LoroText` child at `key`.
pub fn ensure_text(meta: &LoroMap, key: &str) -> Result<LoroText> {
    meta.ensure_mergeable_text(key)
        .map_err(|e| enrich_legacy_state_error(key, "text", e))
}

/// Open (creating if absent) the mergeable `LoroMap` child at `key`.
pub fn ensure_map(meta: &LoroMap, key: &str) -> Result<LoroMap> {
    meta.ensure_mergeable_map(key)
        .map_err(|e| enrich_legacy_state_error(key, "map", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_with_legacy_child(key: &str) -> (loro::LoroDoc, LoroMap) {
        let doc = loro::LoroDoc::new();
        let tree = doc.get_tree("blocks");
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        #[allow(deprecated)]
        let text: LoroText = meta.get_or_create_container(key, LoroText::new()).unwrap();
        text.insert(0, "legacy").unwrap();
        doc.commit();
        (doc, meta)
    }

    #[test]
    fn a_legacy_op_id_child_is_rejected_with_the_migration_instruction() {
        let (_doc, meta) = meta_with_legacy_child("content_raw");

        let err = ensure_text(&meta, "content_raw")
            .expect_err("a key holding a legacy op-id child must not be written through");

        let msg = err.to_string();
        assert!(
            msg.contains("predates the mergeable migration"),
            "error must name the cause; got: {msg}"
        );
        assert!(
            msg.contains(".loro") && msg.contains("holon.db"),
            "error must name both artifacts to delete; got: {msg}"
        );
        assert!(
            msg.contains(":ID:"),
            "error must reassure that org ids survive; got: {msg}"
        );
    }

    #[test]
    fn a_fresh_key_yields_a_mergeable_child() {
        let doc = loro::LoroDoc::new();
        let tree = doc.get_tree("blocks");
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();

        let text = ensure_text(&meta, "content_raw").unwrap();
        text.insert(0, "fresh").unwrap();

        assert_eq!(
            ensure_text(&meta, "content_raw").unwrap().to_string(),
            "fresh"
        );
    }
}
