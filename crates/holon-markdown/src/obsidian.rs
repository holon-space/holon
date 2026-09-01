//! `ObsidianMarkdownAdapter` — Tier R/O ingest of Obsidian-flavored `.md`.
//!
//! Obsidian is free-form Markdown, so it maps at **paragraph/heading-block**
//! granularity (not one-block-per-line like LogSeq): headings form the tree,
//! each paragraph / list item is a child block. YAML frontmatter → document
//! properties + `tags`/`aliases`. Wikilinks / embeds / `#tags` → marks + tag
//! edge field. Callouts (`> [!note]`) and `%% comments %%` become disclosed
//! opaque blocks (verbatim). Trailing `^blockid` anchors are captured as ids.

use std::path::Path;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_core::file_format::WriteTier;

use crate::build::opaque_block;
use crate::build::text_block;
use crate::params::build_block_params;

pub struct ObsidianMarkdownAdapter;

impl ObsidianMarkdownAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ObsidianMarkdownAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal frontmatter model: scalar values, inline `[a, b]` lists, and block
/// `-`-lists. Sufficient for `tags` / `aliases` / typed scalar properties.
struct Frontmatter {
    scalars: Vec<(String, String)>,
    lists: Vec<(String, Vec<String>)>,
}

fn parse_frontmatter(content: &str) -> (Frontmatter, usize) {
    let mut fm = Frontmatter {
        scalars: Vec::new(),
        lists: Vec::new(),
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return (fm, 0);
    }
    let mut i = 1;
    let mut cur_list_key: Option<String> = None;
    let mut cur_list: Vec<String> = Vec::new();
    while i < lines.len() {
        let l = lines[i];
        if l.trim_end() == "---" {
            i += 1;
            break;
        }
        let list_item = l.trim_start();
        if let Some(item) = list_item.strip_prefix("- ") {
            if cur_list_key.is_some() {
                cur_list.push(strip_quotes(item.trim()));
                i += 1;
                continue;
            }
        }
        // flush a pending block-list before a new key
        if let Some(k) = cur_list_key.take() {
            fm.lists.push((k, std::mem::take(&mut cur_list)));
        }
        if let Some((k, v)) = l.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim();
            if val.is_empty() {
                cur_list_key = Some(key); // block list follows
            } else if val.starts_with('[') && val.ends_with(']') {
                let items = val[1..val.len() - 1]
                    .split(',')
                    .map(|s| strip_quotes(s.trim()))
                    .filter(|s| !s.is_empty())
                    .collect();
                fm.lists.push((key, items));
            } else {
                fm.scalars.push((key, strip_quotes(val)));
            }
        }
        i += 1;
    }
    if let Some(k) = cur_list_key.take() {
        fm.lists.push((k, cur_list));
    }
    // byte offset where the body begins
    let body_start_line = i;
    let offset = lines
        .iter()
        .take(body_start_line)
        .map(|l| l.len() + 1)
        .sum();
    (fm, offset)
}

fn strip_quotes(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// Split a trailing ` ^blockid` anchor off a line. Returns (content, id?).
fn split_block_anchor(line: &str) -> (String, Option<String>) {
    if let Some(pos) = line.rfind(" ^") {
        let id = &line[pos + 2..];
        if !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return (line[..pos].to_string(), Some(id.to_string()));
        }
    }
    (line.to_string(), None)
}

