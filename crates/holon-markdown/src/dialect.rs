//! `MarkdownDialect` — atomic, orthogonal feature switches for the Markdown
//! dialect a vault uses.
//!
//! Each field toggles exactly one syntactic feature with **no hidden
//! coupling**: flipping one switch never changes another feature's behavior.
//! The parser gates every extractor on its own switch and writes every
//! feature's metadata into its own sidecar property; the renderer gates every
//! emission on the same switch. The one deliberate refinement is
//! [`nested_tags`](MarkdownDialect::nested_tags), which parameterizes how
//! [`inline_tags`](MarkdownDialect::inline_tags) parses and is inert when
//! inline tags are off — it touches no other feature.
//!
//! Two presets bracket the range:
//! - [`MarkdownDialect::obsidian`] — everything on. This is the crate's
//!   historical behavior and the [`Default`].
//! - [`MarkdownDialect::commonmark`] — everything off. No Obsidian extension
//!   is recognized; a useful orthogonality probe and a degrade target.

use holon_api::types::StateCategory;

/// Per-feature switches describing how a single vault's Markdown is parsed
/// and rendered. See the module docs for the orthogonality contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownDialect {
    /// `[[Note]]`, `[[Note|alias]]`, `[[Note#Heading]]`, `[[Note#^block]]`
    /// cross-file references → `wikilinks` sidecar property.
    pub wikilinks: bool,
    /// `![[Target]]` embeds → `embeds` sidecar property, and the embed
    /// syntax for image blocks on render. Independent of `wikilinks`: an
    /// embed is recorded only under `embeds`, a plain link only under
    /// `wikilinks`.
    pub embeds: bool,
    /// Same-file references `[[#Heading]]` / `[[#^block]]` → `self_links`
    /// sidecar property.
    pub self_links: bool,
    /// Trailing `^block-id` markers become the block's stable ID (stripped
    /// from content on parse, re-emitted on render). Off → markers stay as
    /// literal text and every block gets a fresh UUID.
    pub block_ids: bool,
    /// YAML `--- … ---` frontmatter. Off → a leading `---` is ordinary
    /// CommonMark and no document-level metadata is projected.
    pub yaml_frontmatter: bool,
    /// Project the frontmatter `aliases:` list onto a typed field (and the
    /// `aliases` document property). Off → `aliases` round-trips opaquely
    /// through `extra`, exactly like any other unrecognized key.
    pub frontmatter_aliases: bool,
    /// `#tag` inline tags → `inline_tags` sidecar property.
    pub inline_tags: bool,
    /// Allow `/` to extend an inline tag (`#area/sub` is one tag rather than
    /// `#area` + literal `/sub`). Refines `inline_tags`; inert when
    /// `inline_tags` is off.
    pub nested_tags: bool,
    /// `> [!type] title`, collapsible `[!type]-` / `[!type]+` callouts →
    /// `callouts` sidecar property.
    pub callouts: bool,
    /// `==text==` highlights → `highlights` sidecar property.
    pub highlights: bool,
    /// `%%inline%%` and `%%\n…\n%%` comments → `comments` sidecar property.
    pub comments: bool,
    /// GFM task markers (`[ ]` / `[x]` / `[/]`) at the head of a heading →
    /// `task_state` properties. The marker↔keyword table is
    /// [`task_keywords`](Self::task_keywords).
    pub gfm_tasks: bool,
    /// Marker↔keyword mapping consulted when `gfm_tasks` is on.
    pub task_keywords: TaskKeywords,
    /// Daily-note inference from a file's path. `None` disables it; `Some`
    /// records a `daily_note` / `date` document property for files whose
    /// stem matches the configured date format.
    pub daily_notes: Option<DailyNoteConfig>,
}

impl Default for MarkdownDialect {
    fn default() -> Self {
        Self::obsidian()
    }
}

impl MarkdownDialect {
    /// Full Obsidian flavor — every extension on. Daily-note inference stays
    /// off because its folder/format is vault-specific; opt in by setting
    /// [`daily_notes`](Self::daily_notes).
    pub fn obsidian() -> Self {
        Self {
            wikilinks: true,
            embeds: true,
            self_links: true,
            block_ids: true,
            yaml_frontmatter: true,
            frontmatter_aliases: true,
            inline_tags: true,
            nested_tags: true,
            callouts: true,
            highlights: true,
            comments: true,
            gfm_tasks: true,
            task_keywords: TaskKeywords::obsidian(),
            daily_notes: None,
        }
    }

    /// Plain CommonMark — every Obsidian extension off. `task_keywords` is
    /// retained but unused while `gfm_tasks` is false.
    pub fn commonmark() -> Self {
        Self {
            wikilinks: false,
            embeds: false,
            self_links: false,
            block_ids: false,
            yaml_frontmatter: false,
            frontmatter_aliases: false,
            inline_tags: false,
            nested_tags: false,
            callouts: false,
            highlights: false,
            comments: false,
            gfm_tasks: false,
            task_keywords: TaskKeywords::obsidian(),
            daily_notes: None,
        }
    }
}

