use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::ContentType;
use holon_api::types::SourceLanguage;
use holon_api::types::Tags;
use holon_api::types::TaskState;
use orgize::ParseConfig;
use orgize::SyntaxKind;
use orgize::ast::Headline;
use orgize::ast::Section;
use orgize::ast::SourceBlock as OrgizeSourceBlock;
use orgize::rowan::ast::AstNode;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

use crate::models::DEFAULT_ACTIVE_KEYWORDS;
use crate::models::DEFAULT_DONE_KEYWORDS;
use crate::models::OrgBlockExt;
use crate::models::OrgDocumentExt;
use crate::models::SourceBlock;
use crate::models::parse_header_args_from_str;

/// Generate a file URI from a file path relative to a root directory.
///
/// The root is canonicalized to handle symlinks (e.g., /var -> /private/var on
/// macOS), and the path is made relative to produce portable, sync-friendly
/// URIs like:
/// - `file:index.org` for files in the root
/// - `file:projects/todo.org` for nested files
///
/// File URIs are transient identifiers used during parsing. They are resolved
/// to the page's permanent `block:<uuid>` URI at startup via
/// FileSyncController. Generate a file URI from a file path relative to a root
/// directory.
///
/// Both `path` and `root` must already be canonicalized by the caller when
/// symlink resolution is needed (e.g. macOS /var → /private/var).
/// This function is pure and does not touch the filesystem.
pub fn generate_file_id(path: &Path, root: &Path) -> EntityUri {
    let relative = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string());

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
    pub document: Block,
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

/// Parse `#+ID: <bare-id>` from file content. The bare id is wrapped into
/// `block:<id>` at the boundary by callers — the file format stores bare ids
/// per the org syntax convention. Returns None when no directive is present.
pub fn parse_doc_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#+ID:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Parse an org file and return Document + Block entities
pub fn parse_org_file(
    path: &Path,
    content: &str,
    parent_dir_id: &EntityUri,
    root: &Path,
) -> Result<ParseResult> {
    // Use the file stem (no extension) as the page title. The reference model
    // and PBT downstream consumers all normalize on stem.
    // ALLOW(fallback): file stem is a deterministic title source, not a
    // failure-mode shim
    let file_name = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Parse file-level metadata
    let title = parse_title(content);
    let todo_keywords_raw = parse_todo_keywords_config(content);

    // `#+ID: <bare>` (when present) overrides the path-derived `file:` identity
    // with a stable `block:<bare>` URI, so renames don't change document
    // identity. The resolved `file_id` is used both as the document's id and
    // as the parent for top-level headlines.
    let file_id = match parse_doc_id(content) {
        Some(bare) => EntityUri::block(&bare),
        None => generate_file_id(path, root),
    };

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

    // Create document block. The first line of content is the title; the
    // `Page` tag marks it as a page (formerly the `name`-bearing variant).
    let title_line = title.clone().unwrap_or(file_name);
    let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), title_line);
    document.set_page(true);

    // Set org-specific properties using extension trait
    document.set_file_title(title);
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

    // Top-level (pre-first-headline) source/image children of the document —
    // emitted FIRST so they precede headline blocks in document order. A page
    // whose direct child is a source block (row-28: `convert_block_to_page` on a
    // block owning a `holon_rule`) renders that child as a top-level
    // `#+BEGIN_SRC` under the file `#+ID:` header; `process_headlines` only
    // walks headlines, so without this pass the block is dropped on round-trip.
    let top_section = extract_section_content(doc.section());
    // The pre-first-headline body belongs to the doc-root, stored exactly as a
    // headline stores its own: `title\nbody`. Dropping it here is what let the
    // renderer delete it from disk on every write-back.
    if let Some(body) = top_section
        .body
        .as_deref()
        .map(crate::models::trim_blank_lines)
        .filter(|b| !b.is_empty())
    {
        document.content = format!("{}\n{}", document.content, body);
    }
    emit_section_children(
        top_section.source_blocks,
        top_section.image_paths,
        file_id.id(),
        &mut sequence_counter,
        &mut blocks,
    );

    process_headlines(
        doc.headlines(),
        file_id.as_str(), // Top-level headlines have document as parent
        &file_id,
        &mut sequence_counter,
        &mut blocks,
        &mut headlines_needing_ids,
        &done_kws,
    )?;

    // Parse boundary (F8, dogfood 2026-07-21): a single file must project to a
    // valid FOREST. Every block id must be distinct from the document id and
    // from every other block id in the file. A duplicate id makes a block its
    // own ancestor -- the classic case is a doc `#+ID:` equal to a heading `:ID:`,
    // which sets `block.parent_id == block.id` (a self-parent 1-cycle); two
    // headings sharing an `:ID:` likewise tie one identity to two nodes.
    // Downstream the recursive `focus_descendants` tree projection walks
    // `child.parent_id = fd.id` with no cycle guard, so a self-parent recurses
    // without bound -- boot stack-overflow that kills the whole app. Parse,
    // don't validate: reject the malformed file HERE with an enriched error so
    // the ingest boundary quarantines it (loud + disclosed, other files keep
    // syncing) instead of writing the cyclic row the projection then crashes on.
    reject_id_cycles(path, &document, &blocks)?;

    // Sibling order is conveyed positionally — blocks are pushed in document
    // DFS order, so the org sync controller derives each block's
    // `after_block_id` from that order and the order owner mints the
    // `sort_key` on create (`place`/`new_child_anchor` for SqlOnly, the Loro
    // fractional index projected to SQL for Loro mode). The parser must NOT
    // mint keys: a second key generator here (`gen_n_keys`) lived in a
    // different value space than the owner's, so the two disagreed on order.
    Ok(ParseResult {
        document,
        blocks,
        headlines_needing_ids,
    })
}

/// Parse-boundary forest check (F8, dogfood 2026-07-21): reject a file whose
/// parsed blocks do not form a valid forest under the document root. A
/// duplicate id -- a heading `:ID:` equal to the doc `#+ID:`, or to another
/// heading's `:ID:` -- makes a block its own ancestor; the recursive
/// `focus_descendants` projection then recurses without bound and crashes the
/// app on boot. Fail loud here so the offending file is quarantined at ingest,
/// never written to the store where the projection would overflow the stack.
fn reject_id_cycles(path: &Path, document: &Block, blocks: &[Block]) -> Result<()> {
    let mut owners: HashMap<&str, &'static str> = HashMap::new();
    owners.insert(document.id.as_str(), "the document root (#+ID:)");
    for block in blocks {
        if let Some(prev) = owners.insert(block.id.as_str(), "a heading/block (:ID:)") {
            anyhow::bail!(
                "org id collision in {}: id {:?} is claimed by both {} and a heading/block -- \
                 duplicate ids make a block its own ancestor (self-parent cycle), which recurses \
                 the tree projection without bound and crashes the app on boot. Give the colliding \
                 heading a distinct :ID: (or drop the file's #+ID:).",
                path.display(),
                block.id.as_str(),
                prev,
            );
        }
        // Direct 1-cycle backstop: any block naming itself as parent, however
        // its id was assigned.
        if block.id == block.parent_id {
            anyhow::bail!(
                "org self-parent in {}: block {:?} lists itself as its own parent -- a 1-cycle \
                 that recurses the tree projection without bound and crashes the app on boot.",
                path.display(),
                block.id.as_str(),
            );
        }
    }
    Ok(())
}

