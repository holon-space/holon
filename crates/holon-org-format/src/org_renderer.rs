//! Loro → Org-mode rendering
//!
//! Converts Loro document blocks to org-mode format using Block with
//! OrgBlockExt.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use holon_api::EntityUri;
use holon_api::RenderLoss;
use holon_api::Rendered;
use holon_api::block::Block;
use holon_api::types::ContentType;

use crate::models::KeywordLine;
use crate::models::LineBreaks;
use crate::models::OrgBlockExt;
use crate::models::OrgDocumentExt;
use crate::models::carrier_or_loss;
use crate::task_keyword::TaskKeywordVocabulary;

/// Refuses org text the parser would not read back: writing it would take
/// every block of the page out of Holon on the next ingest.
fn refuse_unreadable(path: &Path, text: &str) -> anyhow::Result<crate::ParseResult> {
    let root = path.parent().unwrap_or(Path::new(""));
    crate::parse_org_file(path, text, &EntityUri::no_parent(), root).with_context(|| {
        format!(
            "org render of {} refused: the parser does not read the rendered file back",
            path.display()
        )
    })
}

/// One render of a page: its text, losses, header lines, and its re-read.
struct Pass {
    text: String,
    losses: Vec<RenderLoss>,
    header: Vec<KeywordLine>,
    reread: crate::ParseResult,
}

/// A block as the renderer writes it: its headline level, and for a source
/// block the id the parser mints for one with no `:id` at this place.
struct Place<'b> {
    block: &'b Block,
    level: i64,
    minted_here: Option<String>,
}

/// `kids` of `parent`, written at `parent_level`, each in its place. A
/// headline keeps its authored star count while org reads it as a child of
/// `parent` below its earlier siblings (more stars than the parent, at most
/// as many as each earlier sibling); else it is written one level below
/// `parent`.
fn places<'b>(kids: &[&'b Block], parent: &EntityUri, parent_level: i64) -> Vec<Place<'b>> {
    let mut ceiling: Option<i64> = None;
    let mut source_index = 0;
    kids.iter()
        .map(|&block| {
            let mut place = Place {
                block,
                level: parent_level + 1,
                minted_here: None,
            };
            match block.content_type {
                ContentType::Text => {
                    // block_to_org records the loss for an unreadable carrier.
                    let authored =
                        crate::models::read_carrier::<i64>(block, crate::models::org_props::STARS)
                            .unwrap_or_default();
                    if let Some(stars) = authored
                        .filter(|&stars| stars > parent_level && ceiling.is_none_or(|c| stars <= c))
                    {
                        place.level = stars;
                    }
                    ceiling = Some(place.level);
                }
                ContentType::Source => {
                    place.minted_here = Some(format!("{}::src::{source_index}", parent.id()));
                    source_index += 1;
                }
                _ => {}
            }
            place
        })
        .collect()
}

/// Render a Loro document (represented as blocks) to org-mode format.
///
/// Takes a list of blocks in tree order and converts them to org-mode text.
pub struct OrgRenderer;

impl OrgRenderer {
    /// Render a complete org document: header (#+TITLE, #+TODO) + blocks.
    ///
    /// This is THE SINGLE path for producing a complete org file from blocks.
    /// THE SINGLE path for producing a complete org file from blocks.
    pub fn render_document(
        doc_block: &Block,
        blocks: &[Block],
        path: &Path,
        file_id: &EntityUri,
    ) -> anyhow::Result<Rendered> {
        let lines = Self::page_keyword_lines(doc_block, blocks, file_id)?;
        let unedited = Self::render_pass(
            doc_block,
            blocks,
            path,
            file_id,
            &crate::page_keywords::Edits::default(),
        )?;
        let mut edit_losses = Vec::new();
        let edits = crate::page_keywords::edits(
            doc_block,
            &crate::page_keywords::Reading::of(&unedited.reread.document),
            &lines,
            &mut edit_losses,
        );
        let mut pass = if edits.is_empty() {
            unedited
        } else {
            Self::render_pass(doc_block, blocks, path, file_id, &edits)?
        };
        pass.losses.splice(0..0, edit_losses);
        check_header_reads_back(
            doc_block,
            &pass.header,
            &pass.reread.document,
            &mut pass.losses,
        );
        Ok(Rendered {
            text: pass.text,
            losses: pass.losses,
        })
    }

