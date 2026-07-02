//! Obsidian-flavored markdown → `Block` tree.
//!
//! Mirrors the shape of `holon_org_format::parser::parse_org_file`:
//! takes a single file's text, returns a document `Block`, a list of
//! child `Block`s, and the IDs of blocks that were freshly assigned
//! (caller writes them back to disk so the next parse is stable).
//!
//! ## Block model
//!
//! - **Document block**: file-as-entity. `content` is empty; metadata
//!   (title, tags) lives in properties. Frontmatter `extra` keys go through
//!   verbatim under a `frontmatter` JSON property so unknown keys round-trip.
//! - **Heading blocks**: one per ATX heading. `level` matches heading depth.
//!   Parent is the nearest enclosing heading of strictly lower depth, or
//!   the document itself for top-level headings. `content` is the heading
//!   text on the first line followed by the verbatim body markdown that
//!   appears between this heading and the next (excluding code fences,
//!   which are split out into source children).
//! - **Source children**: each fenced code block becomes a `ContentType::Source`
//!   child of the surrounding heading (same as org's `#+BEGIN_SRC`).
//! - **Preamble block**: content above the first heading is attached to
//!   the document block's `content` field — there is no separate "preamble"
//!   child block. This matches org's behavior (top-level paragraphs
//!   without a headline live on the document).
//!
//! ## Block IDs
//!
//! Obsidian's `^block-id` markers (trailing tokens like `Some text ^abc123`)
//! are recognized as stable IDs. When present they replace the heading's
//! generated UUID and the `^id` is stripped from the rendered content.
//! When absent, a UUID is generated and the heading is recorded in
//! `blocks_needing_ids` so the controller can re-render and write the
//! marker back.

use anyhow::Result;
use holon_api::block::Block;
use holon_api::types::{ContentType, SourceLanguage, Tags, TaskState};
use holon_api::EntityUri;
use markdown::mdast::Node;
use markdown::{Constructs, ParseOptions};
use std::path::Path;
use uuid::Uuid;

use crate::callout::extract_callouts;
use crate::dialect::{DailyNoteConfig, MarkdownDialect, TaskKeywords};
use crate::frontmatter::{self, Frontmatter};
use crate::inline::{extract_comments, extract_highlights, extract_inline_tags};
use crate::wikilink::classify_wikilinks;

pub struct ParseResult {
    pub document: Block,
    pub blocks: Vec<Block>,
    /// Block IDs that lacked a `^block-id` marker and were given a fresh
    /// UUID. The controller uses this list to decide whether to re-render
    /// and persist the new IDs.
    pub blocks_needing_ids: Vec<String>,
}

pub fn generate_file_id(path: &Path, root: &Path) -> EntityUri {
    let relative = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string());
    EntityUri::file(&relative)
}

