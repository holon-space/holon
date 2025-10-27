//! Obsidian-flavored Markdown parsing, rendering, and the
//! `MarkdownFormatAdapter` impl of `holon_core::FileFormatAdapter`.
//!
//! Second implementation of the file-format seam introduced in Phase 1 of
//! `codev/specs/0006-pre-velocity-refactors.md`. Org-mode is the first impl
//! (`holon-orgmode`), markdown is this one. Sharing the same trait lets the
//! `OrgSyncController::with_format(...)` constructor host either format
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
//! frontmatter + `[[wikilink]]` + `^block-id` markers. See
//! `docs/Architecture/Sync.md` for the full conventions.

pub mod file_format;
pub mod frontmatter;
pub mod parser;
pub mod renderer;
pub mod wikilink;

pub use file_format::MarkdownFormatAdapter;
pub use parser::{parse_markdown_file, ParseResult};
pub use renderer::MarkdownRenderer;