/// Bidirectional `[marker]` ↔ task keyword table.
///
/// Markers are the text between the brackets (`" "`, `"x"`, `"/"`); lookup is
/// ASCII-case-insensitive so `[X]` and `[x]` resolve to the same row. The
/// first row also serves as the default marker when a block carries a task
/// keyword that is not in the table (e.g. an org keyword), so round-trips
/// never silently drop the task itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskKeywords {
    pub markers: Vec<TaskMarker>,
}

/// One `[marker]` ↔ keyword row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMarker {
    /// Text inside the brackets, e.g. `" "`, `"x"`, `"/"`.
    pub marker: String,
    /// Keyword stored on the block, e.g. `"TODO"`, `"DONE"`, `"DOING"`.
    pub keyword: String,
    /// Whether the keyword is an active or done state.
    pub category: StateCategory,
}

impl TaskKeywords {
    /// GFM/Obsidian defaults: `[ ]`→TODO, `[x]`→DONE, `[/]`→DOING.
    pub fn obsidian() -> Self {
        Self {
            markers: vec![
                TaskMarker {
                    marker: " ".into(),
                    keyword: "TODO".into(),
                    category: StateCategory::Active,
                },
                TaskMarker {
                    marker: "x".into(),
                    keyword: "DONE".into(),
                    category: StateCategory::Done,
                },
                TaskMarker {
                    marker: "/".into(),
                    keyword: "DOING".into(),
                    category: StateCategory::Active,
                },
            ],
        }
    }

    /// Row for a bracket marker (case-insensitive), if any.
    pub fn by_marker(&self, marker: &str) -> Option<&TaskMarker> {
        self.markers
            .iter()
            .find(|m| m.marker.eq_ignore_ascii_case(marker))
    }

    /// Bracket marker to emit for a keyword. Falls back to the first row's
    /// marker when the keyword is unknown, so the task marker is always
    /// rendered.
    pub fn marker_for_keyword(&self, keyword: &str) -> &str {
        self.markers
            .iter()
            .find(|m| m.keyword == keyword)
            .or_else(|| self.markers.first())
            .map(|m| m.marker.as_str())
            .unwrap_or(" ") // ALLOW(fallback): empty table → bare checkbox
    }
}

/// Daily-note inference settings. A file whose stem matches [`format`] (and,
/// if [`folder`] is set, that lives under that vault-relative folder) is
/// tagged as a daily note on the document block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyNoteConfig {
    /// Vault-relative folder daily notes live in. `None` matches any folder.
    pub folder: Option<String>,
    /// chrono `strftime` format the filename stem must match.
    pub format: String,
}

impl Default for DailyNoteConfig {
    fn default() -> Self {
        Self {
            folder: None,
            format: "%Y-%m-%d".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_obsidian() {
        assert_eq!(MarkdownDialect::default(), MarkdownDialect::obsidian());
    }

    #[test]
    fn obsidian_turns_every_extension_on() {
        let d = MarkdownDialect::obsidian();
        assert!(
            d.wikilinks
                && d.embeds
                && d.self_links
                && d.block_ids
                && d.yaml_frontmatter
                && d.frontmatter_aliases
                && d.inline_tags
                && d.nested_tags
                && d.callouts
                && d.highlights
                && d.comments
                && d.gfm_tasks
        );
    }

    #[test]
    fn commonmark_turns_every_extension_off() {
        let d = MarkdownDialect::commonmark();
        assert!(
            !d.wikilinks
                && !d.embeds
                && !d.self_links
                && !d.block_ids
                && !d.yaml_frontmatter
                && !d.frontmatter_aliases
                && !d.inline_tags
                && !d.nested_tags
                && !d.callouts
                && !d.highlights
                && !d.comments
                && !d.gfm_tasks
        );
        assert!(d.daily_notes.is_none());
    }

    #[test]
    fn task_marker_lookup_is_case_insensitive() {
        let k = TaskKeywords::obsidian();
        assert_eq!(k.by_marker("x").map(|m| m.keyword.as_str()), Some("DONE"));
        assert_eq!(k.by_marker("X").map(|m| m.keyword.as_str()), Some("DONE"));
        assert_eq!(k.by_marker(" ").map(|m| m.keyword.as_str()), Some("TODO"));
        assert_eq!(k.by_marker("/").map(|m| m.keyword.as_str()), Some("DOING"));
        assert!(k.by_marker("link").is_none());
    }

    #[test]
    fn keyword_to_marker_round_trips_and_falls_back() {
        let k = TaskKeywords::obsidian();
        assert_eq!(k.marker_for_keyword("DONE"), "x");
        assert_eq!(k.marker_for_keyword("DOING"), "/");
        assert_eq!(k.marker_for_keyword("TODO"), " ");
        // Unknown keyword falls back to the first row's marker, never dropped.
        assert_eq!(k.marker_for_keyword("WAITING"), " ");
    }
}