    /// The page written with `edits` applied to its keyword lines, and how
    /// the written file reads back.
    fn render_pass(
        doc_block: &Block,
        blocks: &[Block],
        path: &Path,
        file_id: &EntityUri,
        edits: &crate::page_keywords::Edits,
    ) -> anyhow::Result<Pass> {
        let mut losses = Vec::new();
        let (mut result, header) = crate::models::document_head(doc_block, edits, &mut losses)?;
        let header_lines_authored = doc_block.header_lines().unwrap_or_default();
        // The doc-root's OWN body — the pre-first-headline text. Like a
        // headline, a doc-root stores `title\nbody` in its content; the title
        // went out as `#+TITLE:` above, so everything after the first line is
        // body that belongs on disk between the `#+` directives and the first
        // headline. Without this it is silently deleted on every write-back,
        // and a page promoted from a `:Page:`-tagged headline loses the whole
        // body it was carrying.
        let preamble = crate::models::trim_blank_lines(
            doc_block
                .content
                .split_once('\n')
                .map(|(_, rest)| rest)
                .unwrap_or(""),
        );
        let authored = carrier_or_loss(
            crate::models::read_carrier::<String>(
                doc_block,
                crate::models::org_props::AUTHORED_TEXT,
            ),
            &doc_block.id,
            &mut losses,
        );
        let preamble = crate::models::written_text(
            crate::comma_escape::CommaEscape::Preamble,
            preamble,
            authored.as_deref(),
        );
        crate::models::disclose_text_after_source(doc_block, &mut losses);
        let blank_lines = carrier_or_loss(doc_block.blank_lines(), &doc_block.id, &mut losses);
        for line in &blank_lines.before_body {
            result.push_str(line);
            result.push('\n');
        }
        if header_lines_authored.is_empty() {
            // A page with no header lines of its own: the text follows its
            // generated header, if any, after one blank line.
            result.push_str(&crate::models::with_keyword_lines("", &header));
            if !preamble.is_empty() {
                if !header.is_empty() {
                    result.push('\n');
                }
                result.push_str(&preamble);
                result.push('\n');
            }
        } else {
            result.push_str(&crate::models::with_keyword_lines(&preamble, &header));
        }
        // The document's OWN `#+TODO:` declaration governs what its headlines
        // may spell — the same chain (`from_declared`) the editor's surface
        // projection resolves. Rendering a keyword this document does not
        // declare emits bytes the next parse reads as ordinary title text.
        let mut last_section_blank_lines = blank_lines.after;
        let head_after = last_section_blank_lines.clone();
        let vocabulary = TaskKeywordVocabulary::from_declared(doc_block.todo_keywords());
        let mut result = Self::render_walk(
            blocks,
            file_id,
            result,
            &head_after,
            &mut |block: &Block, minted_here: Option<&str>| {
                let mut block = block.clone();
                if block.content_type == ContentType::Text {
                    // render_headline_block records the loss for an unreadable carrier.
                    last_section_blank_lines = block.blank_lines().unwrap_or_default().after;
                    Self::apply_page_keyword_edits(&mut block, edits);
                }
                if let Some(loss) = Self::refuse_undeclared_task_state(&mut block, &vocabulary) {
                    losses.push(loss);
                }
                crate::models::block_to_org(&block, &vocabulary, minted_here, &mut losses)
            },
        )?;
        // The file ends with a line break and then exactly the blank lines its
        // last section had: a list body's closing blank line is dropped there.
        if !result.ends_with('\n') {
            result.push('\n');
        }
        let extra = trailing_blank_lines(&result).saturating_sub(last_section_blank_lines.len());
        result.truncate(result.len() - extra);
        match carrier_or_loss(doc_block.line_breaks(), &doc_block.id, &mut losses) {
            LineBreaks::Lf => {}
            LineBreaks::Crlf => result = result.replace("\r\n", "\n").replace('\n', "\r\n"),
            LineBreaks::Mixed => {
                result = result.replace("\r\n", "\n");
                losses.push(RenderLoss {
                    block: doc_block.id.clone(),
                    detail: format!(
                        "{} mixes CRLF and LF line breaks; every line break is written as LF",
                        path.display()
                    ),
                });
            }
        }
        let reread = refuse_unreadable(path, &result)?;
        Ok(Pass {
            text: result,
            losses,
            header,
            reread,
        })
    }

