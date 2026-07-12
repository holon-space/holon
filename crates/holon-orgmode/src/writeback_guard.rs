//! Ingest→write-back data-loss guard (BugFunnel row 28, P0 data-loss class).
//!
//! The vault-sync controller ingests an on-disk org file into the block store,
//! then re-renders the store's blocks back over that same file. When ingest
//! silently drops blocks — e.g. a create-txn FK rollback that aborts part of a
//! file WITHOUT surfacing an error — the re-render is a TRUNCATED projection,
//! and writing it back DELETES the dropped lines from the user's file. That is
//! permanent data loss with no error, no banner (row 28: ~20 lines of a real
//! vault file gone).
//!
//! This guard sits at the ingest→write-back boundary. It re-parses the on-disk
//! `source` and compares it against a [`SurvivingProjection`] — the block ids
//! and contents of the projection(s) about to be written — refusing the write
//! when a block present in `source` survives in NEITHER the projection's block
//! ids NOR its block contents. The caller quarantines the file (loud ERROR) so
//! no write-back path rewrites the truncated state over disk. (The evidence set
//! is a distinct type so it can later widen from one file to a union across a
//! companion and the sibling files its subtree materializes to — Fork B; the
//! union behavior is not wired here.)
//!
//! ## Why this boundary is unambiguous
//!
//! The check runs on the ingest→projection cycle, BEFORE any user interaction
//! with the freshly-ingested blocks. A block missing from the projection at
//! this point was lost by ingest, not deleted by the user — so refusing is
//! always correct here. (A later user-driven deletion goes through a different
//! write- back path and legitimately shrinks the file; this guard does not run
//! there.)
//!
//! ## Canonical reformat is NOT loss
//!
//! On first boot the app rewrites seeded pages in canonical form (reordered
//! header args, injected `#+ID:`, normalized whitespace). That is expected. The
//! anchor is block preservation, NOT byte equality: canonical reformat keeps
//! every block (same id, content reproduced), so every source block id-matches
//! or content-matches and the guard passes.
//!
//! ## A 3-way text merge is NOT loss
//!
//! When disk and the live store both edited the same block, ingest 3-way-merges
//! the content — producing text on neither side. The merged block keeps its id,
//! so it id-matches and the guard passes despite the content mutation. (Only
//! blocks matched by NEITHER id NOR content are treated as dropped.)

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

use crate::parser::parse_org_file;

/// A refused write-back: the freshly-ingested projection dropped block(s) that
/// exist in the on-disk source. Rewriting the file would destroy that content
/// permanently, so the caller MUST refuse the write and quarantine the file.
#[derive(Debug, Clone)]
pub struct IngestLoss {
    /// The file whose write-back is refused.
    pub path: PathBuf,
    /// Number of non-empty blocks parsed from the on-disk source.
    pub source_block_count: usize,
    /// Number of non-empty blocks in the projection about to be written.
    pub rendered_block_count: usize,
    /// One `id: excerpt` line per dropped source block (matched by neither id
    /// nor content in the projection).
    pub dropped: Vec<String>,
}

impl std::fmt::Display for IngestLoss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "INGEST DATA LOSS: write-back of {} would DELETE {} block(s) that exist on disk but \
             did NOT survive ingest (source has {} block(s), projection has {}). Refusing the \
             write-back to protect the file. Dropped blocks:\n  {}",
            self.path.display(),
            self.dropped.len(),
            self.source_block_count,
            self.rendered_block_count,
            self.dropped.join("\n  "),
        )
    }
}

impl std::error::Error for IngestLoss {}

/// The evidence set a source block must survive in to be considered preserved:
/// the block ids and non-empty normalized contents of the projection(s) that
/// write-back is about to put on disk.
///
/// In the per-file case (all current callers) this is built from a single
/// rendered file via [`from_rendered`](Self::from_rendered) and is
/// byte-for-byte equivalent to the guard's original in-line parse of
/// `rendered`. The type exists so the evidence base can later widen to a UNION
/// across a companion and the sibling files its subtree materializes to (Fork B
/// B1 behavior half) — NOT wired here; this commit is a pure signature
/// refactor.
#[derive(Debug, Clone, Default)]
pub struct SurvivingProjection {
    /// Every block id present in the projection(s) about to be written.
    ids: HashSet<String>,
    /// Every non-empty normalized block content in the projection(s).
    contents: HashSet<String>,
}

