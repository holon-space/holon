//! Write-back data-loss guard (BugFunnel row 28, P0 data-loss class; extended
//! to the block-driven write-back path by Fork B B1' per ADR 0025).
//!
//! The vault-sync controller ingests an on-disk org file into the block store,
//! then re-renders the store's blocks back over that same file. When a block
//! present on disk is silently absent from the re-render — e.g. a create-txn FK
//! rollback that aborts part of a file WITHOUT surfacing an error — writing the
//! projection back DELETES the dropped lines from the user's file. That is
//! permanent data loss with no error, no banner (row 28: ~20 lines of a real
//! vault file gone).
//!
//! This guard grounds every such absence. It re-parses the on-disk `source`
//! and, for each of its blocks, checks the block survives in one of the
//! GROUNDING sets:
//! - the [`SurvivingProjection`] union — the block ids and contents of the
//!   projection(s) about to be written (the file itself, unioned via
//!   [`SurvivingProjection::union_rendered`] with any sibling file the same
//!   convergence pass materializes — e.g. a child page legitimately de-inlined
//!   from a folder companion into its own file); OR
//! - `sanctioned_removals` — block ids the triggering delta's `Remove` set
//!   authorizes (a genuine user deletion; ADR 0025 op-grounding).
//!
//! A block grounded by NEITHER is loss by definition: refuse the write, and the
//! caller quarantines the file (loud ERROR) so no write-back path rewrites the
//! truncated state over disk.
//!
//! ## Op-grounding is the MECHANISM; the caller chooses the POLICY (ADR 0025)
//!
//! This module only *detects* ungrounded drops (via [`writeback_drops`] /
//! [`ensure_ingest_lossless`]); each write-back path decides what to DO with
//! the verdict, matched to how much intent that path holds:
//! - **ingest re-project** (the original row-28 site) is one of the two
//!   irreducibly intent-less boundaries but its `source` is a SETTLED user
//!   edit, so ANY ungrounded drop is loss → [`ensure_ingest_lossless`] →
//!   quarantine. Grounds only via the file's own projection; a permanent
//!   tripwire, exempt from "delete defensive code" sweeps.
//! - **`on_block_changed` / `on_block_removed` / `re_render_all_tracked`** are
//!   MID-FLIGHT, state-driven paths. Since the ADR 0025 ROOT-ITEM threading the
//!   feed preserves per-block `Remove` identity end-to-end (di.rs): a feed
//!   removal is routed to the owning file (`on_block_removed`, reverse lookup
//!   over the tracked projections) with the id as a sanctioned removal, and the
//!   ids no single file could consume accumulate into the sanctioned set the
//!   debounced `re_render_all_tracked` pass receives — every op-delivered
//!   deletion is grounded. What remains ungroundable are shrinks with no
//!   delivered op (cross-doc moves, TOCTOU-spent sanctions, matview-lag races),
//!   so these paths still run a MASS-TRUNCATION tripwire off the
//!   [`WritebackDrops`] verdict
//!   (`FileSyncController::tripwire_mass_truncation`): veto+quarantine only
//!   when the ungrounded-drop count exceeds a fraction of the block count (the
//!   row-28 signature), letting single/small drops pass. ADR 0025 names the
//!   follow-up that grounds those last classes (and tightens the tripwire
//!   toward zero): the C2b history relation.
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
/// In the per-file case this is built from a single rendered file via
/// [`from_rendered`](Self::from_rendered). The block-driven write-back path
/// (Fork B B1') widens it to a UNION across the file being written and every
/// sibling file the same convergence pass materializes, by folding each sibling
/// render in with [`union_rendered`](Self::union_rendered) — so a child page
/// legitimately de-inlined from a folder companion into its own file is
/// grounded by that sibling file rather than flagged as loss.
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

    /// Fold another rendered projection into this surviving set (UNION). Used
    /// on the block-driven write-back path to admit the sibling file(s) a
    /// child page de-inlined into: a block absent from the file being
    /// written but present in a sibling that the same convergence pass
    /// materializes is preserved, not lost. `path`/`root` are used only for
    /// stable file-id derivation while parsing and never affect the
    /// comparison. A sibling render that does not parse is a defect the
    /// caller must see, so the parse error is propagated.
    ///
    /// Crucially this also grounds the sibling's own DOCUMENT id: a child page
    /// that de-inlines from a companion becomes the `#+ID:` of its own file, so
    /// the de-inlined page block's id lives on the sibling as its document
    /// identity, NOT as a body block. Without folding `document.id` in, the
    /// page that legitimately moved would read as loss.
    pub fn union_rendered(
        &mut self,
        path: &Path,
        rendered: &str,
        root: &Path,
    ) -> anyhow::Result<()> {
        let parsed =
            parse_org_file(path, rendered, &EntityUri::no_parent(), root).map_err(|e| {
                e.context(format!(
                    "write-back guard: parsing sibling projection of {} failed",
                    path.display()
                ))
            })?;
        self.ids.insert(parsed.document.id.as_str().to_string());
        for block in &parsed.blocks {
            self.ids.insert(block.id.as_str().to_string());
            let content = normalize_content(&block.content);
            if !content.is_empty() {
                self.contents.insert(content);
            }
        }
        Ok(())
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

/// Guard a write-back against SILENT block loss (ADR 0025 op-grounding).
///
/// `source` is the on-disk file about to be overwritten; `surviving` is the
/// grounding union — block ids and non-empty contents — of the projection(s)
/// write-back is about to put on disk (the file itself, optionally unioned via
/// [`SurvivingProjection::union_rendered`] with the sibling files the same
/// convergence pass materializes). `sanctioned_removals` is the set of block
/// ids the triggering op authorizes to disappear (a genuine user deletion;
/// empty on the intent-less ingest and recovery paths). `root` is the vault
/// root (used only for stable file-id derivation while parsing `source`; it
/// never affects the comparison).
///
/// Returns `Err(IngestLoss)` when a non-empty block present in `source` is
/// grounded by NONE of: a `surviving` block id, a `surviving` normalized
/// content, or `sanctioned_removals` — the block was dropped and the write-back
/// would delete it from disk. Returns `Ok(())` for a lossless projection,
/// including a legal canonical reformat, a 3-way text merge, a sibling
/// de-inline (grounded by the union), or a sanctioned removal (see module
/// docs).
///
/// Parse errors on the `source` side are propagated (never swallowed): a source
/// that no longer parses is a defect the caller must see. (Projection parse
/// errors are surfaced when the caller builds `surviving`.)
pub fn ensure_ingest_lossless(
    path: &Path,
    source: &str,
    surviving: &SurvivingProjection,
    sanctioned_removals: &HashSet<String>,
    root: &Path,
) -> anyhow::Result<()> {
    let drops = writeback_drops(path, source, surviving, sanctioned_removals, root)?;
    if drops.dropped.is_empty() {
        Ok(())
    } else {
        Err(IngestLoss {
            path: path.to_path_buf(),
            source_block_count: drops.source_block_count,
            rendered_block_count: surviving.content_count(),
            dropped: drops.dropped,
        }
        .into())
    }
}

/// The ungrounded drops a write-back would cause — the guard VERDICT as data,
/// separated from real parse/IO failures (which are `Err`). `dropped` is one
/// `id: excerpt` per source block grounded by NEITHER the `surviving` union NOR
/// `sanctioned_removals`; empty means lossless.
///
/// This lets the two write-back POLICIES share one mechanism: the
/// intent-bearing ingest boundary wraps a non-empty verdict into a quarantining
/// [`IngestLoss`] (via [`ensure_ingest_lossless`]); the block-driven paths,
/// which cannot yet ground removals (ADR 0025 C2b follow-up), DISCLOSE the
/// verdict loudly and proceed. A source that no longer parses is a real defect
/// and is propagated as `Err`, never folded into the verdict.
#[derive(Debug, Clone, Default)]
pub struct WritebackDrops {
    /// Number of non-empty blocks parsed from `source`.
    pub source_block_count: usize,
    /// `id: excerpt` per ungrounded (dropped) source block.
    pub dropped: Vec<String>,
}

pub fn writeback_drops(
    path: &Path,
    source: &str,
    surviving: &SurvivingProjection,
    sanctioned_removals: &HashSet<String>,
    root: &Path,
) -> anyhow::Result<WritebackDrops> {
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
        let sanctioned = sanctioned_removals.contains(block.id.as_str());
        if !id_match && !content_match && !sanctioned {
            dropped.push(excerpt(block));
        }
    }

    Ok(WritebackDrops {
        source_block_count: source_blocks.len(),
        dropped,
    })
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

    /// The intent-less-boundary grounding: no sanctioned removals (ingest /
    /// recovery paths). Block-driven callers pass the delta's `Remove` ids.
    fn no_removals() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn honest_round_trip_passes() {
        let rendered = render_projection(SOURCE);
        ensure_ingest_lossless(
            &path(),
            SOURCE,
            &surviving(&rendered),
            &no_removals(),
            &root(),
        )
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
        ensure_ingest_lossless(
            &path(),
            noisy,
            &surviving(&canonical),
            &no_removals(),
            &root(),
        )
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
        let err = ensure_ingest_lossless(
            &path(),
            SOURCE,
            &surviving(&rendered),
            &no_removals(),
            &root(),
        )
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
        ensure_ingest_lossless(
            &path(),
            SOURCE,
            &surviving(&rendered),
            &no_removals(),
            &root(),
        )
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
        ensure_ingest_lossless(
            &path(),
            SOURCE,
            &surviving(&rendered),
            &no_removals(),
            &root(),
        )
        .expect("extra blocks in the projection are not loss");
    }

    /// Fork B B1' — a block dropped from the projection but sanctioned by the
    /// triggering delta's `Remove` set is a GENUINE user deletion, not loss:
    /// the guard passes and the shrunken file is written.
    #[test]
    fn sanctioned_removal_passes() {
        // Projection de-inlines `capture-mode-overlay` (row shape of a delete).
        let after_delete = "\
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
        let rendered = render_projection(after_delete);
        // The parsed source block id carries the scheme, exactly as the delta's
        // `Remove(EntityUri::block("capture-mode-overlay"))` would.
        let sanctioned = HashSet::from([EntityUri::block("capture-mode-overlay")
            .as_str()
            .to_string()]);
        ensure_ingest_lossless(&path(), SOURCE, &surviving(&rendered), &sanctioned, &root())
            .expect("a delta-sanctioned removal must NOT be flagged as loss");
    }

    /// Fork B B1' — a block dropped from the file being written but PRESENT in
    /// a sibling projection folded into the union (a child page de-inlined
    /// into its own materialized file) is grounded, not loss.
    #[test]
    fn sibling_union_grounds_deinlined_page() {
        // The companion render de-inlines `capture-mode-overlay`; a sibling file
        // materializes it, and its render is unioned into the surviving set.
        let deinlined_companion = "\
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
        let sibling_file = "\
#+ID: capture-mode-overlay
Overlay for capture.
";
        let mut union = surviving(&render_projection(deinlined_companion));
        union
            .union_rendered(
                &PathBuf::from("/vault/capture-mode-overlay.org"),
                &render_projection(sibling_file),
                &root(),
            )
            .unwrap();
        ensure_ingest_lossless(&path(), SOURCE, &union, &no_removals(), &root())
            .expect("a page de-inlined into a sibling file must be grounded by the union");
    }

    /// Fork B B1' — a block dropped with NEITHER a sanctioned removal NOR a
    /// sibling home is genuine loss and STILL vetoes (row 28 protection intact
    /// on the block-driven path).
    #[test]
    fn ungrounded_drop_still_vetoes() {
        let lossy = "\
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
        let rendered = render_projection(lossy);
        // Sanction a DIFFERENT id; the empty-sibling union has no capture-overlay.
        let unrelated = HashSet::from([EntityUri::block("some-other-block").as_str().to_string()]);
        let err =
            ensure_ingest_lossless(&path(), SOURCE, &surviving(&rendered), &unrelated, &root())
                .expect_err("an ungrounded drop must still be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("capture-mode-overlay") && msg.contains("INGEST DATA LOSS"),
            "veto must loudly name the dropped block; got: {msg}"
        );
    }

    /// Content-only grounding: a source block whose id changed in the
    /// projection but whose (normalized) content is unchanged is preserved, NOT
    /// loss. This exercises the `contents` arm of the surviving set — the
    /// `from_rendered` filter `!c.is_empty()` must KEEP non-empty contents.
    /// Deleting that `!` empties the content set, so this honest edit reads as
    /// data loss and the guard falsely vetoes.
    #[test]
    fn content_match_with_changed_id_passes() {
        let source = "\
#+ID: doc

* Heading
:PROPERTIES:
:ID: old-id
:END:
Shared body text.
";
        let projection = "\
#+ID: doc

* Heading
:PROPERTIES:
:ID: new-id
:END:
Shared body text.
";
        ensure_ingest_lossless(
            &path(),
            source,
            &surviving(projection),
            &no_removals(),
            &root(),
        )
        .expect("a block grounded only by unchanged content must pass");
    }

    /// Sibling content-only grounding: a block de-inlined into a sibling file
    /// (new id there) is grounded by the sibling's CONTENT folded in via
    /// `union_rendered`. Deleting the `!` in that method's
    /// `!content.is_empty()` drops the sibling's non-empty contents from
    /// the union, so the moved block reads as loss and the guard falsely
    /// vetoes.
    #[test]
    fn sibling_union_content_grounding_passes() {
        let source = "\
#+ID: main-doc

* Main heading
:PROPERTIES:
:ID: main-block
:END:
Main body.

* Child heading
:PROPERTIES:
:ID: child-in-main
:END:
Child page body.
";
        // Main projection: the child block is gone (de-inlined out).
        let main_projection = "\
#+ID: main-doc

* Main heading
:PROPERTIES:
:ID: main-block
:END:
Main body.
";
        // Sibling projection: same child CONTENT under a NEW id.
        let sibling_projection = "\
#+ID: child-doc

* Child heading
:PROPERTIES:
:ID: child-relocated
:END:
Child page body.
";
        let mut surviving =
            SurvivingProjection::from_rendered(&path(), main_projection, &root()).unwrap();
        let sibling_path = PathBuf::from("/vault/Child.org");
        surviving
            .union_rendered(&sibling_path, sibling_projection, &root())
            .unwrap();

        ensure_ingest_lossless(&path(), source, &surviving, &no_removals(), &root())
            .expect("a block grounded only by sibling content must pass");
    }
}