pub fn parse_markdown_file(
    path: &Path,
    content: &str,
    parent_dir_id: &EntityUri,
    root: &Path,
    dialect: &MarkdownDialect,
) -> Result<ParseResult> {
    let file_id = generate_file_id(path, root);
    // Use the file stem (no extension) as the page title default so the // ALLOW(fallback): default-value comment
    // markdown and org adapters agree on the name shape.
    let file_name = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let (frontmatter, body) = if dialect.yaml_frontmatter {
        frontmatter::parse(content, dialect.frontmatter_aliases)?
    } else {
        (Frontmatter::default(), content)
    };

    // The page title is the first line of `content`. We hold onto `file_name`
    // here and prepend it once the body preamble has been parsed below.
    let title_line = file_name.clone();
    let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), "");
    document.set_page(true);
    apply_frontmatter_to_document(&mut document, &frontmatter);

    let options = parse_options();
    let ast = markdown::to_mdast(body, &options)
        .map_err(|m| anyhow::anyhow!("markdown parse failed: {m}"))?;

    let body_bytes = body.as_bytes();

    let mut blocks = Vec::new();
    let mut blocks_needing_ids = Vec::new();
    let mut sequence = 0i64;

    let root_children = match ast {
        Node::Root(r) => r.children,
        other => anyhow::bail!("markdown root was not Node::Root: {other:?}"),
    };

    let heading_indices: Vec<usize> = root_children
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n, Node::Heading(_)))
        .map(|(i, _)| i)
        .collect();

    let preamble_text = if let Some(&first_heading) = heading_indices.first() {
        let preamble = source_slice(
            body_bytes,
            0,
            position_offset_or(&root_children[first_heading], 0),
        );
        trim_trailing_blank_lines(preamble).to_string()
    } else {
        trim_trailing_blank_lines(body).to_string()
    };
    // The first content line is the page title; the body follows.
    document.content = if preamble_text.is_empty() {
        title_line
    } else {
        format!("{}\n{}", title_line, preamble_text)
    };
    apply_inline_metadata(&mut document, dialect);
    if let Some(cfg) = &dialect.daily_notes {
        if let Some(date) = daily_note_date(path, root, cfg) {
            document.set_property("daily_note", holon_api::Value::Boolean(true));
            document.set_property("date", holon_api::Value::String(date));
        }
    }

    // `parent_stack` holds (depth, EntityUri) frames. The current parent for
    // a heading of depth `d` is the nearest entry whose depth is `< d`; if
    // the stack is empty, the document is the parent.
    let mut parent_stack: Vec<(u8, EntityUri)> = Vec::new();
    let now_ms = holon_api::clock::now_millis();

    for hi in 0..heading_indices.len() {
        let heading_idx = heading_indices[hi];
        let next_heading_idx = heading_indices.get(hi + 1).copied();
        let heading_node = match &root_children[heading_idx] {
            Node::Heading(h) => h,
            _ => unreachable!("filtered above"),
        };
        let depth = heading_node.depth;

        while let Some(&(d, _)) = parent_stack.last() {
            if d >= depth {
                parent_stack.pop();
            } else {
                break;
            }
        }
        let parent_uri = parent_stack
            .last()
            .map(|(_, uri)| uri.clone())
            .unwrap_or_else(|| file_id.clone());

        let between_start = position_offset_or(&root_children[heading_idx], 0);
        let between_end = match next_heading_idx {
            Some(j) => position_offset_or(&root_children[j], body_bytes.len()),
            None => body_bytes.len(),
        };
        let between = source_slice(body_bytes, between_start, between_end);
        let (heading_text, body_text) = split_first_line(between);
        let heading_text = strip_atx_marker(heading_text);

        let (heading_text, body_text, block_id) = if dialect.block_ids {
            let (heading_text, marker) = pop_trailing_block_id(heading_text);
            let (body_text, body_marker) = pop_trailing_block_id_in_body(&body_text);
            (heading_text, body_text, marker.or(body_marker))
        } else {
            (heading_text, body_text, None)
        };

        let (id, needs_write) = match block_id {
            Some(id) => (id, false),
            None => (Uuid::new_v4().to_string(), true),
        };
        // Only record a writeback when block-id markers are in play; with
        // `block_ids` off there is no `^id` syntax to persist.
        if needs_write && dialect.block_ids {
            blocks_needing_ids.push(id.clone());
        }

        // Strip code fences out of the body — they become source-block children.
        let (body_without_code, source_blocks) = extract_code_fences(&body_text);

        let (task_state, heading_text) = if dialect.gfm_tasks {
            strip_task_state_prefix(heading_text, &dialect.task_keywords)
        } else {
            (None, heading_text.to_string())
        };

        let mut combined = heading_text.to_string();
        let body_clean = body_without_code.trim_end_matches('\n');
        if !body_clean.is_empty() {
            combined.push('\n');
            combined.push_str(body_clean);
        }

        let now = now_ms;
        let mut block = Block {
            id: EntityUri::block(&id),
            parent_id: parent_uri.clone(),
            content: combined,
            created_at: now,
            updated_at: now,
            ..Block::default()
        };
        // Block-level metadata that org carries on properties:
        block.set_property("ID", holon_api::Value::String(id.clone()));
        block.set_property("level", holon_api::Value::Integer(depth as i64));
        block.set_property("sequence", holon_api::Value::Integer(sequence));
        sequence += 1;
        if let Some(state) = task_state {
            block.set_property(
                "task_state",
                holon_api::Value::String(state.keyword.clone()),
            );
            block.set_property(
                "task_state_kind",
                holon_api::Value::String(if state.is_done() {
                    "done".into()
                } else {
                    "active".into()
                }),
            );
        }

        apply_inline_metadata(&mut block, dialect);

        blocks.push(block);

        for (src_index, src) in source_blocks.into_iter().enumerate() {
            let src_id = format!("{}::src::{}", id, src_index);
            let mut src_block = Block {
                id: EntityUri::block(&src_id),
                parent_id: EntityUri::block(&id),
                content: src.value,
                content_type: ContentType::Source,
                source_language: src
                    .language
                    .as_deref()
                    .and_then(|l| l.parse::<SourceLanguage>().ok()), // ALLOW(ok): unknown language → None is intentional
                created_at: now,
                updated_at: now,
                ..Block::default()
            };
            src_block.set_property("sequence", holon_api::Value::Integer(sequence));
            sequence += 1;
            blocks.push(src_block);
        }

        parent_stack.push((depth, EntityUri::block(&id)));
    }

    // Sibling order is conveyed positionally (document order); the order
    // owner mints `sort_key` on create via the sync controller's place loop.
    // The parser must NOT mint keys — see `holon-org-format` parser.
    Ok(ParseResult {
        document,
        blocks,
        blocks_needing_ids,
    })
}