impl SurvivingProjection {
    /// Build the surviving-block evidence set from one rendered projection
    /// string (the per-file case). Parses `rendered` and collects every block
    /// id plus every non-empty normalized content — the exact sets the guard
    /// previously computed in-line. `root` is used only for stable file-id
    /// derivation while parsing and never affects the comparison.
    ///
    /// A projection that renders to unparseable text is a defect the caller
    /// must see, so the parse error is propagated (never swallowed).
    pub fn from_rendered(path: &Path, rendered: &str, root: &Path) -> anyhow::Result<Self> {
        let parsed =
            parse_org_file(path, rendered, &EntityUri::no_parent(), root).map_err(|e| {
                e.context(format!(
                    "write-back guard: parsing rendered projection of {} failed",
                    path.display()
                ))
            })?;
        let ids = parsed
            .blocks
            .iter()
            .map(|b| b.id.as_str().to_string())
            .collect();
        let contents = parsed
            .blocks
            .iter()
            .map(|b| normalize_content(&b.content))
            .filter(|c| !c.is_empty())
            .collect();
        Ok(Self { ids, contents })
    }

    /// Number of distinct non-empty contents — the projection's non-empty block
    /// count as reported in [`IngestLoss`].
    fn content_count(&self) -> usize {
        self.contents.len()
    }
}

