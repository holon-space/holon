use crate::models::{
    parse_header_args_from_str, Document, OrgBlockExt, OrgDocumentExt, SourceBlock,
    DEFAULT_ACTIVE_KEYWORDS, DEFAULT_DONE_KEYWORDS,
};
use anyhow::Result;
use chrono::Utc;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::{ContentType, SourceLanguage, TaskState};
use orgize::ast::{Headline, SourceBlock as OrgizeSourceBlock};
use orgize::rowan::ast::AstNode;
use orgize::{ParseConfig, SyntaxKind};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

/// Generate a directory ID from its path (ID is the relative path from root)
pub fn generate_directory_id(path: &Path, root_directory: &Path) -> String {
    path.strip_prefix(root_directory)
        .map(|rel_path| rel_path.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Generate a file URI from a file path relative to a root directory.
///
/// The root is canonicalized to handle symlinks (e.g., /var -> /private/var on macOS),
/// and the path is made relative to produce portable, sync-friendly URIs like:
/// - `file:index.org` for files in the root
/// - `file:projects/todo.org` for nested files
///
/// File URIs are transient identifiers used during parsing. They are resolved
/// to permanent `doc:<uuid>` URIs at startup via OrgSyncController.
pub fn generate_file_id(path: &Path, root: &Path) -> EntityUri {
    // Canonicalize both paths to handle symlinks consistently
    let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    // Make path relative to root
    let relative = canonical_path
        .strip_prefix(&canonical_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| canonical_path.to_string_lossy().to_string());

    EntityUri::file(&relative)
}

/// Generate a file URI from a path string (already relative to root).
pub fn generate_file_id_from_relative_path(relative_path: &str) -> EntityUri {
    EntityUri::file(relative_path)
}

/// Compute content hash for change detection
pub fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Result of parsing an org file
pub struct ParseResult {
    pub document: Document,
    pub blocks: Vec<Block>,
    /// Block IDs that need :ID: property added (for write-back)
    pub headlines_needing_ids: Vec<String>,
}

/// Parse TODO keywords from file content (#+TODO: or #+SEQ_TODO: lines)
fn parse_todo_keywords_config(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#+TODO:") || trimmed.starts_with("#+SEQ_TODO:") {
            let spec = trimmed
                .split_once(':')
                .map(|(_, rest)| rest.trim())
                .unwrap_or("");
            if !spec.is_empty() {
                return Some(spec.replace(" | ", "|").replace(' ', ","));
            }
        }
    }
    None
}

/// Parse #+TITLE: from file content
fn parse_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#+TITLE:") {
            return trimmed
                .split_once(':')
                .map(|(_, rest)| rest.trim().to_string());
        }
    }
    None
}

/// Parse an org file and return Document + Block entities
pub fn parse_org_file(
    path: &Path,
    content: &str,
    parent_dir_id: &EntityUri,
    _parent_depth: i64,
    root: &Path,
) -> Result<ParseResult> {
    let file_id = generate_file_id(path, root);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Parse file-level metadata
    let title = parse_title(content);
    let todo_keywords_raw = parse_todo_keywords_config(content);

    // Build TaskState array from raw config (or None if no config)
    let todo_task_states: Option<Vec<TaskState>> = todo_keywords_raw.as_ref().map(|kw| {
        let (active, done) = parse_keywords_from_config(kw);
        let mut states = Vec::new();
        for k in &active {
            states.push(TaskState::active(k));
        }
        for k in &done {
            states.push(TaskState::done(k));
        }
        states
    });

    // Create Document entity
    let mut document = Document::new(file_id.clone(), parent_dir_id.clone(), file_name);

    // Set org-specific properties using extension trait
    document.set_org_title(title);
    document.set_todo_keywords(todo_task_states);

    // Parse org content
    let org = if let Some(ref kw) = todo_keywords_raw {
        let (active, done) = parse_keywords_from_config(kw);
        let config = ParseConfig {
            todo_keywords: (active, done),
            ..Default::default()
        };
        config.parse(content)
    } else {
        let active: Vec<String> = DEFAULT_ACTIVE_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .collect();
        let done: Vec<String> = DEFAULT_DONE_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .collect();
        let config = ParseConfig {
            todo_keywords: (active, done),
            ..Default::default()
        };
        config.parse(content)
    };

    // Extract blocks (headlines)
    let mut blocks = Vec::new();
    let mut headlines_needing_ids = Vec::new();
    let mut sequence_counter = 0i64;

    // Extract done keywords for TaskState categorization
    let done_kws: Vec<String> = todo_keywords_raw
        .as_ref()
        .map(|kw| parse_keywords_from_config(kw).1)
        .unwrap_or_else(|| vec!["DONE".into(), "CANCELLED".into(), "CLOSED".into()]);

    // Process document headlines recursively
    let doc = org.document();
    process_headlines(
        doc.headlines(),
        file_id.as_str(), // Top-level headlines have document as parent
        &file_id,
        &mut sequence_counter,
        &mut blocks,
        &mut headlines_needing_ids,
        &done_kws,
    )?;

    Ok(ParseResult {
        document,
        blocks,
        headlines_needing_ids,
    })
}

