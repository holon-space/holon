//! `LogseqMarkdownAdapter` — Tier R/O ingest of LogSeq-flavored `.md`.
//!
//! LogSeq's outline-per-block model maps 1:1 onto the Holon block substrate:
//! every `-` bullet is a block, indentation is the tree, `key:: value` lines
//! are block/page properties, `((uuid))`/`[[Page]]`/`#tag` are marks, and
//! `TODO/DOING/…` + `SCHEDULED:`/`DEADLINE:` are task fields. `{{query}}` /
//! `{{embed}}` standalone bullets and `:LOGBOOK:` drawers become disclosed
//! opaque blocks (verbatim, never dropped).

use std::path::Path;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;

use crate::build::apply_planning;
use crate::build::opaque_block;
use crate::build::text_block;
use crate::params::build_block_params;

pub struct LogseqMarkdownAdapter;

impl LogseqMarkdownAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogseqMarkdownAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn indent_of(line: &str) -> usize {
    let mut n = 0;
    for c in line.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 2,
            _ => break,
        }
    }
    n
}

/// A property line `key:: value` (LogSeq). Key rules are permissive; we accept
/// an identifier-ish key followed by `::`.
fn parse_property(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let idx = t.find("::")?;
    let key = t[..idx].trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let val = t[idx + 2..].trim();
    Some((key.to_string(), val.to_string()))
}

fn doc_id_for(path: &Path, root: &Path, page_id: Option<&str>) -> EntityUri {
    match page_id {
        Some(id) => EntityUri::block(id),
        None => {
            let rel = path.strip_prefix(root).unwrap_or(path);
            EntityUri::file(&rel.to_string_lossy())
        }
    }
}

fn is_journal(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .and_then(|r| r.components().next().map(|c| c.as_os_str() == "journals"))
        .unwrap_or(false)
}