fn parse_options() -> ParseOptions {
    let mut opts = ParseOptions::gfm();
    opts.constructs = Constructs {
        // Frontmatter is split out manually; turning the parser-level
        // construct off keeps the AST positions aligned with our `body`
        // slice (which is already post-frontmatter).
        frontmatter: false,
        ..Constructs::gfm()
    };
    opts
}

fn position_offset_or(node: &Node, default_offset: usize) -> usize {
    node.position()
        .map(|p| p.start.offset)
        .unwrap_or(default_offset)
}

fn source_slice(body: &[u8], start: usize, end: usize) -> &str {
    let s = start.min(body.len());
    let e = end.min(body.len());
    std::str::from_utf8(&body[s..e]).unwrap_or("")
}

fn split_first_line(s: &str) -> (&str, String) {
    match s.split_once('\n') {
        Some((first, rest)) => (first, rest.to_string()),
        None => (s, String::new()),
    }
}

/// Strip the leading `#` markers and exactly one space from an ATX heading
/// line. Defensive against missing space.
fn strip_atx_marker(line: &str) -> &str {
    let trimmed = line.trim_start_matches('#');
    trimmed.strip_prefix(' ').unwrap_or(trimmed).trim_end()
}

/// `^block-id` markers are an Obsidian convention. They appear at the end
/// of a paragraph or heading line, separated by whitespace. We accept
/// `[A-Za-z0-9_-]+` for the id.
fn pop_trailing_block_id(line: &str) -> (&str, Option<String>) {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_block_id_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start == end {
        return (line, None);
    }
    if start == 0 || bytes[start - 1] != b'^' {
        return (line, None);
    }
    let caret = start - 1;
    if caret == 0 || (bytes[caret - 1] != b' ' && bytes[caret - 1] != b'\t') {
        return (line, None);
    }
    let id = std::str::from_utf8(&bytes[start..end])
        .expect("block-id chars are ASCII")
        .to_string();
    let head = line[..caret].trim_end();
    (head, Some(id))
}

fn pop_trailing_block_id_in_body(body: &str) -> (String, Option<String>) {
    let trimmed_end = body.trim_end_matches('\n');
    if let Some((rest, last_line)) = trimmed_end.rsplit_once('\n') {
        let (cleaned_line, id) = pop_trailing_block_id(last_line);
        if let Some(id) = id {
            let mut out = rest.to_string();
            if !cleaned_line.is_empty() {
                out.push('\n');
                out.push_str(cleaned_line);
            }
            // Restore one trailing newline if the original had any.
            if body.ends_with('\n') {
                out.push('\n');
            }
            return (out, Some(id));
        }
    } else {
        let (cleaned_line, id) = pop_trailing_block_id(trimmed_end);
        if let Some(id) = id {
            let mut out = cleaned_line.to_string();
            if body.ends_with('\n') {
                out.push('\n');
            }
            return (out, Some(id));
        }
    }
    (body.to_string(), None)
}