/// Parse keywords config string "TODO,INPROGRESS|DONE,CANCELLED" into (Vec<String>, Vec<String>)
fn parse_keywords_from_config(config: &str) -> (Vec<String>, Vec<String>) {
    let parts: Vec<&str> = config.split('|').collect();
    let active = parts
        .first()
        .map(|s| s.split(',').map(|k| k.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["TODO".to_string()]);
    let done = parts
        .get(1)
        .map(|s| s.split(',').map(|k| k.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["DONE".to_string()]);
    (active, done)
}

/// Recursively process headlines and their children
fn process_headlines(
    headlines: impl Iterator<Item = Headline>,
    parent_id: &str,
    file_id: &EntityUri,
    sequence_counter: &mut i64,
    output: &mut Vec<Block>,
    needs_id: &mut Vec<String>,
    done_keywords: &[String],
) -> Result<()> {
    for headline in headlines {
        // Extract headline level (number of stars)
        let level = headline.level() as i64;

        // Assign sequence number
        let sequence = *sequence_counter;
        *sequence_counter += 1;

        // Extract :ID: property if exists
        let (id, needs_write) = extract_or_generate_id(&headline);
        if needs_write {
            needs_id.push(id.clone());
        }

        // Extract TODO keyword first, parsed into TaskState with category
        let task_state = headline
            .todo_keyword()
            .map(|t| TaskState::from_keyword_with_done_list(&t.to_string(), &done_keywords));

        // Extract title using title_raw() and remove TODO keyword if present
        let mut title = headline.title_raw().trim().to_string();
        if let Some(ref todo) = task_state {
            let kw = todo.keyword.as_str();
            if title.starts_with(kw) {
                title = title[kw.len()..].trim_start().to_string();
            }
        }

        // Extract priority (Token contains just the letter like "A")
        let priority = headline.priority().map(|t| {
            let letter = t.to_string();
            holon_api::Priority::from_letter(&letter).unwrap_or_else(|e| {
                panic!("org headline has invalid priority letter {letter:?}: {e}")
            })
        });

        // Extract tags
        let tags =
            holon_api::Tags::from_iter(headline.tags().map(|t| t.to_string()).collect::<Vec<_>>());

        // Extract section content with source blocks
        let section = extract_section_content(&headline);
        let body = section.body;
        let mut source_blocks = section.source_blocks;

        // Look for #+NAME: directives for each source block that doesn't have a name yet
        let source_blocks_for_name_lookup = source_blocks.clone();
        for source_block in &mut source_blocks {
            if source_block.name.is_none() {
                source_block.name =
                    find_block_name_in_section(&headline, &source_blocks_for_name_lookup);
            }
        }

        // Extract planning (SCHEDULED, DEADLINE).
        // Fall back to values extracted from paragraph text when orgize
        // misclassifies planning as PARAGRAPH (properties drawer before planning).
        let (scheduled, deadline) = {
            let (s, d) = extract_planning(&headline);
            (
                s.or(section.scheduled_fallback),
                d.or(section.deadline_fallback),
            )
        };

        // Extract properties as JSON
        let string_properties = extract_properties(&headline);

        // Create Block entity - content is title + body combined
        let content = if let Some(ref b) = body {
            format!("{}\n{}", title, b)
        } else {
            title.clone()
        };

        let now = Utc::now().timestamp_millis();
        let mut block = Block {
            id: EntityUri::from_raw(&id),
            parent_id: EntityUri::from_raw(parent_id),
            document_id: file_id.clone(),
            content,
            content_type: ContentType::Text,
            source_language: None,
            source_name: None,
            properties: HashMap::new(),
            created_at: now,
            updated_at: now,
        };

        // Set org-specific properties using extension trait
        block.set_level(level);
        block.set_sequence(sequence);
        block.set_task_state(task_state);
        block.set_priority(priority);
        block.set_tags(tags);
        block.set_scheduled(
            scheduled.and_then(|s| match holon_api::types::Timestamp::parse(&s) {
                Ok(ts) => Some(ts),
                Err(e) => {
                    tracing::warn!("Ignoring unparseable SCHEDULED timestamp {s:?}: {e}");
                    None
                }
            }),
        );
        block.set_deadline(
            deadline.and_then(|s| match holon_api::types::Timestamp::parse(&s) {
                Ok(ts) => Some(ts),
                Err(e) => {
                    tracing::warn!("Ignoring unparseable DEADLINE timestamp {s:?}: {e}");
                    None
                }
            }),
        );

        // Store drawer properties as flat keys in block properties
        for (key, value) in string_properties.iter() {
            block.set_property(key, holon_api::Value::String(value.to_string()));
        }
        // Store ID in properties (extract_properties filters it out since it's used for block.id)
        block.set_property("ID", holon_api::Value::String(id.clone()));

        output.push(block);

        // Create child Block entities for each source block
        for (src_index, mut source_block) in source_blocks.into_iter().enumerate() {
            // Extract :id from header args if present (preserves ID across round-trips)
            // Otherwise fall back to stable ID based on parent + index
            let src_id = source_block
                .header_args
                .remove("id")
                .and_then(|v| v.as_string().map(|s| s.to_string()))
                .unwrap_or_else(|| format!("{}::src::{}", id, src_index));

            let src_sequence = *sequence_counter;
            *sequence_counter += 1;

            let mut src_block = Block {
                id: EntityUri::block(&src_id),
                parent_id: EntityUri::from_raw(&id),
                document_id: file_id.clone(),
                content: source_block.source,
                content_type: ContentType::Source,
                source_language: source_block
                    .language
                    .map(|l| l.parse::<SourceLanguage>().unwrap()),
                source_name: source_block.name,
                properties: HashMap::new(),
                created_at: now,
                updated_at: now,
            };
            src_block.set_sequence(src_sequence);

            // Separate standard org header args from custom properties.
            // Standard args (results, session, connection, var, etc.) go into
            // _source_header_args. Everything else is a custom property stored
            // directly in block.properties for round-trip fidelity.
            if !source_block.header_args.is_empty() {
                const KNOWN_HEADER_ARGS: &[&str] = &[
                    "results",
                    "session",
                    "connection",
                    "var",
                    "tangle",
                    "noweb",
                    "exports",
                    "cache",
                    "dir",
                    "eval",
                    "file",
                    "hlines",
                    "colnames",
                    "rownames",
                    "sep",
                    "mkdirp",
                    "padline",
                    "shebang",
                    "wrap",
                    "post",
                    "prologue",
                    "epilogue",
                ];
                let mut standard_args = HashMap::new();
                for (k, v) in source_block.header_args {
                    if KNOWN_HEADER_ARGS.contains(&k.as_str()) {
                        standard_args.insert(k, v);
                    } else if let Some(s) = v.as_string() {
                        src_block.set_property(&k, holon_api::Value::String(s.to_string()));
                    }
                }
                if !standard_args.is_empty() {
                    src_block.set_source_header_args(standard_args);
                }
            }

            output.push(src_block);
        }

        // Recursively process children
        process_headlines(
            headline.headlines(),
            &id,
            file_id,
            sequence_counter,
            output,
            needs_id,
            done_keywords,
        )?;
    }

    Ok(())
}

/// Extract :ID: property from headline, or generate a new UUID
/// Returns (id, needs_write_back)
fn extract_or_generate_id(headline: &Headline) -> (String, bool) {
    if let Some(drawer) = headline.properties() {
        if let Some(id_token) = drawer.get("ID") {
            let value = id_token.to_string().trim().to_string();
            if !value.is_empty() {
                return (value, false);
            }
        }
    }
    (Uuid::new_v4().to_string(), true)
}

/// Extract SCHEDULED and DEADLINE timestamps from headline
fn extract_planning(headline: &Headline) -> (Option<String>, Option<String>) {
    let mut scheduled = None;
    let mut deadline = None;

    if let Some(planning) = headline.planning() {
        if let Some(s) = planning.scheduled() {
            scheduled = Some(s.syntax().to_string());
        }
        if let Some(d) = planning.deadline() {
            deadline = Some(d.syntax().to_string());
        }
    }

    (scheduled, deadline)
}

/// Extract custom properties from the property drawer (excludes :ID:).
fn extract_properties(headline: &Headline) -> HashMap<String, String> {
    let drawer = match headline.properties() {
        Some(d) => d,
        None => return HashMap::new(),
    };

    drawer
        .iter()
        .filter_map(|(key_token, value_token)| {
            let key = key_token.to_string().trim().to_string();
            if key.eq_ignore_ascii_case("ID") {
                return None;
            }
            let value = value_token.to_string().trim().to_string();
            Some((key, value))
        })
        .collect()
}

/// Extract source blocks from a headline's section.
/// Returns (plain_text_content, source_blocks)
struct SectionContent {
    body: Option<String>,
    source_blocks: Vec<SourceBlock>,
    /// SCHEDULED extracted from paragraph text (fallback when orgize misclassifies)
    scheduled_fallback: Option<String>,
    /// DEADLINE extracted from paragraph text (fallback when orgize misclassifies)
    deadline_fallback: Option<String>,
}

fn extract_section_content(headline: &Headline) -> SectionContent {
    let section = match headline.section() {
        Some(s) => s,
        None => {
            return SectionContent {
                body: None,
                source_blocks: Vec::new(),
                scheduled_fallback: None,
                deadline_fallback: None,
            }
        }
    };

    let section_syntax = section.syntax();
    let section_text = section_syntax.to_string();
    let mut source_blocks = Vec::new();
    let mut scheduled_fallback: Option<String> = None;
    let mut deadline_fallback: Option<String> = None;

    let mut pending_name: Option<String> = None;

    for child in section_syntax.children() {
        if child.kind() == SyntaxKind::KEYWORD {
            let keyword_text = child.text().to_string();
            let trimmed = keyword_text.trim();
            if trimmed.starts_with("#+NAME:") || trimmed.starts_with("#+name:") {
                if let Some((_, name)) = trimmed.split_once(':') {
                    pending_name = Some(name.trim().to_string());
                }
                continue;
            }
        }

        if child.kind() == SyntaxKind::SOURCE_BLOCK {
            if let Some(src_block) = OrgizeSourceBlock::cast(child.clone()) {
                let language = src_block
                    .language()
                    .map(|t| t.to_string().trim().to_string());
                let source = src_block.value();
                let parameters = src_block.parameters().map(|t| t.to_string());

                let mut source_block =
                    SourceBlock::new(language.clone().unwrap_or_default(), source);

                // Check for #+NAME: in the block text (orgize includes it in SOURCE_BLOCK)
                let block_text = child.text().to_string();
                if let Some(name) = extract_name_from_block_text(&block_text) {
                    source_block.name = Some(name);
                } else if let Some(name) = pending_name.take() {
                    source_block.name = Some(name);
                }

                if let Some(params) = parameters {
                    let header_args_str = parse_header_args_from_str(&params);
                    for (k, v) in header_args_str {
                        source_block
                            .header_args
                            .insert(k, holon_api::Value::String(v));
                    }
                }

                source_blocks.push(source_block);
                pending_name = None;
            }
        } else if !child.text().to_string().trim().is_empty() {
            pending_name = None;
        }
    }

    // Extract SCHEDULED/DEADLINE fallback from non-planning text (orgize
    // misclassifies them as PARAGRAPH when properties drawer precedes planning).
    for child in section_syntax.children() {
        match child.kind() {
            SyntaxKind::SOURCE_BLOCK
            | SyntaxKind::KEYWORD
            | SyntaxKind::PROPERTY_DRAWER
            | SyntaxKind::PLANNING => {}
            _ => {
                let child_text = child.text().to_string();
                for line in child_text.lines() {
                    let t = line.trim();
                    if t.starts_with("SCHEDULED:") {
                        scheduled_fallback =
                            Some(t.trim_start_matches("SCHEDULED:").trim().to_string());
                    } else if t.starts_with("DEADLINE:") {
                        deadline_fallback =
                            Some(t.trim_start_matches("DEADLINE:").trim().to_string());
                    }
                }
            }
        }
    }

    // Extract body text by removing non-body nodes from the full section text.
    // This preserves original spacing (blank lines, lists, etc.) instead of
    // reassembling from individual child nodes which would lose inter-node spacing.
    let section_start = usize::from(section_syntax.text_range().start());
    let mut ranges_to_remove: Vec<(usize, usize)> = Vec::new();
    for child in section_syntax.children() {
        match child.kind() {
            SyntaxKind::SOURCE_BLOCK
            | SyntaxKind::KEYWORD
            | SyntaxKind::PROPERTY_DRAWER
            | SyntaxKind::PLANNING => {
                let range = child.text_range();
                let start = usize::from(range.start()) - section_start;
                let end = usize::from(range.end()) - section_start;
                ranges_to_remove.push((start, end));
            }
            _ => {}
        }
    }

    // Build body text by taking only the non-removed ranges
    let mut body_text = String::new();
    let mut pos = 0usize;
    ranges_to_remove.sort_by_key(|r| r.0);
    for (start, end) in &ranges_to_remove {
        if pos < *start {
            body_text.push_str(&section_text[pos..*start]);
        }
        pos = *end;
    }
    if pos < section_text.len() {
        body_text.push_str(&section_text[pos..]);
    }

    let body_text = strip_planning_lines(&body_text);
    let trimmed = body_text.trim();
    let plain_text = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };

    SectionContent {
        body: plain_text,
        source_blocks,
        scheduled_fallback,
        deadline_fallback,
    }
}

/// Strip SCHEDULED:/DEADLINE: lines from text.
///
/// When the properties drawer precedes planning (our render order), orgize
/// misclassifies the planning lines as a PARAGRAPH. We strip them here since
/// planning is already extracted separately via `extract_planning`.
fn strip_planning_lines(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let t = line.trim();
            !t.starts_with("SCHEDULED:") && !t.starts_with("DEADLINE:")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract #+NAME: from block text (orgize includes it in SOURCE_BLOCK node)
fn extract_name_from_block_text(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#+NAME:") || trimmed.starts_with("#+name:") {
            if let Some((_, name)) = trimmed.split_once(':') {
                return Some(name.trim().to_string());
            }
        }
        // Stop looking once we hit BEGIN_SRC
        if trimmed.starts_with("#+BEGIN_SRC") || trimmed.starts_with("#+begin_src") {
            break;
        }
    }
    None
}