/// Split a headline TITLE LINE into (title, tags) exactly the way the org
/// parser does: a trailing `:tag1:tag2:` group is org TAG syntax, not title
/// text (org has no escape for it). Round-trip normalization mirrors this
/// parse boundary so reference models agree with the parser.
///
/// Runs the same orgize grammar the real parser uses (no TODO keywords, so
/// leading keywords stay in the title).
pub fn split_headline_tags(line: &str) -> (String, Vec<String>) {
    assert!(
        !line.contains('\n'),
        "split_headline_tags takes a single title line, got {line:?}"
    );
    let config = ParseConfig {
        todo_keywords: (vec![], vec![]),
        ..Default::default()
    };
    let org = config.parse(format!("* {line}"));
    let headline = org
        .document()
        .first_headline()
        .unwrap_or_else(|| panic!("synthetic headline must parse: {line:?}"));
    let title = headline.title_raw().trim().to_string();
    let tags = headline.tags().map(|t| t.to_string()).collect();
    (title, tags)
}

/// Parse keywords config string "TODO,INPROGRESS|DONE,CANCELLED" into
/// (Vec<String>, Vec<String>)
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

/// Emit a section's source-block and image children as `Block`s parented to
/// `parent_bare` (the bare id of the owning headline OR the document root for
/// top-level, pre-first-headline content). Shared by `process_headlines` and
/// the document top-level pass in `parse_org_file` so a source/image block
/// round-trips identically whether it sits under a `* headline` or directly
/// under the file `#+ID:` header (row-28: a `convert_block_to_page` page whose
/// direct child is a `holon_rule` renders that child as a top-level
/// `#+BEGIN_SRC`; without this pass the parser dropped it silently).
fn emit_section_children(
    source_blocks: Vec<SourceBlock>,
    image_paths: Vec<String>,
    parent_bare: &str,
    sequence_counter: &mut i64,
    output: &mut Vec<Block>,
) {
    let now = holon_api::clock::now_millis();
    // Create child Block entities for each source block
    for (src_index, mut source_block) in source_blocks.into_iter().enumerate() {
        // Extract :id from header args if present (preserves ID across round-trips)
        // Otherwise fall back to stable ID based on parent + index
        let src_id = source_block
            .header_args
            .remove("id")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
            .unwrap_or_else(|| format!("{}::src::{}", parent_bare, src_index));

        let src_sequence = *sequence_counter;
        *sequence_counter += 1;

        let mut src_block = Block {
            id: EntityUri::block(&src_id),
            // ALLOW(entity_uri_from_raw): org parser output: parent headline raw org slug
            parent_id: EntityUri::from_raw(parent_bare),
            content: source_block.source,
            content_type: ContentType::Source,
            source_language: source_block
                .language
                .map(|l| l.parse::<SourceLanguage>().unwrap()),
            source_name: source_block.name,
            created_at: now,
            updated_at: now,
            ..Block::default()
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
                } else if k.eq_ignore_ascii_case("REQUIRES") || k.eq_ignore_ascii_case("BLOCKED-BY")
                {
                    // `:BLOCKED-BY <bare>` (canonical) / `:REQUIRES <bare>`
                    // (legacy alias) is an edge-typed header arg emitted by
                    // `source_block_to_org` via `drawer_properties()`. UNION
                    // both spellings into the typed `block.requires` edge field
                    // (the `block_requires` junction) so it round-trips as an
                    // edge, symmetric with the headline path above.
                    if let Some(s) = v.as_string() {
                        for slug in s
                            .split(|c: char| c == ',' || c.is_whitespace())
                            .filter(|s| !s.is_empty())
                        {
                            // ALLOW(entity_uri_from_raw): org src-block REQUIRES/BLOCKED-BY
                            // header arg bare slug at parse boundary
                            let uri = EntityUri::from_raw(slug);
                            if !src_block.requires.contains(&uri) {
                                src_block.requires.push(uri);
                            }
                        }
                    }
                } else if k.eq_ignore_ascii_case("ADVICE_SUPPRESSED") {
                    if let Some(s) = v.as_string() {
                        src_block.advice_suppressed = s
                            .split(|c: char| c == ',' || c.is_whitespace())
                            .filter(|s| !s.is_empty())
                            // ALLOW(entity_uri_from_raw): org src-block ADVICE_SUPPRESSED
                            // header arg: bare slug promoted at parse boundary
                            .map(EntityUri::from_raw)
                            .collect();
                    }
                } else if k.eq_ignore_ascii_case("TAGS") {
                    // `:TAGS <space-joined>` is emitted by `source_block_to_org`
                    // because a Source block has no headline to carry `:tag:`
                    // notation. Lift it back into the typed `block.tags` set so
                    // tags survive the org round-trip on rule/source blocks.
                    if let Some(s) = v.as_string() {
                        src_block.tags = Tags::from_tag_iter(
                            s.split(|c: char| c == ',' || c.is_whitespace())
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string()),
                        );
                    }
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

    // Create child Block entities for each image link
    for (img_index, image_path) in image_paths.into_iter().enumerate() {
        let img_id = format!("{}::img::{}", parent_bare, img_index);
        let img_sequence = *sequence_counter;
        *sequence_counter += 1;

        let mut img_block = Block::new_image(
            EntityUri::block(&img_id),
            // ALLOW(entity_uri_from_raw): org parser output: parent headline raw org slug
            EntityUri::from_raw(parent_bare),
            image_path,
        );
        img_block.set_sequence(img_sequence);
        img_block.created_at = now;
        img_block.updated_at = now;
        output.push(img_block);
    }
}

/// Recursively process headlines and their children
#[allow(clippy::only_used_in_recursion)] // file_id threaded for future log/diagnostic plumbing
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
            .map(|t| TaskState::from_keyword_with_done_list(t.as_ref(), done_keywords));

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
        let tags = holon_api::Tags::from_tag_iter(
            headline.tags().map(|t| t.to_string()).collect::<Vec<_>>(),
        );

        // Extract section content with source blocks
        let section = extract_section_content(headline.section());
        let body = section.body;
        let source_blocks = section.source_blocks;

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

        // Parse planning timestamps up front. The raw planning line has already
        // been stripped from the body, so an unparseable timestamp must be
        // preserved as a literal body line — dropping it would silently delete
        // the user's SCHEDULED/DEADLINE line on the next write-back.
        let mut preserved_planning: Vec<String> = Vec::new();
        let scheduled = scheduled.and_then(|s| match holon_api::types::Timestamp::parse(&s) {
            Ok(ts) => Some(ts),
            Err(e) => {
                tracing::warn!("Unparseable SCHEDULED timestamp {s:?} preserved in body: {e}");
                preserved_planning.push(format!("SCHEDULED: {s}"));
                None
            }
        });
        let deadline = deadline.and_then(|s| match holon_api::types::Timestamp::parse(&s) {
            Ok(ts) => Some(ts),
            Err(e) => {
                tracing::warn!("Unparseable DEADLINE timestamp {s:?} preserved in body: {e}");
                preserved_planning.push(format!("DEADLINE: {s}"));
                None
            }
        });
        let body = if preserved_planning.is_empty() {
            body
        } else {
            let mut merged = preserved_planning.join("\n");
            if let Some(b) = body {
                merged.push('\n');
                merged.push_str(&b);
            }
            Some(merged)
        };

        // Extract properties as JSON
        let string_properties = extract_properties(&headline);

        // Create Block entity - content is title + body combined
        let raw_content = if let Some(ref b) = body {
            format!("{}\n{}", title, b)
        } else {
            title.clone()
        };

        // Extract inline marks (Bold/Italic/Link/etc) from the raw org text.
        // When marks are present, we store the rendered (delimiter-stripped)
        // text in `block.content` and the spans in `block.marks`. When the
        // paragraph contains no inline markup, both `extract_inline_marks`
        // returns empty marks and we keep the raw content byte-identical to
        // preserve today's behavior for non-rich blocks.
        let (content, marks) = {
            let (rendered, spans) = crate::inline_marks::extract_inline_marks(&raw_content);
            if spans.is_empty() {
                (raw_content, None)
            } else {
                (rendered, Some(spans))
            }
        };

        let now = holon_api::clock::now_millis();
        let mut block = Block {
            // ALLOW(entity_uri_from_raw): org parser output: id from extract_or_generate_id()
            id: EntityUri::from_raw(&id),
            // ALLOW(entity_uri_from_raw): org parser output: parent headline raw org slug
            parent_id: EntityUri::from_raw(parent_id),
            content,
            marks,
            created_at: now,
            updated_at: now,
            ..Block::default()
        };

        // Set org-specific properties using extension trait
        block.set_level(level);
        block.set_sequence(sequence);
        block.set_task_state(task_state);
        block.set_priority(priority);
        block.set_tags(tags);
        block.set_scheduled(scheduled);
        block.set_deadline(deadline);

        // Store drawer properties as flat keys in block properties.
        // `REQUIRES` is the only edge-typed drawer key — it gets pulled out
        // into block.requires (Vec<String>) so SqlOperationProvider's edge
        // partition can route it to the block_requires junction. Values may
        // be either comma- or whitespace-separated (org-edna convention is
        // space-separated; we accept both for ergonomics). Bare slugs are
        // promoted to `block:` URIs at the boundary so block_requires.required_id
        // matches block.id (per docs/Reference/ORG_SYNTAX.md). Anything else stays as
        // a flat string property on block.properties.
        for (key, value) in string_properties.iter() {
            if key.eq_ignore_ascii_case("REQUIRES") || key.eq_ignore_ascii_case("BLOCKED-BY") {
                // `:REQUIRES:` and `:BLOCKED-BY:` are two org-drawer spellings of
                // the SAME dependency edge (the `block_requires` junction — see
                // block_requires.sql). Accept both on input and UNION them into
                // `block.requires`; the renderer emits the canonical
                // `:REQUIRES:` (ruling 2026-07-16; `:BLOCKED-BY:` converges).
                for slug in value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                {
                    // ALLOW(entity_uri_from_raw): org drawer REQUIRES/BLOCKED-BY bare slug at parse
                    // boundary
                    let uri = EntityUri::from_raw(slug);
                    if !block.requires.contains(&uri) {
                        block.requires.push(uri);
                    }
                }
            } else if key.eq_ignore_ascii_case("ADVICE_SUPPRESSED") {
                // `:ADVICE_SUPPRESSED:` is the authored advice-suppression
                // exclusion set (ADR 0021): identical bare-ID grammar to
                // REQUIRES, pulled into block.advice_suppressed so the SQL edge
                // partition routes it to the advice_suppressed junction.
                // Closure kept deliberately: archlint's rule matches the call
                // form, so point-free would drop this boundary from the ledger.
                #[allow(clippy::redundant_closure)]
                {
                    block.advice_suppressed = value
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|s| !s.is_empty())
                        // ALLOW(entity_uri_from_raw): org drawer ADVICE_SUPPRESSED bare slug at
                        // parse boundary
                        .map(|s| EntityUri::from_raw(s))
                        .collect();
                }
            } else if key.eq_ignore_ascii_case("COLLAPSED") {
                // Outline fold state is document state (Martin ruling
                // 2026-07-11), so it round-trips through org the same as any
                // other block field — a plain drawer property, following
                // org-mode's own boolean-drawer convention (LogSeq's
                // `collapsed:: true`; org-mode itself uses `t`/`nil` for
                // drawer booleans, e.g. `:VISIBILITY:`). Absent means
                // expanded (Block::default() already sets `collapsed: false`).
                block.collapsed =
                    value.eq_ignore_ascii_case("t") || value.eq_ignore_ascii_case("true");
            } else if key.eq_ignore_ascii_case("WIDGET_ONLY") {
                // Same boolean-drawer grammar as `:COLLAPSED:`, but a present
                // value outside the accepted spellings is a hard parse error:
                // silently defaulting a render-mode flag to false would hide
                // the authored intent behind a correct-looking page.
                if value.eq_ignore_ascii_case("t") || value.eq_ignore_ascii_case("true") {
                    block.widget_only = true;
                } else {
                    anyhow::bail!(
                        "block {id}: :WIDGET_ONLY: must be `t` or `true` (case-insensitive), got \
                         {value:?}"
                    );
                }
            } else {
                block.set_property(key, holon_api::Value::String(value.to_string()));
            }
        }
        // Record the authored drawer key order so the renderer replays it
        // instead of alphabetizing — a reordered drawer is pure write-back
        // churn. `:BLOCKED-BY:` folds onto the canonical `:REQUIRES:` spelling
        // so the slot it occupied is the one `:REQUIRES:` gets back.
        let mut drawer_order: Vec<String> = Vec::new();
        for (key, _) in string_properties.iter() {
            let canonical = if key.eq_ignore_ascii_case("BLOCKED-BY") {
                "REQUIRES".to_string()
            } else {
                key.clone()
            };
            // Exact-match dedupe: `:Effort:` and `:effort:` are DISTINCT drawer
            // keys and both round-trip, so collapsing them by case would hand
            // one of them the other's slot.
            if !drawer_order.contains(&canonical) {
                drawer_order.push(canonical);
            }
        }
        if !drawer_order.is_empty() {
            block.set_property(
                crate::models::org_props::DRAWER_ORDER,
                holon_api::Value::String(
                    serde_json::to_string(&drawer_order)
                        .expect("drawer key order is a Vec<String> — always serializable"),
                ),
            );
        }

        // Store ID in properties (extract_properties filters it out since it's used for
        // block.id)
        block.set_property("ID", holon_api::Value::String(id.clone()));

        output.push(block);

        // Source-block + image children (shared with the document top-level
        // pass in `parse_org_file`).
        emit_section_children(
            source_blocks,
            section.image_paths,
            &id,
            sequence_counter,
            output,
        );

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