impl FileFormatAdapter for ObsidianMarkdownAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["md", "markdown"]
    }

    fn write_tier(&self) -> WriteTier {
        WriteTier::ReadOnly
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let (fm, _body_offset) = parse_frontmatter(content);

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let title = fm
            .scalars
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("title"))
            .map(|(_, v)| v.clone())
            .unwrap_or(stem);

        let rel = path.strip_prefix(root).unwrap_or(path);
        let file_id = EntityUri::file(&rel.to_string_lossy());
        let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), title);
        document.set_page(true);
        for (k, v) in &fm.scalars {
            if k.eq_ignore_ascii_case("title") {
                continue;
            }
            if k.eq_ignore_ascii_case("tags") || k.eq_ignore_ascii_case("aliases") {
                // A scalar `tags: a, b` / `tags: project` is a shorthand list.
                for t in v
                    .split([',', ' '])
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    document.tags.insert(t.to_string());
                }
                continue;
            }
            document.set_property(k.clone(), v.clone());
        }
        for (k, items) in &fm.lists {
            if k.eq_ignore_ascii_case("tags") || k.eq_ignore_ascii_case("aliases") {
                for it in items {
                    document.tags.insert(it.clone());
                }
            } else {
                document.set_property(k.clone(), items.join(", "));
            }
        }

        // Body starts after the frontmatter block.
        let body: Vec<&str> = {
            let all: Vec<&str> = content.lines().collect();
            let mut start = 0;
            if all.first().map(|l| l.trim_end()) == Some("---") {
                if let Some(end) = all.iter().skip(1).position(|l| l.trim_end() == "---") {
                    start = end + 2;
                }
            }
            all[start..].to_vec()
        };

        let mut blocks: Vec<Block> = Vec::new();
        let mut seq = 0usize;
        let mut heading_stack: Vec<(usize, EntityUri)> = Vec::new();
        let mut i = 0;

        let mint = |seq: &mut usize, file_id: &EntityUri, explicit: Option<String>| -> EntityUri {
            let id = match explicit {
                Some(e) => EntityUri::block(&e),
                None => EntityUri::block(&format!("{}::b::{}", file_id.id(), seq)),
            };
            *seq += 1;
            id
        };

        while i < body.len() {
            let line = body[i];
            let trimmed = line.trim_start();

            if trimmed.is_empty() {
                i += 1;
                continue;
            }

            // --- Heading
            if let Some(level) = heading_level(trimmed) {
                let (text, anchor) = split_block_anchor(trimmed[level..].trim());
                while let Some((l, _)) = heading_stack.last() {
                    if *l >= level {
                        heading_stack.pop();
                    } else {
                        break;
                    }
                }
                let parent = heading_stack
                    .last()
                    .map(|(_, id)| id.clone())
                    .unwrap_or_else(|| file_id.clone());
                let id = mint(&mut seq, &file_id, anchor);
                let block = text_block(id.clone(), parent, &text);
                heading_stack.push((level, id));
                blocks.push(block);
                i += 1;
                continue;
            }

            let parent = heading_stack
                .last()
                .map(|(_, id)| id.clone())
                .unwrap_or_else(|| file_id.clone());

            // --- Callout `> [!type]` (+ following `>` lines) → opaque
            if trimmed.starts_with("> [!") {
                let mut src = vec![line.to_string()];
                let mut j = i + 1;
                while j < body.len() && body[j].trim_start().starts_with('>') {
                    src.push(body[j].to_string());
                    j += 1;
                }
                let id = mint(&mut seq, &file_id, None);
                blocks.push(opaque_block(id, parent, "callout", &src.join("\n")));
                i = j;
                continue;
            }

            // --- Standalone comment `%% ... %%` → opaque
            if let Some(after_open) = trimmed.strip_prefix("%%") {
                let mut src = vec![line.to_string()];
                let mut j = i;
                if !after_open.trim_end().ends_with("%%") {
                    j += 1;
                    while j < body.len() {
                        src.push(body[j].to_string());
                        if body[j].trim_end().ends_with("%%") {
                            break;
                        }
                        j += 1;
                    }
                }
                let id = mint(&mut seq, &file_id, None);
                blocks.push(opaque_block(id, parent, "comment", &src.join("\n")));
                i = j + 1;
                continue;
            }

            // --- Embed `![[...]]` on its own line → opaque
            if trimmed.starts_with("![[") {
                let id = mint(&mut seq, &file_id, None);
                blocks.push(opaque_block(id, parent, "embed", trimmed));
                i += 1;
                continue;
            }

            // --- List item (task or plain) → one block per item
            if let Some(rest) = list_item_body(trimmed) {
                let (text, task) = checkbox_state(rest);
                let (text, anchor) = split_block_anchor(&text);
                let id = mint(&mut seq, &file_id, anchor);
                let mut block = text_block(id, parent, &prefix_task(task, &text));
                // checkbox already encoded as TODO/DONE marker in prefix_task
                let _ = &mut block;
                blocks.push(block);
                i += 1;
                continue;
            }

            // --- Fenced code block → preserved verbatim as one block
            if trimmed.starts_with("```") {
                let mut src = vec![line.to_string()];
                let mut j = i + 1;
                while j < body.len() {
                    src.push(body[j].to_string());
                    if body[j].trim_start().starts_with("```") {
                        break;
                    }
                    j += 1;
                }
                let id = mint(&mut seq, &file_id, None);
                blocks.push(opaque_block(id, parent, "code", &src.join("\n")));
                i = j + 1;
                continue;
            }

            // --- Paragraph: gather consecutive plain lines.
            let mut para = vec![line.to_string()];
            let mut j = i + 1;
            while j < body.len() {
                let t = body[j].trim_start();
                if t.is_empty()
                    || heading_level(t).is_some()
                    || list_item_body(t).is_some()
                    || t.starts_with("> [!")
                    || t.starts_with("```")
                    || t.starts_with("![[")
                {
                    break;
                }
                para.push(body[j].to_string());
                j += 1;
            }
            let joined = para.join("\n");
            let (text, anchor) = split_block_anchor(joined.trim_end());
            let id = mint(&mut seq, &file_id, anchor);
            blocks.push(text_block(id, parent, &text));
            i = j;
        }

        Ok(FileFormatParseResult {
            document,
            blocks,
            blocks_needing_ids: Vec::new(),
            typed_rows: Vec::new(),
        })
    }

    fn render_document(&self, _d: &Block, _b: &[Block], path: &Path, _id: &EntityUri) -> String {
        panic!(
            "ObsidianMarkdownAdapter is read-only (Tier R/O): refused to render {} — a write to a \
             foreign vault is loss under ADR 0025 until anchored write-back lands",
            path.display()
        );
    }

    fn render_blocks(&self, _b: &[Block], path: &Path, _id: &EntityUri) -> String {
        panic!(
            "ObsidianMarkdownAdapter is read-only: refused to render {}",
            path.display()
        );
    }

    fn doc_id_from_content(&self, _content: &str) -> Option<String> {
        None // Obsidian identity is path/basename; no embedded stable id by
        // default.
    }

    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
        previous: Option<&Block>,
    ) -> StorageEntity {
        build_block_params(block, parent_id, document_uri, previous)
    }

    fn content_differs(&self, a: &Block, b: &Block) -> bool {
        a.content != b.content || a.marks != b.marks || a.tags != b.tags
    }

    fn sync_document_metadata(&self, _parsed: &Block, _persisted: &mut Block) -> bool {
        false
    }

    fn writeback_drops(
        &self,
        path: &Path,
        _source: &str,
        _rendered: &str,
        _sibling_renders: &[(&Path, &str)],
        _sanctioned_removals: &std::collections::HashSet<String>,
        _root: &Path,
    ) -> Result<holon_core::file_format::WritebackDropVerdict> {
        anyhow::bail!(
            "ObsidianMarkdownAdapter (Tier R/O) refuses write-back to foreign vault file {}",
            path.display()
        )
    }
}

fn heading_level(trimmed: &str) -> Option<usize> {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
        Some(hashes)
    } else {
        None
    }
}

fn list_item_body(trimmed: &str) -> Option<&str> {
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some(rest);
    }
    // ordered `1. `
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &trimmed[digits.len()..];
        if let Some(rest) = after.strip_prefix(". ") {
            return Some(rest);
        }
    }
    None
}

fn checkbox_state(body: &str) -> (String, Option<bool>) {
    if let Some(rest) = body.strip_prefix("[ ] ") {
        (rest.to_string(), Some(false))
    } else if let Some(rest) = body
        .strip_prefix("[x] ")
        .or_else(|| body.strip_prefix("[X] "))
    {
        (rest.to_string(), Some(true))
    } else {
        (body.to_string(), None)
    }
}

/// Encode a checkbox as a leading task marker so `text_block` lifts it into
/// `task_state` (unifying with LogSeq/org task handling).
fn prefix_task(task: Option<bool>, text: &str) -> String {
    match task {
        Some(false) => format!("TODO {text}"),
        Some(true) => format!("DONE {text}"),
        None => text.to_string(),
    }
}