impl FileFormatAdapter for LogseqMarkdownAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["md", "markdown"]
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let lines: Vec<&str> = content.lines().collect();
        let mut idx = 0;

        // --- Page properties: leading `key:: value` lines before the first bullet.
        let mut page_props: Vec<(String, String)> = Vec::new();
        while idx < lines.len() {
            let l = lines[idx];
            if l.trim().is_empty() {
                idx += 1;
                continue;
            }
            if l.trim_start().starts_with("- ") || l.trim() == "-" {
                break;
            }
            if let Some(kv) = parse_property(l) {
                page_props.push(kv);
                idx += 1;
            } else {
                break;
            }
        }
        let page_id = page_props
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("id"))
            .map(|(_, v)| v.clone());
        let title = page_props
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("title"))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            });

        let file_id = doc_id_for(path, root, page_id.as_deref());
        let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), title);
        document.set_page(true);
        for (k, v) in &page_props {
            if k.eq_ignore_ascii_case("id") {
                continue;
            }
            if k.eq_ignore_ascii_case("tags") {
                for t in v.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    document.tags.insert(t.to_string());
                }
            } else {
                document.set_property(k.clone(), v.clone());
            }
        }
        if is_journal(path, root) {
            document.tags.insert("journal".to_string());
        }

        // --- Blocks: bullet outline with an indent stack.
        let mut blocks: Vec<Block> = Vec::new();
        let mut seq = 0usize;
        // stack of (indent, block_id) — parents of the current position.
        let mut stack: Vec<(usize, EntityUri)> = Vec::new();

        while idx < lines.len() {
            let line = lines[idx];
            if line.trim().is_empty() {
                idx += 1;
                continue;
            }
            let bullet_pos = line.find("- ");
            let is_bullet = line.trim_start().starts_with("- ") || line.trim() == "-";
            if !is_bullet {
                // Stray continuation with no owning bullet — preserve verbatim.
                idx += 1;
                continue;
            }
            let indent = indent_of(line);
            let first = if line.trim() == "-" {
                String::new()
            } else {
                line[bullet_pos.unwrap() + 2..].to_string()
            };

            // Gather continuation lines: deeper-indented, non-bullet lines.
            let mut cont: Vec<String> = Vec::new();
            let mut props: Vec<(String, String)> = Vec::new();
            let mut planning: Vec<String> = Vec::new();
            let mut j = idx + 1;
            let mut in_logbook = false;
            while j < lines.len() {
                let cl = lines[j];
                if cl.trim().is_empty() {
                    j += 1;
                    continue;
                }
                let cindent = indent_of(cl);
                let cbullet = cl.trim_start().starts_with("- ") || cl.trim() == "-";
                if cbullet && cindent <= indent {
                    break;
                }
                if cbullet {
                    // a deeper bullet — a child, stop gathering continuations.
                    break;
                }
                let ct = cl.trim();
                if in_logbook {
                    if ct == ":END:" {
                        in_logbook = false;
                    }
                    j += 1;
                    continue;
                }
                if ct == ":LOGBOOK:" {
                    in_logbook = true;
                    j += 1;
                    continue;
                }
                if ct.starts_with("SCHEDULED:") || ct.starts_with("DEADLINE:") {
                    planning.push(ct.to_string());
                } else if let Some(kv) = parse_property(cl) {
                    props.push(kv);
                } else {
                    cont.push(ct.to_string());
                }
                j += 1;
            }

            // Resolve parent from the indent stack.
            while let Some((si, _)) = stack.last() {
                if *si >= indent {
                    stack.pop();
                } else {
                    break;
                }
            }
            let parent = stack
                .last()
                .map(|(_, id)| id.clone())
                .unwrap_or_else(|| file_id.clone());

            // Block id: explicit `id::` property wins.
            let explicit_id = props
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("id"))
                .map(|(_, v)| v.clone());
            let block_id = match explicit_id {
                Some(id) => EntityUri::block(&id),
                None => EntityUri::block(&format!("{}::b::{}", file_id.id(), seq)),
            };

            let mut full = first.clone();
            for c in &cont {
                full.push('\n');
                full.push_str(c);
            }

            let mut block = if is_macro_only(&full) {
                let kind = macro_kind(&full);
                opaque_block(block_id.clone(), parent.clone(), kind, &full)
            } else {
                text_block(block_id.clone(), parent.clone(), &full)
            };
            for (k, v) in &props {
                if k.eq_ignore_ascii_case("id") {
                    continue;
                }
                if k.eq_ignore_ascii_case("collapsed") {
                    continue; // document state, not user content
                }
                block.set_property(k.clone(), v.clone());
            }
            for p in &planning {
                apply_planning(&mut block, p);
            }

            stack.push((indent, block_id.clone()));
            blocks.push(block);
            seq += 1;
            idx = j;
        }

        Ok(FileFormatParseResult {
            document,
            blocks,
            blocks_needing_ids: Vec::new(),
        })
    }

    fn render_document(&self, _d: &Block, _b: &[Block], path: &Path, _id: &EntityUri) -> String {
        panic!(
            "LogseqMarkdownAdapter is read-only (Tier R/O): refused to render {} — a write to a \
             foreign vault is loss under ADR 0025 until anchored write-back lands",
            path.display()
        );
    }

    fn render_blocks(&self, _b: &[Block], path: &Path, _id: &EntityUri) -> String {
        panic!(
            "LogseqMarkdownAdapter is read-only: refused to render {}",
            path.display()
        );
    }

    fn doc_id_from_content(&self, content: &str) -> Option<String> {
        for l in content.lines() {
            if l.trim_start().starts_with("- ") || l.trim() == "-" {
                break;
            }
            if let Some((k, v)) = parse_property(l) {
                if k.eq_ignore_ascii_case("id") {
                    return Some(v);
                }
            }
        }
        None
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
            "LogseqMarkdownAdapter (Tier R/O) refuses write-back to foreign vault file {}",
            path.display()
        )
    }
}

/// A bullet whose entire content is a single `{{...}}` macro.
fn is_macro_only(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("{{") && t.ends_with("}}") && !t[2..].contains("{{")
}

fn macro_kind(s: &str) -> &'static str {
    let t = s.trim().trim_start_matches("{{").trim();
    if t.starts_with("query") {
        "query"
    } else if t.starts_with("embed") {
        "embed"
    } else {
        "macro"
    }
}