/// Look for #+NAME: directive for a source block in the section
fn find_block_name_in_section(
    _headline: &Headline,
    _source_blocks: &[SourceBlock],
) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_simple_headlines() {
        let content = "* First headline\n** Nested headline\n* Second headline";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        assert_eq!(result.blocks.len(), 3);
        assert_eq!(result.blocks[0].org_title(), "First headline");
        assert_eq!(result.blocks[1].org_title(), "Nested headline");
        assert_eq!(result.blocks[1].parent_id, result.blocks[0].id);
        assert_eq!(result.blocks[2].org_title(), "Second headline");
    }

    #[test]
    fn test_parse_todo_and_priority() {
        let content = "* TODO [#A] Important task :work:urgent:";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        assert_eq!(result.blocks.len(), 1);
        let h = &result.blocks[0];
        assert_eq!(h.task_state(), Some(TaskState::active("TODO")));
        assert_eq!(h.priority(), Some(holon_api::Priority::High));
        assert_eq!(h.tags(), holon_api::Tags::from_csv("work,urgent"));
    }

    #[test]
    fn test_default_keywords_without_todo_config() {
        // Files without #+TODO: should still recognize DOING from DEFAULT_ACTIVE_KEYWORDS
        let content = "* DOING Work in progress\n* DONE Finished task\n* CANCELLED Dropped task";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        assert_eq!(result.blocks.len(), 3);
        assert_eq!(
            result.blocks[0].task_state(),
            Some(TaskState::active("DOING"))
        );
        assert_eq!(result.blocks[0].org_title(), "Work in progress");
        assert_eq!(result.blocks[1].task_state(), Some(TaskState::done("DONE")));
        assert_eq!(result.blocks[1].org_title(), "Finished task");
        assert_eq!(
            result.blocks[2].task_state(),
            Some(TaskState::done("CANCELLED"))
        );
        assert_eq!(result.blocks[2].org_title(), "Dropped task");
    }

    #[test]
    fn test_parse_title_and_todo_keywords() {
        let content = "#+TITLE: My Document\n#+TODO: TODO INPROGRESS | DONE CANCELLED\n* Task";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        assert_eq!(result.document.org_title(), Some("My Document".to_string()));
        let kws = result.document.todo_keywords().unwrap();
        let active: Vec<&str> = kws
            .iter()
            .filter(|s| s.is_active())
            .map(|s| s.keyword.as_str())
            .collect();
        let done: Vec<&str> = kws
            .iter()
            .filter(|s| s.is_done())
            .map(|s| s.keyword.as_str())
            .collect();
        assert_eq!(active, vec!["TODO", "INPROGRESS"]);
        assert_eq!(done, vec!["DONE", "CANCELLED"]);
    }

    #[test]
    fn test_generate_ids() {
        let root = Path::new("/path/to");
        let path1 = Path::new("/path/to/file1.org");
        let path2 = Path::new("/path/to/file2.org");

        let id1 = generate_file_id(path1, root);
        let id2 = generate_file_id(path2, root);

        assert_ne!(id1, id2);
        assert!(id1.is_file());
        // Should be relative paths with file: scheme
        assert_eq!(id1.as_str(), "file:file1.org");
        assert_eq!(id2.as_str(), "file:file2.org");

        let id1_again = generate_file_id(path1, root);
        assert_eq!(id1, id1_again);
    }

    #[test]
    fn test_parse_existing_id_property() {
        let content = "* Headline\n:PROPERTIES:\n:ID: existing-uuid-here\n:END:";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id.id(), "existing-uuid-here");
        assert!(result.headlines_needing_ids.is_empty());
    }

    #[test]
    fn test_headlines_without_id_need_writeback() {
        let content = "* Headline without ID";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        assert_eq!(result.blocks.len(), 1);
        assert!(!result.headlines_needing_ids.is_empty());
    }

    #[test]
    fn test_parse_source_block_basic() {
        let content = r#"* Headline with code
#+BEGIN_SRC python
def hello():
    print("Hello, world!")
#+END_SRC
"#;
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        // Should have 2 blocks: headline + source block
        assert_eq!(result.blocks.len(), 2);

        let headline = &result.blocks[0];
        assert_eq!(headline.content_type, ContentType::Text);

        // Source block is a separate child block
        let source_block = &result.blocks[1];
        assert_eq!(source_block.content_type, ContentType::Source);
        assert_eq!(source_block.parent_id, headline.id);
        assert_eq!(
            source_block.source_language,
            Some("python".parse::<SourceLanguage>().unwrap())
        );
        assert!(source_block.content.contains("def hello():"));
        assert!(source_block.content.contains("print(\"Hello, world!\")"));
    }

    #[test]
    fn test_parse_source_block_with_header_args() {
        let content = r#"* Headline with PRQL
#+BEGIN_SRC holon_prql :connection main :results table
from tasks
filter completed == false
#+END_SRC
"#;
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        // Should have 2 blocks: headline + source block
        assert_eq!(result.blocks.len(), 2);

        let source_block = &result.blocks[1];
        assert_eq!(source_block.content_type, ContentType::Source);
        assert_eq!(
            source_block.source_language,
            Some("holon_prql".parse::<SourceLanguage>().unwrap())
        );
        assert!(source_block.is_prql_block());

        // Parse header args from JSON
        let header_args = source_block.get_source_header_args();
        assert_eq!(
            header_args.get("connection"),
            Some(&holon_api::Value::String("main".to_string()))
        );
        assert_eq!(
            header_args.get("results"),
            Some(&holon_api::Value::String("table".to_string()))
        );
    }

    #[test]
    fn test_parse_multiple_source_blocks() {
        let content = r#"* Multiple blocks
Some intro text.

#+BEGIN_SRC holon_sql
SELECT * FROM users;
#+END_SRC

Middle text.

#+BEGIN_SRC holon_prql
from users | take 10
#+END_SRC

Outro text.
"#;
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        // Should have 3 blocks: headline + 2 source blocks
        assert_eq!(result.blocks.len(), 3);

        let headline = &result.blocks[0];
        assert_eq!(headline.content_type, ContentType::Text);

        // First source block (sql)
        let sql_block = &result.blocks[1];
        assert_eq!(sql_block.content_type, ContentType::Source);
        assert_eq!(
            sql_block.source_language,
            Some("holon_sql".parse::<SourceLanguage>().unwrap())
        );
        assert_eq!(sql_block.parent_id, headline.id);

        // Second source block (prql)
        let prql_block = &result.blocks[2];
        assert_eq!(prql_block.content_type, ContentType::Source);
        assert_eq!(
            prql_block.source_language,
            Some("holon_prql".parse::<SourceLanguage>().unwrap())
        );
        assert_eq!(prql_block.parent_id, headline.id);

        // Text content should be preserved in headline
        assert!(headline.body().is_some());
    }

    #[test]
    fn test_parse_named_source_block() {
        let content = r#"* Named block
#+NAME: my-query
#+BEGIN_SRC holon_prql
from tasks
#+END_SRC
"#;
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        // Should have 2 blocks: headline + source block
        assert_eq!(result.blocks.len(), 2);

        let source_block = &result.blocks[1];
        assert_eq!(source_block.content_type, ContentType::Source);
        assert_eq!(source_block.source_name, Some("my-query".to_string()));
    }

    #[test]
    fn test_parse_header_args() {
        let params = ":var x=1 :results table :tangle yes";
        let args = parse_header_args_from_str(params);

        assert_eq!(args.get("var"), Some(&"x=1".to_string()));
        assert_eq!(args.get("results"), Some(&"table".to_string()));
        assert_eq!(args.get("tangle"), Some(&"yes".to_string()));
    }

    #[test]
    fn test_parse_header_args_flags_only() {
        let params = ":noweb :tangle";
        let args = parse_header_args_from_str(params);

        assert_eq!(args.get("noweb"), Some(&"".to_string()));
        assert_eq!(args.get("tangle"), Some(&"".to_string()));
    }

    #[test]
    fn test_prql_blocks_filter() {
        let content = r#"* Mixed blocks
#+BEGIN_SRC holon_sql
SELECT 1;
#+END_SRC

#+BEGIN_SRC holon_prql
from users
#+END_SRC

#+BEGIN_SRC python
print("hello")
#+END_SRC
"#;
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        // Should have 4 blocks: headline + 3 source blocks
        assert_eq!(result.blocks.len(), 4);

        // Filter to find PRQL blocks
        let prql_blocks: Vec<_> = result.blocks.iter().filter(|b| b.is_prql_block()).collect();

        assert_eq!(prql_blocks.len(), 1);
        assert!(prql_blocks[0].content.contains("from users"));
    }

    #[test]
    fn test_parse_real_index_org() {
        let content = r#"* Today's Tasks
:PROPERTIES:
:ID: 39471ed2-64b6-4b98-9782-30c6caf8f061
:VIEW: query
:END:

#+BEGIN_SRC holon_prql
from blocks
select {id, parent_id, content, content_type}
#+END_SRC
"#;
        let path = PathBuf::from("/test/index.org");
        let root = PathBuf::from("/test");
        let result = parse_org_file(&path, content, &EntityUri::doc_root(), 0, &root).unwrap();

        // Should have 2 blocks: headline + source block
        assert_eq!(result.blocks.len(), 2, "Expected 2 blocks");

        let headline = &result.blocks[0];
        assert_eq!(headline.content_type, ContentType::Text);
        assert!(headline.org_title().contains("Today's Tasks"));

        let source = &result.blocks[1];
        assert_eq!(source.content_type, ContentType::Source);
        assert_eq!(
            source.source_language,
            Some("holon_prql".parse::<SourceLanguage>().unwrap())
        );
        assert!(source.content.contains("from blocks"));
        assert_eq!(source.parent_id, headline.id);

        println!("\n=== Parse Test Results ===");
        println!("Headline ID: {}", headline.id);
        println!("Headline content_type: {}", headline.content_type);
        println!("Source block ID: {}", source.id);
        println!("Source block content_type: {}", source.content_type);
        println!("Source block parent_id: {}", source.parent_id);
        println!("Source block language: {:?}", source.source_language);
        println!("Source block content:\n{}", source.content);
    }
}