fn is_block_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn trim_trailing_blank_lines(s: &str) -> &str {
    s.trim_end_matches(['\n', '\r', ' ', '\t'])
}

#[derive(Debug)]
struct CodeFence {
    language: Option<String>,
    value: String,
}

/// Walk the body line-by-line, peel out fenced code blocks, return the
/// remaining markdown plus the extracted code fences in source order.
///
/// We re-extract instead of reusing the AST so the body markdown returned
/// to `block.content` is byte-identical (including blank lines between
/// paragraphs) to the source between two headings.
fn extract_code_fences(body: &str) -> (String, Vec<CodeFence>) {
    let mut out = String::new();
    let mut fences = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0usize;
    let original_ended_with_newline = body.ends_with('\n');

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if let Some((fence_char, fence_len)) = open_fence(trimmed) {
            let language = parse_fence_info(&trimmed[fence_len..]);
            i += 1;
            let mut value = String::new();
            while i < lines.len() {
                let l = lines[i];
                if closes_fence(l.trim_start(), fence_char, fence_len) {
                    i += 1;
                    break;
                }
                if !value.is_empty() {
                    value.push('\n');
                }
                value.push_str(l);
                i += 1;
            }
            fences.push(CodeFence { language, value });
        } else {
            out.push_str(line);
            out.push('\n');
            i += 1;
        }
    }

    if !original_ended_with_newline && out.ends_with('\n') {
        out.pop();
    }

    (out, fences)
}

/// An opening fence per CommonMark: a run of >= 3 backticks or tildes.
/// Returns the fence character and the exact run length so the close check
/// can enforce the same-char, at-least-as-long rule.
fn open_fence(s: &str) -> Option<(char, usize)> {
    let ch = s.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = s.chars().take_while(|&c| c == ch).count();
    (len >= 3).then_some((ch, len))
}

/// A closing fence per CommonMark: a run of the SAME character, at least as
/// long as the opening run, with nothing but whitespace after it.
fn closes_fence(s: &str, fence_char: char, open_len: usize) -> bool {
    let run = s.chars().take_while(|&c| c == fence_char).count();
    run >= open_len && s[run..].trim().is_empty()
}

fn parse_fence_info(rest: &str) -> Option<String> {
    let info = rest.trim();
    if info.is_empty() {
        return None;
    }
    let lang = info.split_whitespace().next()?.to_string();
    Some(lang)
}

/// Recognize a `[marker]` task state at the head of a heading line using the
/// dialect's marker table. Returns (parsed_state_or_none,
/// remaining_text_after_marker). A leading `[…]` whose content is not a known
/// marker (e.g. `[link]`) is left untouched.
fn strip_task_state_prefix(line: &str, keywords: &TaskKeywords) -> (Option<TaskState>, String) {
    let trimmed = line.trim_start();
    let Some(after_open) = trimmed.strip_prefix('[') else {
        return (None, line.to_string());
    };
    let Some(close) = after_open.find(']') else {
        return (None, line.to_string());
    };
    let Some(marker) = keywords.by_marker(&after_open[..close]) else {
        return (None, line.to_string());
    };
    let rest = after_open[close + 1..].trim_start();
    (
        Some(TaskState::new(marker.keyword.clone(), marker.category)),
        rest.to_string(),
    )
}

fn apply_frontmatter_to_document(doc: &mut Block, fm: &Frontmatter) {
    if let Some(title) = &fm.title {
        doc.set_property("title", holon_api::Value::String(title.clone()));
    }
    if !fm.tags.is_empty() {
        let tags = Tags::from_tag_iter(fm.tags.clone());
        doc.set_property("tags", holon_api::Value::String(tags.to_csv()));
    }
    // `fm.aliases` is only populated when the dialect enables
    // `frontmatter_aliases`; otherwise aliases ride along inside `extra`.
    if !fm.aliases.is_empty() {
        doc.set_property(
            "aliases",
            holon_api::Value::String(to_json_string(&fm.aliases)),
        );
    }
    if !fm.extra.is_empty() {
        let json = serde_json::to_string(&fm.extra).expect("YAML extras serialize to JSON");
        doc.set_property("frontmatter_extra", holon_api::Value::String(json));
    }
}