    /// The page's keyword lines: its header lines, then each block's in the
    /// order the file holds them.
    fn page_keyword_lines(
        doc_block: &Block,
        blocks: &[Block],
        file_id: &EntityUri,
    ) -> anyhow::Result<Vec<(crate::page_keywords::LineAt, String)>> {
        use crate::page_keywords::LineAt;
        let mut lines: Vec<(LineAt, String)> = doc_block
            .header_keyword_lines()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, l)| (LineAt::Header(i), l.raw))
            .collect();
        Self::render_walk(
            blocks,
            file_id,
            String::new(),
            &[],
            &mut |block: &Block, _| {
                lines.extend(
                    block
                        .keyword_lines()
                        .unwrap_or_default()
                        .into_iter()
                        .enumerate()
                        .map(|(j, l)| (LineAt::Block(block.id.clone(), j), l.raw)),
                );
                anyhow::Ok(String::new())
            },
        )?;
        Ok(lines)
    }

    /// `block` with the page-keyword edits to its own keyword lines applied.
    fn apply_page_keyword_edits(block: &mut Block, edits: &crate::page_keywords::Edits) {
        let Ok(lines) = block.keyword_lines() else {
            return;
        };
        let edited: Vec<KeywordLine> = lines
            .into_iter()
            .enumerate()
            .filter_map(|(j, mut line)| {
                match edits
                    .lines
                    .get(&crate::page_keywords::LineAt::Block(block.id.clone(), j))
                {
                    Some(Some(raw)) => line.raw = raw.clone(),
                    Some(None) => return None,
                    None => {}
                }
                Some(line)
            })
            .collect();
        block.set_keyword_lines(edited);
    }

    /// Render blocks to org-mode format.
    ///
    /// # Arguments
    /// * `blocks` - Blocks in tree order (parent before children)
    /// * `file_path` - Path to the org file (for OrgBlock metadata)
    /// * `file_id` - ID of the org file
    ///
    /// # Returns
    /// Org-mode formatted string
    /// No document accompanies these blocks, so no declaration is known. Task
    /// states therefore render as stored here; the refusal in
    /// [`Self::refuse_undeclared_task_state`] applies only where the owning
    /// document is in hand, i.e. [`Self::render_document`].
    pub fn render_entitys(
        blocks: &[Block],
        path: &Path,
        file_id: &EntityUri,
    ) -> anyhow::Result<Rendered> {
        let mut losses = Vec::new();
        // A file with no `#+TODO:` line reads with the default keywords.
        let vocabulary = TaskKeywordVocabulary::default();
        let text = Self::render_walk(
            blocks,
            file_id,
            String::new(),
            &[],
            &mut |block: &Block, minted_here: Option<&str>| {
                crate::models::block_to_org(block, &vocabulary, minted_here, &mut losses)
            },
        )?;
        refuse_unreadable(path, &text)?;
        Ok(Rendered { text, losses })
    }

    /// Dense projection variant of [`Self::render_entitys`]: identical tree
    /// walk and projection invariants, but each headline's `:ID:` drawer
    /// scaffolding is compressed to a trailing `{#alias}` token via
    /// `alias_table` (projection-only — see [`crate::dense`]). Source/Image
    /// blocks keep their canonical form.
    pub fn render_entitys_dense(
        blocks: &[Block],
        file_id: &EntityUri,
        alias_table: &crate::dense::AliasTable,
        gap_ids: &std::collections::HashSet<String>,
    ) -> String {
        let Ok(text) =
            Self::render_walk(blocks, file_id, String::new(), &[], &mut |b: &Block, _| {
                Ok::<_, std::convert::Infallible>(crate::dense::to_org_dense(
                    b,
                    alias_table,
                    gap_ids,
                ))
            });
        text
    }

    /// `head` (the page's text before its blocks) followed by the blocks. The
    /// blank lines that end the page's own section (`head_after`) stand
    /// after its source blocks, before its first headline.
    fn render_walk<E, F: FnMut(&Block, Option<&str>) -> Result<String, E>>(
        blocks: &[Block],
        file_id: &EntityUri,
        head: String,
        head_after: &[String],
        render_block: &mut F,
    ) -> Result<String, E> {
        let mut result = head;

        // Sibling order is the caller's responsibility — `blocks` arrives in
        // authoritative order (the ordered read; ADR 0005). The renderer trusts
        // it and never re-derives order from a per-block key. Build a
        // parent→children index that preserves the input order.
        let mut children_by_parent: HashMap<&str, Vec<&Block>> = HashMap::new();
        let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for b in blocks {
            ids.insert(b.id.as_str());
            children_by_parent
                .entry(b.parent_id.as_str())
                .or_default()
                .push(b);
        }

        // WP-F projection assertion (cheap, no extra pass beyond ids we already
        // built): every block's stated parent must be the file root or another
        // block in this set. Otherwise the block is a dangling orphan that this
        // renderer would silently drop (never reachable from the file roots).
        // A self-parented row (the filtered-out `sentinel:no_parent` FK anchor)
        // is excluded so it can never trip a false positive. A dangling parent
        // is a projection bug, not content, so it panics.
        let file_id_str = file_id.as_str();
        for b in blocks {
            let parent = b.parent_id.as_str();
            if parent == b.id.as_str() {
                continue; // self-parented FK-anchor sentinel; never a real
                // block
            }
            if parent != file_id_str && !ids.contains(parent) {
                panic!(
                    "{}",
                    holon_api::ProjectionInvariantViolated {
                        detail: format!(
                            "org render: block {} has dangling parent {} (not the file root {} \
                             and not in the {}-block set)",
                            b.id.as_str(),
                            parent,
                            file_id_str,
                            blocks.len()
                        ),
                    }
                );
            }
        }
        // The only re-ordering the renderer imposes is a content-type grouping:
        // Source/Image children render before Text children (sub-headings) so a
        // re-parse re-attaches the source to this heading, not the next one. The
        // sort is stable, so input order is preserved within each group.
        for kids in children_by_parent.values_mut() {
            kids.sort_by_key(|b| b.content_type.sibling_order_group());
        }

        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut head_ended = false;
        if let Some(roots) = children_by_parent.get(file_id.as_str()) {
            for place in places(roots, file_id, 0) {
                if place.block.content_type == ContentType::Text && !head_ended {
                    end_with_blank_lines(&mut result, head_after);
                    head_ended = true;
                }
                Self::render_entity_tree(
                    place,
                    &children_by_parent,
                    &mut result,
                    &mut visited,
                    render_block,
                )?;
            }
        }

        if !head_ended {
            end_with_blank_lines(&mut result, head_after);
        }

        // WP-F projection assertion (free — `visited` is populated by the walk we
        // already performed): with dangling parents ruled out above, every real
        // block chains up to a file root, so every block must have been visited.
        // A block that was NOT visited is present with a present parent yet
        // unreachable from any root — the signature of a parent CYCLE (or a
        // disconnected component). Self-parented sentinel rows are skipped so
        // they cannot masquerade as a cycle.
        for b in blocks {
            if b.parent_id.as_str() == b.id.as_str() {
                continue; // self-parented FK-anchor sentinel
            }
            if !visited.contains(b.id.as_str()) {
                panic!(
                    "{}",
                    holon_api::ProjectionInvariantViolated {
                        detail: format!(
                            "org render: block {} (parent {}) is unreachable from file root {} \
                             despite its parent being present — parent cycle or disconnected \
                             component",
                            b.id.as_str(),
                            b.parent_id.as_str(),
                            file_id.as_str()
                        ),
                    }
                );
            }
        }

        Ok(result)
    }

    /// Render a block and its children recursively.
    fn render_entity_tree<'b, E, F: FnMut(&Block, Option<&str>) -> Result<String, E>>(
        place: Place<'b>,
        children_by_parent: &HashMap<&'b str, Vec<&'b Block>>,
        result: &mut String,
        visited: &mut std::collections::HashSet<&'b str>,
        render_block: &mut F,
    ) -> Result<(), E> {
        let block = place.block;
        // Record reachability for the WP-F cycle/disconnected-component assertion
        // in `render_entitys` — free, we are already walking every reachable node.
        visited.insert(block.id.as_str());
        if block.content_type == ContentType::Source {
            // source_block_to_org records the loss for an unreadable carrier.
            let before = block.blank_lines().unwrap_or_default().before_body;
            end_with_blank_lines(result, &before);
        }

        // Prepare block for org rendering - transfer Loro properties to org_props
        // format
        let mut prepared_block = block.clone();
        Self::prepare_block_for_org(&mut prepared_block, place.level);

        // Render via the caller-supplied per-block renderer (canonical
        // `Block::to_org` or the dense token form). Both guarantee a trailing
        // newline.
        result.push_str(&render_block(
            &prepared_block,
            place.minted_here.as_deref(),
        )?);

        // render_headline_block records the loss for an unreadable carrier.
        let blank_lines_after = prepared_block.blank_lines().unwrap_or_default().after;
        let mut section_ended = false;
        if let Some(kids) = children_by_parent.get(block.id.as_str()) {
            for child in places(kids, &block.id, place.level) {
                if child.block.content_type == ContentType::Text && !section_ended {
                    end_with_blank_lines(result, &blank_lines_after);
                    section_ended = true;
                }
                Self::render_entity_tree(child, children_by_parent, result, visited, render_block)?;
            }
        }
        if !section_ended {
            end_with_blank_lines(result, &blank_lines_after);
        }
        Ok(())
    }

    /// Drop a `task_state` this document's vocabulary does not declare from the
    /// render (the block is a CLONE; the store keeps its column), and return
    /// the loss.
    ///
    /// The same refusal the editable surface makes
    /// (`SourceProjection::Refused` / `Surface::Refused`): rendering `* TODO x`
    /// into a `#+TODO: NEXT | DONE` document emits bytes the next parse reads
    /// as ordinary title text, so the keyword would land in `content` and the
    /// file grow one more of them on every cold boot. Declaring the keyword in
    /// the document's `#+TODO:` line makes the state renderable.
    fn refuse_undeclared_task_state(
        block: &mut Block,
        vocabulary: &TaskKeywordVocabulary,
    ) -> Option<RenderLoss> {
        let state = block.task_state()?;
        if vocabulary.all_keywords().contains(&state.keyword) {
            return None;
        }
        block.set_task_state(None);
        let detail = format!(
            "its task state {:?} is not written: the file's `#+TODO:` keywords are {:?}",
            state.keyword,
            vocabulary.all_keywords()
        );
        tracing::warn!(target: "org.render", block = %block.id, "org render: {detail}");
        Some(RenderLoss {
            block: block.id.clone(),
            detail,
        })
    }

    /// Prepare a block for org rendering by transferring Loro properties to
    /// org_props format.
    ///
    /// Public so a caller that owns its own tree walk (the integration-test
    /// serializer) can reach `Block::to_org` through the SAME preparation
    /// write-back uses instead of re-deriving the drawer.
    pub fn prepare_block_for_org(block: &mut Block, level: i64) {
        let properties = block.properties_map();

        block.set_level(level);

        // Transfer TODO to task_state if not already set
        if block.task_state().is_none() {
            if let Some(todo) = properties.get("TODO").and_then(|v| v.as_string()) {
                block.set_task_state(Some(holon_api::TaskState::from_keyword(todo)));
            }
        }

        // Lift a legacy uppercase `PRIORITY` property into the typed field. The
        // org parser resolves both spellings itself, so this only ever sees
        // blocks built elsewhere; an unusable value is disclosed, never dropped
        // in silence.
        if block.priority().is_none() {
            if let Some(priority_val) = properties.get("PRIORITY") {
                match holon_api::Priority::try_from(priority_val.clone()) {
                    Ok(p) => block.set_priority(Some(p)),
                    Err(e) => tracing::warn!(
                        block = %block.id,
                        value = ?priority_val,
                        error = %e,
                        "stored PRIORITY property is not an org priority — the block is rendered \
                         without one"
                    ),
                }
            }
        }

        // Transfer TAGS to tags if not already set
        if block.tags().is_empty() {
            if let Some(tags) = properties.get("TAGS").and_then(|v| v.as_string()) {
                block.set_tags(holon_api::Tags::from_csv(tags));
            }
        }

        // Transfer SCHEDULED if not already set
        if block.scheduled().is_none() {
            if let Some(sched) = properties.get("SCHEDULED").and_then(|v| v.as_string()) {
                match holon_api::types::Timestamp::parse(sched) {
                    Ok(ts) => block.set_scheduled(Some(ts)),
                    Err(e) => {
                        tracing::warn!("Ignoring unparseable SCHEDULED property {sched:?}: {e}")
                    }
                }
            }
        }

        // Transfer DEADLINE if not already set
        if block.deadline().is_none() {
            if let Some(dead) = properties.get("DEADLINE").and_then(|v| v.as_string()) {
                match holon_api::types::Timestamp::parse(dead) {
                    Ok(ts) => block.set_deadline(Some(ts)),
                    Err(e) => {
                        tracing::warn!("Ignoring unparseable DEADLINE property {dead:?}: {e}")
                    }
                }
            }
        }

        // Reconstruct org_properties JSON when missing (after SQL round-trip,
        // flat properties like "ID" exist but the "org_properties" JSON key doesn't).
        // to_org() renders the :PROPERTIES: drawer exclusively from org_properties().
        if block.org_properties().is_none() {
            let id = properties.get("ID").and_then(|v| v.as_string());

            // Order drawer properties by the sequence the author wrote them
            // (recorded at parse in `_drawer_order`); keys the author never
            // wrote follow, alphabetically, for determinism.
            // serde_json::Map uses IndexMap (preserve_order feature is enabled
            // by a transitive dependency), so insertion order matters.
            // Exact spelling wins, so `:Effort:` and `:effort:` keep their own
            // slots; the case-insensitive probe then catches the lifted keys the
            // renderer re-spells (`:collapsed:` authored, `COLLAPSED` emitted).
            // An unreadable order carrier leaves the drawer in alphabetical order.
            let authored = block.authored_drawer_order().unwrap_or_default();
            let rank = |key: &str| {
                authored
                    .iter()
                    .position(|k| k == key)
                    .or_else(|| authored.iter().position(|k| k.eq_ignore_ascii_case(key)))
                    .unwrap_or(usize::MAX)
            };
            let mut drawer_props: Vec<_> = block.drawer_properties().into_iter().collect();
            drawer_props.sort_by(|(a, _), (b, _)| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));

            let mut org_props = serde_json::Map::new();
            if let Some(id) = id {
                org_props.insert("ID".to_string(), serde_json::Value::String(id.to_string()));
            }
            for (k, v) in drawer_props {
                org_props.insert(k, serde_json::Value::String(v));
            }
            let json = serde_json::to_string(&org_props)
                .expect("drawer properties must serialize to JSON");
            block.set_org_properties(Some(json));
        }
    }
}