/// Collapse all runs of whitespace to a single space and trim, so a block's
/// content matches across the legal canonical-reformat whitespace changes the
/// renderer applies. Empty after normalization means "no textual content".
fn normalize_content(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Short, single-line excerpt of a source block for the loud error message.
fn excerpt(block: &Block) -> String {
    let normalized = normalize_content(&block.content);
    let body: String = normalized.chars().take(60).collect();
    let ellipsis = if normalized.chars().count() > 60 {
        "…"
    } else {
        ""
    };
    format!("{}: {:?}{}", block.id.as_str(), body, ellipsis)
}

/// Guard the ingest→write-back boundary against SILENT block loss.
///
/// `source` is the on-disk file that was just ingested; `surviving` is the
/// evidence set — block ids and non-empty contents — of the projection(s)
/// write-back is about to put on disk (built via
/// [`SurvivingProjection::from_rendered`] for the per-file case). `root` is the
/// vault root (used only for stable file-id derivation while parsing `source`;
/// it never affects the comparison).
///
/// Returns `Err(IngestLoss)` when a non-empty block present in `source` matches
/// NEITHER a block id NOR a normalized block content in `surviving` — the
/// ingest dropped it and the write-back would delete it from disk. Returns
/// `Ok(())` for a lossless projection, including a legal canonical reformat or
/// a 3-way text merge (see module docs).
///
/// Parse errors on the `source` side are propagated (never swallowed): a source
/// that no longer parses is a defect the caller must see. (Projection parse
/// errors are surfaced when the caller builds `surviving`.)
pub fn ensure_ingest_lossless(
    path: &Path,
    source: &str,
    surviving: &SurvivingProjection,
    root: &Path,
) -> anyhow::Result<()> {
    let source_parsed =
        parse_org_file(path, source, &EntityUri::no_parent(), root).map_err(|e| {
            e.context(format!(
                "write-back guard: re-parsing on-disk source of {} failed",
                path.display()
            ))
        })?;

    let source_blocks: Vec<&Block> = source_parsed
        .blocks
        .iter()
        .filter(|b| !normalize_content(&b.content).is_empty())
        .collect();

    let mut dropped = Vec::new();
    for block in &source_blocks {
        let id_match = surviving.ids.contains(block.id.as_str());
        let content_match = surviving
            .contents
            .contains(&normalize_content(&block.content));
        if !id_match && !content_match {
            dropped.push(excerpt(block));
        }
    }

    if dropped.is_empty() {
        Ok(())
    } else {
        Err(IngestLoss {
            path: path.to_path_buf(),
            source_block_count: source_blocks.len(),
            rendered_block_count: surviving.content_count(),
            dropped,
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::org_renderer::OrgRenderer;

    fn path() -> PathBuf {
        PathBuf::from("/vault/GPUI.org")
    }
    fn root() -> PathBuf {
        PathBuf::from("/vault")
    }

    /// A file whose parse extracts several blocks. Render(parse(source))
    /// reproduces the source modulo canonical normalization, so the guard must
    /// PASS on the honest round trip.
    const SOURCE: &str = "\
#+ID: gpui-doc

* Flow mode shell
:PROPERTIES:
:ID: flow-mode-shell
:END:
The shell hosts the flow.

* Capture overlay
:PROPERTIES:
:ID: capture-mode-overlay
:END:
Overlay for capture.

* Palette
:PROPERTIES:
:ID: command-palette
:END:
Command palette body.
";

    fn render_projection(src: &str) -> String {
        let parsed = parse_org_file(&path(), src, &EntityUri::no_parent(), &root()).unwrap();
        OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            &path(),
            &parsed.document.id,
        )
    }

    /// Per-file surviving set from a rendered projection — the shape every
    /// current guard caller uses.
    fn surviving(rendered: &str) -> SurvivingProjection {
        SurvivingProjection::from_rendered(&path(), rendered, &root()).unwrap()
    }

    #[test]
    fn honest_round_trip_passes() {
        let rendered = render_projection(SOURCE);
        ensure_ingest_lossless(&path(), SOURCE, &surviving(&rendered), &root())
            .expect("faithful round trip must pass the guard");
    }

    #[test]
    fn canonical_reformat_passes() {
        // Non-canonical disk formatting: extra blank lines, trailing spaces.
        // The projection is the canonical render — byte-different but block-
        // preserving. The guard MUST allow it (legal first-boot canonicalization).
        let noisy = "\
#+ID: gpui-doc



* Flow mode shell
:PROPERTIES:
:ID: flow-mode-shell
:END:
   The shell hosts the flow.


* Capture overlay
:PROPERTIES:
:ID: capture-mode-overlay
:END:
Overlay for capture.
";
        let canonical = render_projection(noisy);
        assert_ne!(noisy, canonical, "reformat should change bytes");
        ensure_ingest_lossless(&path(), noisy, &surviving(&canonical), &root())
            .expect("canonical reformat must NOT be flagged as loss");
    }

    #[test]
    fn dropped_block_is_refused() {
        // Simulate a lossy ingest: the projection is the source MINUS the
        // `capture-mode-overlay` region (the shape of row 28 — a mid-file block
        // that FK-rolled-back and never landed). Write-back of this projection
        // would delete those lines from disk. The guard MUST refuse.
        let lossy_source_missing_overlay = "\
#+ID: gpui-doc

* Flow mode shell
:PROPERTIES:
:ID: flow-mode-shell
:END:
The shell hosts the flow.

* Palette
:PROPERTIES:
:ID: command-palette
:END:
Command palette body.
";
        let rendered = render_projection(lossy_source_missing_overlay);
        let err = ensure_ingest_lossless(&path(), SOURCE, &surviving(&rendered), &root())
            .expect_err("a projection missing a source block must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("capture-mode-overlay"),
            "error must name the dropped block; got: {msg}"
        );
        assert!(
            msg.contains("INGEST DATA LOSS"),
            "error must be loud; got: {msg}"
        );
    }

    #[test]
    fn content_edit_keeping_id_passes() {
        // A block whose CONTENT changed but whose :ID: is preserved (the 3-way
        // merge shape). Matched by id, so NOT flagged despite the text delta.
        let merged = "\
#+ID: gpui-doc

* Flow mode shell
:PROPERTIES:
:ID: flow-mode-shell
:END:
The shell hosts the flow — MERGED text nobody had on disk.

* Capture overlay
:PROPERTIES:
:ID: capture-mode-overlay
:END:
Overlay for capture.

* Palette
:PROPERTIES:
:ID: command-palette
:END:
Command palette body.
";
        let rendered = render_projection(merged);
        ensure_ingest_lossless(&path(), SOURCE, &surviving(&rendered), &root())
            .expect("id-preserving content merge must NOT be flagged as loss");
    }

    #[test]
    fn added_blocks_pass() {
        // The projection has MORE blocks than the source (user/seed additions
        // pushed into the file). Every source block still survives, so no loss.
        let with_extra = format!(
            "{SOURCE}\n* Newly added in app\n:PROPERTIES:\n:ID: brand-new\n:END:\nAdded body.\n"
        );
        let rendered = render_projection(&with_extra);
        ensure_ingest_lossless(&path(), SOURCE, &surviving(&rendered), &root())
            .expect("extra blocks in the projection are not loss");
    }
}