/// Extract :ID: property from headline, or generate a new UUID.
/// Lookup is case-insensitive so Logseq-written lowercase `:id:` is matched.
/// Returns (id, needs_write_back)
fn extract_or_generate_id(headline: &Headline) -> (String, bool) {
    if let Some(drawer) = headline.properties() {
        if let Some(id_token) = drawer.iter().find_map(|(k, v)| {
            if k.trim().eq_ignore_ascii_case("ID") {
                Some(v)
            } else {
                None
            }
        }) {
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

/// Extract custom properties from the property drawer (excludes :ID:), in the
/// order the author wrote them. That order is authored data — the renderer
/// replays it so write-back does not churn the file.
fn extract_properties(headline: &Headline) -> Vec<(String, String)> {
    let drawer = match headline.properties() {
        Some(d) => d,
        None => return Vec::new(),
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
    /// Relative file paths of images found as [[file:...]] links in body text
    image_paths: Vec<String>,
    // ALLOW(fallback): orgize misclassifies SCHEDULED as PARAGRAPH when properties drawer precedes
    // planning
    /// SCHEDULED recovered from paragraph text when orgize misclassifies it.
    scheduled_fallback: Option<String>,
    // ALLOW(fallback): orgize misclassifies DEADLINE as PARAGRAPH when properties drawer precedes
    // planning
    /// DEADLINE recovered from paragraph text when orgize misclassifies it.
    deadline_fallback: Option<String>,
}

fn extract_section_content(section_opt: Option<Section>) -> SectionContent {
    let section = match section_opt {
        Some(s) => s,
        None => {
            return SectionContent {
                body: None,
                source_blocks: Vec::new(),
                image_paths: Vec::new(),
                scheduled_fallback: None,
                deadline_fallback: None,
            };
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
                // Renderer (models.rs::source_block_to_org) always emits exactly one '\n'
                // before #+END_SRC; orgize hands it back to us as part of `value()`.
                // Strip exactly one trailing '\n' so block.content stays the canonical
                // source text — round-trip fidelity, not a presentation artifact.
                let raw = src_block.value();
                let trimmed = raw.strip_suffix('\n').unwrap_or(&raw);
                // Invert the renderer's comma-escape on '*' / '#+' lines.
                let source = unescape_source_lines(trimmed);
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

    // ALLOW(fallback): regex-extract SCHEDULED/DEADLINE from PARAGRAPH text orgize
    // misclassifies Extract SCHEDULED/DEADLINE fallback from non-planning text
    // (orgize misclassifies them as PARAGRAPH when properties drawer precedes
    // planning).
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

    let (plain_text, image_paths) = extract_image_links(&body_text);

    SectionContent {
        body: plain_text,
        source_blocks,
        image_paths,
        scheduled_fallback,
        deadline_fallback,
    }
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tiff", "tif",
];

fn is_image_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    IMAGE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Extract `[[file:path.png]]` image links from body text.
/// Returns (remaining body text or None, extracted image paths).
fn extract_image_links(body: &str) -> (Option<String>, Vec<String>) {
    let mut image_paths = Vec::new();
    let mut remaining = String::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed
            .strip_prefix("[[file:")
            .and_then(|s| s.strip_suffix("]]"))
        {
            if is_image_path(path) {
                image_paths.push(path.to_string());
                continue;
            }
        }
        if !remaining.is_empty() {
            remaining.push('\n');
        }
        remaining.push_str(line);
    }

    let trimmed = crate::models::trim_blank_lines(&remaining);
    let plain_text = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    (plain_text, image_paths)
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

/// Invert the renderer's comma-escape inside source/example block bodies:
/// strip exactly one leading ',' from any line that, after the strip, starts
/// with '*' or '#+'. Mirrors `models.rs::escape_source_lines`.
fn unescape_source_lines(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for (i, line) in content.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let unescaped = line.strip_prefix(',').filter(|&rest| {
            rest.starts_with('*')
                || rest.starts_with("#+")
                || rest.starts_with(",*")
                || rest.starts_with(",#+")
        });
        out.push_str(unescaped.unwrap_or(line));
    }
    out
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Parse `content` as `/test/file.org` rooted at `/test` with no parent —
    /// the standard fixture for parser unit tests.
    fn parse_test_org(content: &str) -> ParseResult {
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap()
    }

    /// F8 (dogfood 2026-07-21): a doc `#+ID:` equal to a heading `:ID:` makes
    /// the heading its own parent (`block.parent_id == block.id`). Before
    /// the fix the parser happily emitted that self-parent block and the
    /// recursive `focus_descendants` projection blew the stack on boot.
    /// Parse must now reject it loudly (â ingest quarantine) naming the
    /// file and colliding id.
    #[test]
    fn parse_rejects_doc_id_equal_to_heading_id_self_parent() {
        let content = "#+ID: cyc-id\n* Cyc\n:PROPERTIES:\n:ID: cyc-id\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let err = match parse_org_file(&path, content, &EntityUri::no_parent(), &root) {
            Ok(_) => {
                panic!("doc-id == heading-id collision must be rejected, not silently ingested")
            }
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cyc-id") && msg.contains("file.org"),
            "error must name the colliding id and the file: {msg}"
        );
    }

    /// Two headings sharing an `:ID:` tie one identity to two nodes --
    /// ambiguous parenthood that can cycle the projection. Reject at parse.
    #[test]
    fn parse_rejects_duplicate_heading_ids() {
        let content =
            "* One\n:PROPERTIES:\n:ID: dup\n:END:\n* Two\n:PROPERTIES:\n:ID: dup\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let err = match parse_org_file(&path, content, &EntityUri::no_parent(), &root) {
            Ok(_) => panic!("duplicate heading :ID: must be rejected"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("dup"),
            "error must name the duplicate id"
        );
    }

    /// A well-formed file (distinct ids, no `#+ID:` collision) still parses.
    #[test]
    fn parse_accepts_distinct_ids() {
        let content = "#+ID: doc-root\n* A\n:PROPERTIES:\n:ID: a1\n:END:\n* B\n:PROPERTIES:\n:ID: b1\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        assert!(
            parse_org_file(&path, content, &EntityUri::no_parent(), &root).is_ok(),
            "distinct ids must parse cleanly"
        );
    }

    #[test]
    fn test_parse_simple_headlines() {
        let content = "* First headline\n** Nested headline\n* Second headline";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 3);
        assert_eq!(result.blocks[0].org_title(), "First headline");
        assert_eq!(result.blocks[1].org_title(), "Nested headline");
        assert_eq!(result.blocks[1].parent_id, result.blocks[0].id);
        assert_eq!(result.blocks[2].org_title(), "Second headline");
    }

    #[test]
    fn test_parse_todo_and_priority() {
        let content = "* TODO [#A] Important task :work:urgent:";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        let h = &result.blocks[0];
        assert_eq!(h.task_state(), Some(TaskState::active("TODO")));
        assert_eq!(h.priority(), Some(holon_api::Priority::High));
        assert_eq!(h.tags(), holon_api::Tags::from_csv("work,urgent"));
    }

    #[test]
    fn test_parse_requires_drawer_promotes_bare_slugs_to_block_uris() {
        // Bare slugs in :REQUIRES: must be promoted to `block:` URIs so the
        // junction table's required_id matches block.id at JOIN time.
        let content = "* TODO Task\n:PROPERTIES:\n:ID: t1\n:REQUIRES: foo, bar baz\n:END:\n";
        let result = parse_test_org(content);

        let h = result.blocks.iter().find(|b| b.id.id() == "t1").unwrap();
        assert_eq!(
            h.requires,
            vec![
                EntityUri::parse("block:foo").unwrap(),
                EntityUri::parse("block:bar").unwrap(),
                EntityUri::parse("block:baz").unwrap(),
            ],
            "bare slugs (comma- or whitespace-separated) must be normalised to block: URIs"
        );
    }

    #[test]
    fn test_blocked_by_drawer_lifts_into_requires_edge() {
        // `:BLOCKED-BY:` is the canonical org-drawer spelling of the `requires`
        // dependency edge (block_requires junction). It must lift into
        // `block.requires` exactly like `:REQUIRES:`, and NOT leak as a raw
        // property.
        let content = "* TODO Task\n:PROPERTIES:\n:ID: t1\n:BLOCKED-BY: foo, bar baz\n:END:\n";
        let result = parse_test_org(content);

        let h = result.blocks.iter().find(|b| b.id.id() == "t1").unwrap();
        assert_eq!(
            h.requires,
            vec![
                EntityUri::parse("block:foo").unwrap(),
                EntityUri::parse("block:bar").unwrap(),
                EntityUri::parse("block:baz").unwrap(),
            ],
            ":BLOCKED-BY: must lift into block.requires as block: URIs"
        );
        assert!(
            !h.properties.contains_key("BLOCKED-BY"),
            "`BLOCKED-BY` must NOT leak into properties; found: {:?}",
            h.properties
        );
    }

    #[test]
    fn test_blocked_by_edge_roundtrips_via_canonical_requires() {
        // End-to-end org round-trip for the dependency edge through the real
        // render path (`OrgRenderer::render_entitys`). Canonical render key is
        // `:REQUIRES:` (owner ruling 2026-07-16); a `:BLOCKED-BY:`-authored edge
        // survives render -> re-parse losslessly, converged to `:REQUIRES:`.
        use crate::org_renderer::OrgRenderer;

        let content = "* TODO Task\n:PROPERTIES:\n:BLOCKED-BY: orient-daily-view \
                       now-query-mcp\n:ID: t1\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let file_id = generate_file_id(&path, &root);

        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();
        let h = result.blocks.iter().find(|b| b.id.id() == "t1").unwrap();
        assert_eq!(
            h.requires,
            vec![
                EntityUri::parse("block:orient-daily-view").unwrap(),
                EntityUri::parse("block:now-query-mcp").unwrap(),
            ],
            ":BLOCKED-BY: must lift into block.requires"
        );

        let rendered = OrgRenderer::render_entitys(&result.blocks, &path, &file_id);
        // Canonical key :REQUIRES:, targets sorted (set-valued edge):
        // "now-query-mcp" < "orient-daily-view".
        assert!(
            rendered.contains(":REQUIRES: now-query-mcp orient-daily-view"),
            "renderer must emit the canonical sorted :REQUIRES: drawer; got:\n{rendered}"
        );
        assert!(
            !rendered.contains(":BLOCKED-BY:"),
            ":BLOCKED-BY: must converge to :REQUIRES: on render; got:\n{rendered}"
        );

        // Re-parse the rendered text: the typed edge survives (sorted order).
        let result2 = parse_org_file(&path, &rendered, &EntityUri::no_parent(), &root).unwrap();
        let h2 = result2.blocks.iter().find(|b| b.id.id() == "t1").unwrap();
        assert_eq!(
            h2.requires,
            vec![
                EntityUri::parse("block:now-query-mcp").unwrap(),
                EntityUri::parse("block:orient-daily-view").unwrap(),
            ],
            "dependency edge must survive render -> re-parse (sorted canonical order)"
        );
    }

    #[test]
    fn test_blocked_by_alias_converges_to_requires_on_writeback() {
        // `:BLOCKED-BY:` input is accepted and converges to the canonical
        // `:REQUIRES:` spelling on re-render (convergent canonical form; owner
        // ruling 2026-07-16). Both spellings name the same block_requires edge.
        use crate::org_renderer::OrgRenderer;

        let content = "* TODO Task\n:PROPERTIES:\n:BLOCKED-BY: dep-a\n:ID: t2\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let file_id = generate_file_id(&path, &root);

        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();
        let h = result.blocks.iter().find(|b| b.id.id() == "t2").unwrap();
        assert_eq!(h.requires, vec![EntityUri::parse("block:dep-a").unwrap()]);

        let rendered = OrgRenderer::render_entitys(&result.blocks, &path, &file_id);
        assert!(
            rendered.contains(":REQUIRES: dep-a") && !rendered.contains(":BLOCKED-BY:"),
            ":BLOCKED-BY: input must render back as canonical :REQUIRES:; got:\n{rendered}"
        );
    }

    #[test]
    fn test_source_block_requires_survives_org_roundtrip() {
        // A source block carrying a typed `requires` edge must round-trip
        // through org render -> re-parse. The renderer emits `requires` via
        // `drawer_properties()` as a `:REQUIRES <bare>` header arg on the
        // #+BEGIN_SRC line; the parser must lift that back into the typed
        // `block.requires` edge field, symmetric with the headline path.
        // (Regression: forced-weight keystone red on inv-blocks-match-ref/org.)
        let mut src = Block {
            id: EntityUri::block("lessons_for_tasks::src::0"),
            parent_id: EntityUri::block("lessons_for_tasks"),
            content: "select 1".to_string(),
            content_type: ContentType::Source,
            source_language: Some("holon_prql".parse::<SourceLanguage>().unwrap()),
            requires: vec![EntityUri::parse("block:lessons_for_tasks::rule::0").unwrap()],
            ..Block::default()
        };
        src.set_sequence(0);

        use crate::models::ToOrg;
        let org = format!(
            "* Rule\n:PROPERTIES:\n:ID: lessons_for_tasks\n:END:\n{}",
            src.to_org()
        );
        let result = parse_test_org(&org);

        let parsed = result
            .blocks
            .iter()
            .find(|b| b.content_type == ContentType::Source)
            .expect("source block must survive re-parse");
        assert_eq!(
            parsed.requires,
            vec![EntityUri::parse("block:lessons_for_tasks::rule::0").unwrap()],
            "source-block `requires` edge must survive the org round-trip as a typed edge"
        );
        assert!(
            !parsed.properties.contains_key("REQUIRES"),
            "`REQUIRES` must NOT leak into properties as a raw string; found: {:?}",
            parsed.properties
        );
    }

    #[test]
    fn test_source_block_tags_survive_org_roundtrip() {
        // A Source block has no headline to carry `:tag:` notation, so its tags
        // ride a `:TAGS <space-joined>` header arg on the #+BEGIN_SRC line and
        // the parser must lift them back into `block.tags`. (Regression: keystone
        // red on inv-blocks-match-ref/org — a `task` tag added to the journals
        // holon_rule block was destroyed on org re-ingest.)
        let mut src = Block {
            id: EntityUri::block("journals::action::0"),
            parent_id: EntityUri::block("journals::auto-create"),
            content: "name: daily_journal".to_string(),
            content_type: ContentType::Source,
            source_language: Some("holon_rule".parse::<SourceLanguage>().unwrap()),
            tags: Tags::from_tag_iter(["task".to_string(), "urgent".to_string()]),
            ..Block::default()
        };
        src.set_sequence(0);

        use crate::models::ToOrg;
        let org = format!(
            "* Rule\n:PROPERTIES:\n:ID: journals::auto-create\n:END:\n{}",
            src.to_org()
        );
        let result = parse_test_org(&org);

        let parsed = result
            .blocks
            .iter()
            .find(|b| b.content_type == ContentType::Source)
            .expect("source block must survive re-parse");
        assert_eq!(
            parsed.tags,
            Tags::from_tag_iter(["task".to_string(), "urgent".to_string()]),
            "source-block tags must survive the org round-trip"
        );
        assert!(
            !parsed.properties.contains_key("TAGS"),
            "`TAGS` must NOT leak into properties as a raw string; found: {:?}",
            parsed.properties
        );
    }

    #[test]
    fn test_parse_requires_preserves_existing_block_uris() {
        let content = "* TODO Task\n:PROPERTIES:\n:ID: t2\n:REQUIRES: block:foo\n:END:\n";
        let result = parse_test_org(content);

        let h = result.blocks.iter().find(|b| b.id.id() == "t2").unwrap();
        assert_eq!(h.requires, vec![EntityUri::parse("block:foo").unwrap()]);
    }

    #[test]
    fn test_advice_suppressed_drawer_round_trips_byte_identically() {
        // The `:ADVICE_SUPPRESSED:` drawer (ADR 0021) parses into the typed
        // `block.advice_suppressed` edge field (bare slugs promoted to
        // `block:` URIs at the boundary) and renders back to the same bare
        // space-separated list — a byte-identical round-trip.
        use crate::org_renderer::OrgRenderer;

        let content = "* TODO Task\n:PROPERTIES:\n:ADVICE_SUPPRESSED: id1 id2\n:ID: a1\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let file_id = generate_file_id(&path, &root);

        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();
        let h = result.blocks.iter().find(|b| b.id.id() == "a1").unwrap();
        assert_eq!(
            h.advice_suppressed,
            vec![
                EntityUri::parse("block:id1").unwrap(),
                EntityUri::parse("block:id2").unwrap(),
            ],
            "bare slugs must normalise to block: URIs in the typed field"
        );

        let rendered = OrgRenderer::render_entitys(&result.blocks, &path, &file_id);
        assert!(
            rendered.contains(":ADVICE_SUPPRESSED: id1 id2"),
            "renderer must emit the bare space-joined list, got:\n{rendered}"
        );

        // Re-parse the rendered text: the typed field must be identical.
        let result2 = parse_org_file(&path, &rendered, &EntityUri::no_parent(), &root).unwrap();
        let h2 = result2.blocks.iter().find(|b| b.id.id() == "a1").unwrap();
        assert_eq!(
            h2.advice_suppressed, h.advice_suppressed,
            "advice_suppressed must survive render → re-parse unchanged"
        );
    }

    #[test]
    fn test_collapsed_drawer_round_trips() {
        // Outline fold state is document state (Martin ruling 2026-07-11):
        // `:COLLAPSED: t` in the properties drawer parses into
        // `block.collapsed`, and a folded block renders that property back
        // out on write. Absent property means expanded (false) — a
        // never-folded file's drawer must NOT gain a `:COLLAPSED: nil` line
        // (matches the `requires`/`advice_suppressed` only-if-set convention).
        use crate::org_renderer::OrgRenderer;

        let content = "* TODO Task\n:PROPERTIES:\n:COLLAPSED: t\n:ID: c1\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let file_id = generate_file_id(&path, &root);

        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();
        let h = result.blocks.iter().find(|b| b.id.id() == "c1").unwrap();
        assert!(
            h.collapsed,
            "COLLAPSED: t must parse to block.collapsed = true"
        );

        let rendered = OrgRenderer::render_entitys(&result.blocks, &path, &file_id);
        assert!(
            rendered.contains(":COLLAPSED: t"),
            "renderer must emit the drawer property for a folded block, got:\n{rendered}"
        );

        let result2 = parse_org_file(&path, &rendered, &EntityUri::no_parent(), &root).unwrap();
        let h2 = result2.blocks.iter().find(|b| b.id.id() == "c1").unwrap();
        assert!(
            h2.collapsed,
            "collapsed must survive render -> re-parse unchanged"
        );

        // An expanded (never-collapsed) block must not gain the property.
        let expanded_content = "* TODO Task2\n:PROPERTIES:\n:ID: c2\n:END:\n";
        let expanded_result =
            parse_org_file(&path, expanded_content, &EntityUri::no_parent(), &root).unwrap();
        let e = expanded_result
            .blocks
            .iter()
            .find(|b| b.id.id() == "c2")
            .unwrap();
        assert!(!e.collapsed);
        let expanded_rendered =
            OrgRenderer::render_entitys(&expanded_result.blocks, &path, &file_id);
        assert!(
            !expanded_rendered.contains("COLLAPSED"),
            "an expanded block must not gain a :COLLAPSED: drawer line, got:\n{expanded_rendered}"
        );
    }

    #[test]
    fn test_widget_only_drawer_round_trips() {
        // `:WIDGET_ONLY: t` is a typed Block field, so it survives the org
        // round-trip that drops untyped non-String properties. A block without
        // the flag must not gain the drawer key.
        use crate::org_renderer::OrgRenderer;

        let content = "* Query\n:PROPERTIES:\n:WIDGET_ONLY: t\n:ID: w1\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let file_id = generate_file_id(&path, &root);

        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();
        let h = result.blocks.iter().find(|b| b.id.id() == "w1").unwrap();
        assert!(
            h.widget_only,
            "WIDGET_ONLY: t must parse to block.widget_only = true"
        );

        let rendered = OrgRenderer::render_entitys(&result.blocks, &path, &file_id);
        assert!(
            rendered.contains(":WIDGET_ONLY: t"),
            "renderer must emit the drawer property, got:\n{rendered}"
        );

        let result2 = parse_org_file(&path, &rendered, &EntityUri::no_parent(), &root).unwrap();
        let h2 = result2.blocks.iter().find(|b| b.id.id() == "w1").unwrap();
        assert!(
            h2.widget_only,
            "widget_only must survive render -> re-parse unchanged"
        );

        let plain = "* Query2\n:PROPERTIES:\n:ID: w2\n:END:\n";
        let plain_result = parse_org_file(&path, plain, &EntityUri::no_parent(), &root).unwrap();
        let p = plain_result
            .blocks
            .iter()
            .find(|b| b.id.id() == "w2")
            .unwrap();
        assert!(!p.widget_only);
        let plain_rendered = OrgRenderer::render_entitys(&plain_result.blocks, &path, &file_id);
        assert!(
            !plain_rendered.contains("WIDGET_ONLY"),
            "a plain block must not gain a :WIDGET_ONLY: drawer line, got:\n{plain_rendered}"
        );
    }

    #[test]
    fn test_widget_only_rejects_unknown_spelling() {
        // Unlike :COLLAPSED:, an unrecognised :WIDGET_ONLY: value fails loud
        // instead of silently rendering the headline the author asked to hide.
        let content = "* Query\n:PROPERTIES:\n:WIDGET_ONLY: banana\n:ID: w3\n:END:\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");

        // `ParseResult` is not `Debug`, so unwrap the Err arm by hand.
        let msg = match parse_org_file(&path, content, &EntityUri::no_parent(), &root) {
            Ok(_) => panic!(":WIDGET_ONLY: banana must be a parse error, not a silent false"),
            Err(e) => format!("{e:#}"),
        };
        assert!(msg.contains("w3"), "error must name the block, got: {msg}");
        assert!(
            msg.contains("banana"),
            "error must name the bad value, got: {msg}"
        );
    }

    #[test]
    fn test_widget_only_accepts_case_insensitive_spellings() {
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        for spelling in ["t", "T", "true", "TRUE", "True"] {
            let content =
                format!("* Query\n:PROPERTIES:\n:WIDGET_ONLY: {spelling}\n:ID: w4\n:END:\n");
            let result = parse_org_file(&path, &content, &EntityUri::no_parent(), &root)
                .unwrap_or_else(|e| panic!("{spelling:?} must parse: {e:#}"));
            let b = result.blocks.iter().find(|b| b.id.id() == "w4").unwrap();
            assert!(b.widget_only, "{spelling:?} must parse as widget_only");
        }
    }

    #[test]
    fn test_default_keywords_without_todo_config() {
        // Files without #+TODO: should still recognize DOING from
        // DEFAULT_ACTIVE_KEYWORDS
        let content = "* DOING Work in progress\n* DONE Finished task\n* CANCELLED Dropped task";
        let result = parse_test_org(content);

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
    fn test_logseq_dialect_keywords_recognized() {
        // A foreign LogSeq vault carries no `#+TODO:` header. LATER and NOW
        // must still parse as task keywords (ForeignVaultCompat §4): LATER is
        // TODO-family (active, not started), NOW is DOING-family (active, in
        // progress). Neither is a done keyword.
        let content = "* LATER Draft the proposal\n* NOW Review the draft\n* DONE Ship it";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 3);
        assert_eq!(
            result.blocks[0].task_state(),
            Some(TaskState::active("LATER"))
        );
        assert_eq!(result.blocks[0].org_title(), "Draft the proposal");
        assert_eq!(
            result.blocks[1].task_state(),
            Some(TaskState::active("NOW"))
        );
        assert_eq!(result.blocks[1].org_title(), "Review the draft");
        assert_eq!(result.blocks[2].task_state(), Some(TaskState::done("DONE")));

        // NOW is DOING-family (drives the in-progress glyph); LATER is not.
        assert!(result.blocks[1].task_state().unwrap().is_doing());
        assert!(!result.blocks[0].task_state().unwrap().is_doing());
    }

    #[test]
    fn test_logseq_dialect_keyword_round_trips_byte_identical() {
        // Round-trip fidelity (ADR 0025 write-back doctrine): a LATER/NOW
        // block must render back with the SAME source keyword, never
        // normalized to TODO/DOING.
        use crate::models::ToOrg;
        for keyword in ["LATER", "NOW"] {
            let content = format!("* {keyword} Task headline");
            let result = parse_test_org(&content);
            let block = &result.blocks[0];

            // The typed keyword renders back to its exact source spelling.
            assert_eq!(block.task_state().unwrap().to_string(), keyword);

            // The full headline line renders byte-identical.
            let rendered = block.to_org();
            assert_eq!(
                rendered.lines().next().unwrap(),
                format!("* {keyword} Task headline")
            );
        }
    }

    #[test]
    fn test_custom_done_keyword_preserves_category() {
        // `set_task_state` must not destroy the category the parser derived
        // from the file's `#+TODO:` config: SHIPPED is a done keyword here
        // even though it is not in DEFAULT_DONE_KEYWORDS.
        let content = "#+TODO: TODO | SHIPPED\n* SHIPPED Task";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        assert_eq!(
            result.blocks[0].task_state(),
            Some(TaskState::done("SHIPPED"))
        );
        assert!(result.blocks[0].is_completed());

        // Custom ACTIVE categorization of a default-done keyword must
        // survive too (`#+TODO: DONE | SHIPPED` makes DONE an active state).
        let content = "#+TODO: DONE | SHIPPED\n* DONE Task";
        let result = parse_test_org(content);
        assert_eq!(
            result.blocks[0].task_state(),
            Some(TaskState::active("DONE"))
        );
        assert!(!result.blocks[0].is_completed());
    }

    #[test]
    fn test_parse_title_and_todo_keywords() {
        let content = "#+TITLE: My Document\n#+TODO: TODO INPROGRESS | DONE CANCELLED\n* Task";
        let result = parse_test_org(content);

        assert_eq!(
            result.document.file_title(),
            Some("My Document".to_string())
        );
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
    fn test_parse_hyphenated_drawer_property() {
        let content = "* Y1\n:PROPERTIES:\n:ID: bulk-3-0\n:column-order: 6gnLm\n:END:\n";
        let result = parse_test_org(content);
        assert_eq!(result.blocks.len(), 1);
        let block = &result.blocks[0];
        eprintln!("properties: {:?}", block.properties);
        assert_eq!(
            block
                .properties
                .get("column-order")
                .and_then(|v| v.as_string()),
            Some("6gnLm"),
            "column-order should survive drawer parsing"
        );
    }

    #[test]
    fn test_parse_existing_id_property() {
        let content = "* Headline\n:PROPERTIES:\n:ID: existing-uuid-here\n:END:";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id.id(), "existing-uuid-here");
        assert!(result.headlines_needing_ids.is_empty());
    }

    #[test]
    fn test_headlines_without_id_need_writeback() {
        let content = "* Headline without ID";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        assert!(!result.headlines_needing_ids.is_empty());
    }

    #[test]
    fn test_parse_lowercase_id_property() {
        let content = "* Headline\n:PROPERTIES:\n:id: lower-case-uuid\n:END:";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id.id(), "lower-case-uuid");
        assert!(result.headlines_needing_ids.is_empty());
    }

    #[test]
    fn test_parse_mixed_case_id_property() {
        let content = "* Headline\n:PROPERTIES:\n:Id: mixed-case-uuid\n:END:";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id.id(), "mixed-case-uuid");
        assert!(result.headlines_needing_ids.is_empty());
    }

    #[test]
    fn test_case_insensitive_id_does_not_absorb_other_properties() {
        let content = "* Headline\n:PROPERTIES:\n:id: my-id\n:Custom: val\n:END:";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id.id(), "my-id");
        assert!(result.headlines_needing_ids.is_empty());
        let props = &result.blocks[0].properties;
        assert_eq!(
            props.get("Custom").and_then(|v| v.as_string()),
            Some("val"),
            "custom property keyed 'Custom' must survive"
        );
        // The parser always stores the id under the canonical "ID" key
        // (line ~473). The :id: property (any casing) is correctly extracted
        // as the block ID and stored; it does NOT appear under its original
        // casing.
        assert_eq!(
            props.get("ID").and_then(|v| v.as_string()),
            Some("my-id"),
            "ID must be stored under canonical key 'ID'"
        );
    }

    #[test]
    fn test_parse_source_block_basic() {
        let content = r#"* Headline with code
#+BEGIN_SRC python
def hello():
    print("Hello, world!")
#+END_SRC
"#;
        let result = parse_test_org(content);

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
        let result = parse_test_org(content);

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
        let result = parse_test_org(content);

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
        let result = parse_test_org(content);

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
        let result = parse_test_org(content);

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
        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();

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

    #[test]
    fn test_image_block_parse() {
        let content =
            "* Heading with image\n:PROPERTIES:\n:ID: h1\n:END:\n[[file:attachments/photo.png]]\n";
        let result = parse_test_org(content);

        assert_eq!(result.blocks.len(), 2, "Expected headline + image block");
        let heading = &result.blocks[0];
        let img = &result.blocks[1];

        assert_eq!(heading.content_type, ContentType::Text);
        assert_eq!(img.content_type, ContentType::Image);
        assert_eq!(img.content, "attachments/photo.png");
        assert_eq!(img.parent_id, heading.id);
    }

    #[test]
    fn test_image_block_round_trip() {
        use crate::org_renderer::OrgRenderer;

        let content = "* Gallery\n:PROPERTIES:\n:ID: \
                       gallery-1\n:END:\n[[file:attachments/a.jpg]]\n[[file:img/b.png]]\n";
        let path = PathBuf::from("/test/file.org");
        let root = PathBuf::from("/test");
        let file_id = generate_file_id(&path, &root);

        let result = parse_org_file(&path, content, &EntityUri::no_parent(), &root).unwrap();
        assert_eq!(result.blocks.len(), 3, "heading + 2 images");

        let img1 = &result.blocks[1];
        let img2 = &result.blocks[2];
        assert_eq!(img1.content_type, ContentType::Image);
        assert_eq!(img1.content, "attachments/a.jpg");
        assert_eq!(img2.content_type, ContentType::Image);
        assert_eq!(img2.content, "img/b.png");

        // Round-trip: render back to org text
        let rendered = OrgRenderer::render_entitys(&result.blocks, &path, &file_id);

        // Re-parse the rendered text
        let result2 = parse_org_file(&path, &rendered, &EntityUri::no_parent(), &root).unwrap();

        assert_eq!(
            result2.blocks.len(),
            result.blocks.len(),
            "Block count must survive round-trip"
        );

        let img1_rt = &result2.blocks[1];
        let img2_rt = &result2.blocks[2];
        assert_eq!(img1_rt.content_type, ContentType::Image);
        assert_eq!(img1_rt.content, "attachments/a.jpg");
        assert_eq!(img2_rt.content_type, ContentType::Image);
        assert_eq!(img2_rt.content, "img/b.png");
    }

    #[test]
    fn test_image_to_org_format() {
        use crate::models::ToOrg;

        let img = Block::new_image(
            EntityUri::block("img-1"),
            EntityUri::block("parent-1"),
            "attachments/photo.png",
        );
        assert_eq!(img.to_org(), "[[file:attachments/photo.png]]\n");
    }

    #[test]
    fn test_non_image_file_link_preserved_as_text() {
        let content = "* With PDF link\n:PROPERTIES:\n:ID: h1\n:END:\n[[file:docs/report.pdf]]\n";
        let result = parse_test_org(content);

        // PDF links should NOT create image blocks (the original test intent).
        assert_eq!(result.blocks.len(), 1, "Only the heading, no image block");
        let heading = &result.blocks[0];

        // Phase 1.1 marks integration: the file: link is now extracted as a
        // Link mark, with the rendered text being the URI itself. The mark
        // round-trips back to the original `[[file:docs/report.pdf]]` via
        // `Block::to_org`. Verify the mark exists and the rendered content
        // contains the file path.
        assert!(
            heading.content.contains("file:docs/report.pdf"),
            "PDF link target should be in rendered content, got: {:?}",
            heading.content
        );
        let marks = heading.marks.as_ref().expect("file: link → Link mark");
        assert!(
            marks
                .iter()
                .any(|m| matches!(m.mark, holon_api::InlineMark::Link { .. })),
            "expected a Link mark for the file: link, got: {marks:?}"
        );
    }

    #[test]
    fn test_parser_extracts_inline_marks_from_paragraph() {
        // Phase 1.1 integration: a heading body paragraph with `*bold*` and
        // `/italic/` populates Block.marks with the corresponding spans, and
        // strips the delimiters from Block.content.
        let content = "* heading
:PROPERTIES:
:ID: h1
:END:

paragraph with *bold* and /italic/ words
";
        let result = parse_test_org(content);

        let heading = &result.blocks[0];
        assert_eq!(heading.content_text(), heading.content_text());
        let marks = heading.marks.as_ref().expect("inline marks present");
        assert!(
            marks.iter().any(|m| m.mark == holon_api::InlineMark::Bold),
            "expected Bold mark, got: {marks:?}"
        );
        assert!(
            marks
                .iter()
                .any(|m| m.mark == holon_api::InlineMark::Italic),
            "expected Italic mark, got: {marks:?}"
        );
        // Delimiters stripped from content.
        assert!(
            !heading.content.contains("*bold*"),
            "delimiters should be stripped from content, got: {:?}",
            heading.content
        );
        assert!(
            heading.content.contains("bold") && heading.content.contains("italic"),
            "rendered text should contain the inner words, got: {:?}",
            heading.content
        );
    }

    #[test]
    fn test_parser_renderer_round_trip_with_marks() {
        // Phase 1.1 integration end-to-end: parse a heading with inline
        // marks, render it back via Block::to_org, confirm the rendered org
        // contains the original delimiters (the marks survived).
        use crate::models::ToOrg;

        let original = "* heading\n:PROPERTIES:\n:ID: h3\n:END:\n\nthis is *bold* and /italic/\n";
        let result = parse_test_org(original);

        let heading = &result.blocks[0];
        assert!(heading.marks.is_some(), "marks extracted");
        let rendered = heading.to_org();
        // Marks should be re-emitted with their delimiters; the exact byte
        // sequence may differ around whitespace/blank lines but the marked
        // tokens must be present.
        assert!(
            rendered.contains("*bold*"),
            "Bold delimiters should round-trip, got: {rendered:?}"
        );
        assert!(
            rendered.contains("/italic/"),
            "Italic delimiters should round-trip, got: {rendered:?}"
        );
    }

    #[test]
    fn test_parser_no_marks_keeps_content_byte_identical() {
        // A heading without any inline markup must keep `content` byte-
        // identical to the raw text and `marks` = None — this preserves
        // ALLOW(compatibility): describes a parser invariant for legacy headings, not a
        // versioning shim backward compatibility for the bulk of the corpus.
        let content = "* plain heading
:PROPERTIES:
:ID: h2
:END:

just plain text here
";
        let result = parse_test_org(content);

        let heading = &result.blocks[0];
        assert!(heading.marks.is_none(), "no inline marks → marks=None");
        assert!(heading.content.contains("just plain text here"));
    }
}