/// Records a loss when the written file reads back with another page id,
/// title or task keywords than `doc_block`, or with other header lines, in
/// another order or place, than the renderer meant to write (`header`).
fn check_header_reads_back(
    doc_block: &Block,
    header: &[KeywordLine],
    read: &Block,
    losses: &mut Vec<RenderLoss>,
) {
    let lines = |lines: &[KeywordLine]| -> Vec<(String, usize)> {
        lines
            .iter()
            .map(|l| (l.raw.trim().replace('\r', ""), l.before_line))
            .collect()
    };
    let declared = |block: &Block| -> Vec<(String, bool, bool)> {
        block
            .todo_keywords()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.is_active() || s.is_done())
            .map(|s| (s.keyword.clone(), s.is_active(), s.is_done()))
            .collect()
    };
    let id_differs = !doc_block.id.is_file() && read.id != doc_block.id;
    let read_lines = lines(&read.header_keyword_lines().unwrap_or_default());
    if !id_differs
        && read.file_title() == doc_block.file_title()
        && declared(read) == declared(doc_block)
        && read_lines == lines(header)
    {
        return;
    }
    let detail = format!(
        "the page header reads back as id {}, title {:?}, task keywords {:?}, lines {:?}; \
         the page holds id {}, title {:?}, task keywords {:?}, and the render meant lines {:?}",
        read.id,
        read.file_title(),
        read.todo_keywords(),
        read_lines,
        doc_block.id,
        doc_block.file_title(),
        doc_block.todo_keywords(),
        lines(header),
    );
    tracing::warn!(page = %doc_block.id, "org render: {detail}");
    losses.push(RenderLoss {
        block: doc_block.id.clone(),
        detail,
    });
}

