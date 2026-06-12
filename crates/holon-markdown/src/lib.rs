//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//!
//! Obsidian-flavored Markdown parsing + rendering and the `MarkdownFormatAdapter` impl of `holon_core::FileFormatAdapter`.
//!
//! Second implementation of the file-format seam introduced in Phase 1 of
//! `codev/specs/0006-pre-velocity-refactors.md`. Org-mode is the first impl
//! (`holon-orgmode`), markdown is this one. Sharing the same trait lets the
//! `FileSyncController::with_format(...)` constructor host either format
//! without touching the controller's logic.
//!
//! The crate intentionally keeps the parser, renderer, and adapter together
//! (rather than splitting into `holon-md-format` + `holon-markdown` like the
//! org pair) — the org split was historical and isn't load-bearing for the
//! seam. If/when a markdown crate grows multiple consumers, the split can
//! follow.
//!
//! ## Scope
//!
//! Targets Obsidian-style vaults: CommonMark + GFM task lists + YAML
//! frontmatter + `[[wikilink]]` + `^block-id` markers, plus callouts,
//! highlights, comments, inline tags, aliases, and self-links. Every feature
//! is an atomic, orthogonal switch on [`MarkdownDialect`] so a single crate
//! can match a given vault's settings or degrade all the way to CommonMark.
//! See `docs/Architecture/Sync.md` for the full conventions.

pub mod callout;
pub mod dialect;
pub mod file_format;
pub mod frontmatter;
pub mod inline;
pub mod parser;
pub mod renderer;
pub mod wikilink;

pub use callout::Callout;
pub use dialect::{DailyNoteConfig, MarkdownDialect, TaskKeywords, TaskMarker};
pub use file_format::MarkdownFormatAdapter;
pub use parser::{parse_markdown_file, ParseResult};
pub use renderer::MarkdownRenderer;
pub use wikilink::{classify_wikilinks, ClassifiedWikilinks, Wikilink};