/// Extract each enabled inline feature into its own sidecar property. The
/// source spans stay verbatim in `block.content`; only the metadata is lifted,
/// and each switch touches exactly one property so the features stay
/// orthogonal.
fn apply_inline_metadata(block: &mut Block, dialect: &MarkdownDialect) {
    let mut props: Vec<(&'static str, String)> = Vec::new();
    {
        let content = block.content.as_str();
        if dialect.wikilinks || dialect.embeds || dialect.self_links {
            let classified = classify_wikilinks(content);
            if dialect.wikilinks && !classified.links.is_empty() {
                props.push(("wikilinks", to_json_string(&classified.links)));
            }
            if dialect.embeds && !classified.embeds.is_empty() {
                props.push(("embeds", to_json_string(&classified.embeds)));
            }
            if dialect.self_links && !classified.self_links.is_empty() {
                props.push(("self_links", to_json_string(&classified.self_links)));
            }
        }
        if dialect.inline_tags {
            let tags = extract_inline_tags(content, dialect.nested_tags);
            if !tags.is_empty() {
                props.push(("inline_tags", to_json_string(&tags)));
            }
        }
        if dialect.highlights {
            let highlights = extract_highlights(content);
            if !highlights.is_empty() {
                props.push(("highlights", to_json_string(&highlights)));
            }
        }
        if dialect.comments {
            let comments = extract_comments(content);
            if !comments.is_empty() {
                props.push(("comments", to_json_string(&comments)));
            }
        }
        if dialect.callouts {
            let callouts = extract_callouts(content);
            if !callouts.is_empty() {
                props.push((
                    "callouts",
                    serde_json::to_string(&callouts).expect("callouts serialize to JSON"),
                ));
            }
        }
    }
    for (name, json) in props {
        block.set_property(name, holon_api::Value::String(json));
    }
}

fn to_json_string(values: &[String]) -> String {
    serde_json::to_string(values).expect("string list serializes to JSON")
}

/// ISO date for a daily note, or `None` when the path doesn't match the
/// configured folder/format. A non-matching name is not an error — it just
/// isn't a daily note.
fn daily_note_date(path: &Path, root: &Path, cfg: &DailyNoteConfig) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?; // ALLOW(ok): path outside root → not a daily note
    if let Some(folder) = &cfg.folder {
        if !relative.starts_with(folder) {
            return None;
        }
    }
    let stem = path.file_stem()?.to_str()?;
    let date = chrono::NaiveDate::parse_from_str(stem, &cfg.format).ok()?; // ALLOW(ok): non-date stem → not a daily note
    Some(date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(content: &str) -> ParseResult {
        let path = PathBuf::from("/test/note.md");
        let root = PathBuf::from("/test");
        parse_markdown_file(
            &path,
            content,
            &EntityUri::no_parent(),
            &root,
            &MarkdownDialect::obsidian(),
        )
        .unwrap()
    }

    #[test]
    fn parses_simple_headings() {
        let r = parse("# Top\n\nbody\n\n## Sub\n\nmore\n");
        assert_eq!(r.blocks.len(), 2);
        let top = &r.blocks[0];
        let sub = &r.blocks[1];
        assert!(top.content.starts_with("Top"));
        assert!(top.content.contains("body"));
        assert!(sub.content.starts_with("Sub"));
        assert_eq!(sub.parent_id, top.id);
    }

    #[test]
    fn frontmatter_is_applied_to_document() {
        let r = parse("---\ntitle: Hello\ntags: [a,b]\n---\n# Heading\n");
        assert_eq!(
            r.document
                .properties
                .get("title")
                .and_then(|v| v.as_string()),
            Some("Hello")
        );
    }

    #[test]
    fn unknown_frontmatter_keys_round_trip_via_extras() {
        let r = parse("---\ncreated: 2026-04-26\n---\n# H\n");
        assert!(r
            .document
            .properties
            .get("frontmatter_extra")
            .and_then(|v| v.as_string())
            .map(|s| s.contains("created"))
            .unwrap_or(false));
    }

    #[test]
    fn code_fence_becomes_source_child() {
        let r = parse("# H\n\n```python\nprint(1)\n```\n");
        assert_eq!(r.blocks.len(), 2);
        let head = &r.blocks[0];
        let src = &r.blocks[1];
        assert_eq!(src.content_type, ContentType::Source);
        assert_eq!(
            src.source_language,
            Some("python".parse::<SourceLanguage>().unwrap())
        );
        assert!(src.content.contains("print(1)"));
        assert_eq!(src.parent_id, head.id);
        assert!(!head.content.contains("```"));
    }

    #[test]
    fn longer_fence_encloses_shorter_fence() {
        let r = parse("# H ^aa\n\n````markdown\n```\ninner code\n```\n````\n\ntail para\n");
        let sources: Vec<_> = r
            .blocks
            .iter()
            .filter(|b| b.content_type == ContentType::Source)
            .collect();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].content, "```\ninner code\n```");
        let head = &r.blocks[0];
        assert!(!head.content.contains("inner code"));
        assert!(head.content.contains("tail para"));
    }

    #[test]
    fn fence_language_strips_marker_exactly_once() {
        let r = parse("# H ^aa\n\n````rust\ncode\n````\n");
        let src = r
            .blocks
            .iter()
            .find(|b| b.content_type == ContentType::Source)
            .unwrap();
        assert_eq!(
            src.source_language,
            Some("rust".parse::<SourceLanguage>().unwrap())
        );
    }

    #[test]
    fn block_id_marker_provides_stable_id() {
        let r = parse("# Heading ^abc-123\n\nbody\n");
        assert_eq!(r.blocks[0].id.id(), "abc-123");
        assert!(r.blocks_needing_ids.is_empty());
        assert!(!r.blocks[0].content.contains("^abc-123"));
    }

    #[test]
    fn missing_block_id_is_recorded_for_writeback() {
        let r = parse("# Plain heading\n");
        assert_eq!(r.blocks_needing_ids.len(), 1);
        assert_eq!(r.blocks_needing_ids[0], r.blocks[0].id.id());
    }

    #[test]
    fn task_state_marker_extracted() {
        let r = parse("# [ ] todo me ^id1\n");
        let b = &r.blocks[0];
        assert_eq!(
            b.properties.get("task_state").and_then(|v| v.as_string()),
            Some("TODO")
        );
        assert!(b.content.starts_with("todo me"));
    }

    #[test]
    fn nested_headings_attach_to_correct_parent() {
        let r = parse("# A\n## B\n# C\n");
        assert_eq!(r.blocks.len(), 3);
        let a = &r.blocks[0];
        let b = &r.blocks[1];
        let c = &r.blocks[2];
        assert_eq!(b.parent_id, a.id);
        assert_eq!(c.parent_id.scheme(), "file"); // back to document level
    }

    #[test]
    fn wikilinks_extracted_into_property() {
        let r = parse("# H\n\nsee [[Foo]] and [[Bar|alias]] ^id1\n");
        let json = r.blocks[0]
            .properties
            .get("wikilinks")
            .and_then(|v| v.as_string())
            .expect("wikilinks property present");
        let parsed: Vec<String> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, vec!["Foo".to_string(), "Bar".to_string()]);
    }

    #[test]
    fn preamble_above_first_heading_lives_on_document() {
        let r = parse("intro paragraph\n\n# First\n");
        assert!(r.document.content.contains("intro paragraph"));
    }

    #[test]
    fn no_headings_keeps_body_on_document() {
        let r = parse("just a paragraph\nand another\n");
        assert!(r.blocks.is_empty());
        assert!(r.document.content.contains("just a paragraph"));
    }
}