/// Ends `text`, which is empty or ends with a line break, with the blank lines
/// `blank_lines` holds, byte for byte. Blank lines the renderer already wrote
/// there (a list body's closing one) are replaced, and kept where `text` has
/// more of them.
fn end_with_blank_lines(text: &mut String, blank_lines: &[String]) {
    let present = trailing_blank_lines(text);
    text.truncate(text.len() - present);
    for i in 0..present.max(blank_lines.len()) {
        text.push_str(blank_lines.get(i).map_or("", String::as_str));
        text.push('\n');
    }
}

/// The number of empty lines at the end of `text`, which the renderer writes as
/// bare line breaks after the last line.
fn trailing_blank_lines(text: &str) -> usize {
    let content = text.trim_end_matches('\n');
    (text.len() - content.len()).saturating_sub(usize::from(!content.is_empty()))
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;
    use holon_api::Value;
    use holon_api::types::ContentType;
    use holon_api::types::SourceLanguage;

    use super::*;

    fn test_doc_uri() -> EntityUri {
        EntityUri::file("/test/file.org")
    }

    fn test_source_block(id: &str, parent_id: &str, lang: &str, content: &str, seq: i64) -> Block {
        use holon_orgmode_models::OrgBlockExt;
        let mut b = Block {
            id: EntityUri::block(id),
            parent_id: EntityUri::block(parent_id),
            tags: Vec::new().into(),
            requires: Vec::new(),
            content: content.to_string(),
            content_type: ContentType::Source,
            source_language: Some(lang.parse::<SourceLanguage>().unwrap()),
            source_name: None,
            properties: HashMap::new(),
            marks: None,
            created_at: 0,
            updated_at: 0,
            ..Default::default()
        };
        b.set_sequence(seq);
        b
    }

    use crate::models as holon_orgmode_models;

    #[test]
    fn test_render_simple_block() {
        let mut block = Block::new_text(
            EntityUri::block("test-uuid"),
            test_doc_uri(),
            "Test Title\nBody content here",
        );
        block.set_property("ID", Value::String("test-uuid".to_string()));

        let file_path = Path::new("/test/file.org");
        let org_text = OrgRenderer::render_entitys(&[block], file_path, &test_doc_uri())
            .expect("org render")
            .text;

        assert!(org_text.contains("* Test Title"));
        assert!(org_text.contains("Body content here"));
        assert!(org_text.contains(":ID: test-uuid"));
    }

    // WP-F: dangling parent must fail loud, not silently drop the block.
    #[test]
    #[should_panic(expected = "projection invariant violated")]
    fn wpf_dangling_parent_panics() {
        let doc = test_doc_uri();
        // Block whose parent is neither the file root nor any block in the set.
        let mut orphan = Block::new_text(
            EntityUri::block("orphan"),
            EntityUri::block("ghost-parent-not-in-set"),
            "Orphan",
        );
        orphan.set_property("ID", Value::String("orphan".to_string()));
        let _ = OrgRenderer::render_entitys(&[orphan], Path::new("/test/file.org"), &doc)
            .expect("org render")
            .text;
    }

    // WP-F: a parent cycle (present parents, unreachable from the file root)
    // must fail loud rather than silently dropping the whole component.
    #[test]
    #[should_panic(expected = "projection invariant violated")]
    fn wpf_parent_cycle_panics() {
        let doc = test_doc_uri();
        let mut a = Block::new_text(EntityUri::block("a"), EntityUri::block("b"), "A");
        a.set_property("ID", Value::String("a".to_string()));
        let mut b = Block::new_text(EntityUri::block("b"), EntityUri::block("a"), "B");
        b.set_property("ID", Value::String("b".to_string()));
        let _ = OrgRenderer::render_entitys(&[a, b], Path::new("/test/file.org"), &doc)
            .expect("org render")
            .text;
    }

    // WP-F guard against false positives: a normal tree (roots parented to the
    // file root, children parented to present blocks) renders without panicking.
    #[test]
    fn wpf_normal_tree_does_not_panic() {
        let doc = test_doc_uri();
        let mut root = Block::new_text(EntityUri::block("root"), doc.clone(), "Root");
        root.set_property("ID", Value::String("root".to_string()));
        let mut child =
            Block::new_text(EntityUri::block("child"), EntityUri::block("root"), "Child");
        child.set_property("ID", Value::String("child".to_string()));
        let out = OrgRenderer::render_entitys(&[root, child], Path::new("/test/file.org"), &doc)
            .expect("org render")
            .text;
        assert!(out.contains("Root"));
        assert!(out.contains("Child"));
    }

    // WP-F: a self-parented row (the filtered-out `sentinel:no_parent` FK anchor
    // shape) must NOT trip the dangling or cycle assertion.
    #[test]
    fn wpf_self_parented_sentinel_does_not_panic() {
        let doc = test_doc_uri();
        let mut root = Block::new_text(EntityUri::block("root"), doc.clone(), "Root");
        root.set_property("ID", Value::String("root".to_string()));
        // Self-parented sentinel-shaped row (id == parent_id) alongside a normal
        // root — the same self-parent shape as the filtered `sentinel:no_parent`.
        let mut sentinel = Block::new_text(
            EntityUri::block("selfanchor"),
            EntityUri::block("selfanchor"),
            "Sentinel",
        );
        sentinel.set_property("ID", Value::String("selfanchor".to_string()));
        let out = OrgRenderer::render_entitys(&[root, sentinel], Path::new("/test/file.org"), &doc)
            .expect("org render")
            .text;
        assert!(out.contains("Root"));
    }

    #[test]
    fn test_render_entity_with_todo_and_priority() {
        let mut block =
            Block::new_text(EntityUri::block("test-id"), test_doc_uri(), "Task headline");
        block.set_property("ID", Value::String("test-id".to_string()));
        block.set_property("TODO", Value::String("TODO".to_string()));
        block.set_property("PRIORITY", Value::String("A".to_string()));

        let file_path = Path::new("/test/file.org");
        let org_text = OrgRenderer::render_entitys(&[block], file_path, &test_doc_uri())
            .expect("org render")
            .text;

        assert!(org_text.contains("* TODO [#A] Task headline"));
    }

    #[test]
    fn test_source_blocks_render_before_child_headlines() {
        let doc = test_doc_uri();

        let mut parent =
            Block::new_text(EntityUri::block("parent-id"), doc.clone(), "Parent Heading");
        parent.set_property("ID", Value::String("parent-id".to_string()));

        let mut child_heading = Block::new_text(
            EntityUri::block("child-heading-id"),
            EntityUri::block("parent-id"),
            "Child Heading",
        );
        child_heading.set_property("ID", Value::String("child-heading-id".to_string()));

        let source_block =
            test_source_block("src-id", "parent-id", "holon_prql", "from tasks\n", 1);

        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, child_heading, source_block];
        let org_text = OrgRenderer::render_entitys(&blocks, file_path, &test_doc_uri())
            .expect("org render")
            .text;

        let src_pos = org_text
            .find("#+BEGIN_SRC")
            .expect("source block must be present");
        let child_pos = org_text
            .find("** Child Heading")
            .expect("child heading must be present");

        assert!(
            src_pos < child_pos,
            "Source block must render BEFORE child heading.\nOutput:\n{}",
            org_text
        );
    }

    #[test]
    fn test_multiple_source_blocks_all_before_children() {
        let doc = test_doc_uri();

        let mut parent = Block::new_text(EntityUri::block("parent-id"), doc.clone(), "Parent");
        parent.set_property("ID", Value::String("parent-id".to_string()));

        let src1 = test_source_block("src1", "parent-id", "holon_sql", "SELECT 1;\n", 1);
        let src2 = test_source_block("src2", "parent-id", "holon_prql", "from users\n", 2);

        let mut child = Block::new_text(
            EntityUri::block("child-id"),
            EntityUri::block("parent-id"),
            "Child",
        );
        child.set_property("ID", Value::String("child-id".to_string()));

        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, child, src1, src2];
        let org_text = OrgRenderer::render_entitys(&blocks, file_path, &test_doc_uri())
            .expect("org render")
            .text;

        let src1_pos = org_text
            .find("#+BEGIN_SRC holon_sql")
            .expect("holon_sql block");
        let src2_pos = org_text
            .find("#+BEGIN_SRC holon_prql")
            .expect("holon_prql block");
        let child_pos = org_text.find("** Child").expect("child heading");

        assert!(
            src1_pos < child_pos && src2_pos < child_pos,
            "All source blocks must render before child heading.\nOutput:\n{}",
            org_text
        );
    }

    #[test]
    fn test_source_block_ordering_with_interleaved_input() {
        let doc = test_doc_uri();

        let mut parent = Block::new_text(EntityUri::block("p"), doc.clone(), "Root");
        parent.set_property("ID", Value::String("p".to_string()));

        let mut text_child =
            Block::new_text(EntityUri::block("t1"), EntityUri::block("p"), "Sub Heading");
        text_child.set_property("ID", Value::String("t1".to_string()));

        let src_child = test_source_block("s1", "p", "python", "print('hi')\n", 10);

        // Deliberately put text_child before src_child in the input vec
        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, text_child, src_child];
        let org_text = OrgRenderer::render_entitys(&blocks, file_path, &test_doc_uri())
            .expect("org render")
            .text;

        let src_pos = org_text.find("#+BEGIN_SRC python").expect("source block");
        let sub_pos = org_text.find("** Sub Heading").expect("sub heading");

        assert!(
            src_pos < sub_pos,
            "Source block must come first regardless of input order.\nOutput:\n{}",
            org_text
        );
    }
}
