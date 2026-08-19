//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "FileFormatAdapter port" "Rust"
//! @c4 uses holon-org-format "shared Block/MarkSpan substrate" "Rust"
//!
//! `holon-markdown` — read-only (Tier R/O) ingest adapters for foreign vaults:
//! Obsidian-flavored and LogSeq-flavored Markdown, parsed into the SAME `Block`
//! + `MarkSpan` substrate as org (see
//!   `docs/Proposals/ForeignVaultCompat-2026-07-12.md`).
//!
//! Both adapters implement `holon_core::FileFormatAdapter`. They are **read
//! only**: `render_document` / `render_blocks` panic (a write to a foreign file
//! is loss by definition under ADR 0025 until anchored write-back lands), and
//! `writeback_drops` always refuses. The [`ReadOnlyWriteGuard`] is the
//! controller-level gate that must veto any write to a foreign-vault doc before
//! a renderer is ever reached.

pub mod build;
pub mod inline;
pub mod obsidian;
pub mod params;

pub use obsidian::ObsidianMarkdownAdapter;

mod logseq;
use std::path::Path;

pub use logseq::LogseqMarkdownAdapter;

/// Which foreign flavor a vault root is, detected once from marker files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultFlavor {
    /// `.obsidian/` present.
    Obsidian,
    /// `logseq/config.edn` present.
    Logseq,
    /// Native Holon org vault.
    Org,
}

/// Classify a vault by its on-disk marker directories. A folder with both
/// markers is treated as LogSeq (its `.md` grammar is the stricter superset to
/// parse); neither marker ⇒ `Org` (the native default). Never guesses per file.
pub fn detect_flavor(root: &Path) -> VaultFlavor {
    if root.join("logseq").join("config.edn").exists() {
        VaultFlavor::Logseq
    } else if root.join(".obsidian").is_dir() {
        VaultFlavor::Obsidian
    } else {
        VaultFlavor::Org
    }
}

/// Tier gate: refuses any write-back to a doc that belongs to a read-only
/// foreign vault. This is the ADR-0025 "external file edits" boundary made
/// unconditional for R/O — a foreign file was not produced by a Holon op, so no
/// op can ground a write to it. The controller consults this BEFORE rendering;
/// a `false` from [`may_write`] must short-circuit the write path.
#[derive(Debug, Clone, Default)]
pub struct ReadOnlyWriteGuard {
    read_only: bool,
}

impl ReadOnlyWriteGuard {
    /// A guard for a read-only (foreign) vault: every write is vetoed.
    pub fn read_only() -> Self {
        Self { read_only: true }
    }

    /// A guard for a writable (native) vault: writes pass through.
    pub fn writable() -> Self {
        Self { read_only: false }
    }

    /// Whether a write-back to `path` is permitted. `false` ⇒ the caller MUST
    /// NOT write; doing so anyway is a data-loss bug.
    pub fn may_write(&self, _path: &Path) -> bool {
        !self.read_only
    }

    /// Assert-and-return the bytes a caller intends to write, panicking loudly
    /// if this guard is read-only. Use at the actual `fs.write` call site so a
    /// mis-routed write fails visibly instead of silently corrupting a vault.
    pub fn checked<'a>(&self, path: &Path, bytes: &'a str) -> &'a str {
        assert!(
            self.may_write(path),
            "ReadOnlyWriteGuard: refused write to read-only foreign vault file {}",
            path.display()
        );
        bytes
    }
}
