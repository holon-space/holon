//! Pure planner for `dense_patch`: given a captured [`Projection`] and the
//! agent-edited dense text (parsed by [`holon_org_format::parse_dense`]), it
//! computes a typed [`PatchPlan`] — the batch of block operations, plus the set
//! of concurrency tokens to verify — WITHOUT touching any engine. The engine
//! applier (in the MCP tool) and the PBT both consume the same plan.
//!
//! Structure is diffed RELATIVE to the projection (Martin ruling 2026-07-23):
//! a block emits a move ONLY when its enclosing rendered block or its order
//! relative to its surviving siblings actually changed in the edited text. A
//! re-rooted / gap-marked block that the agent left in place emits no move —
//! the `{#alias^}` marker is display-only and carries no semantic weight. New
//! blocks (rows with no `{#alias}`) are created at their tree position; blocks
//! omitted from the text are NOT deleted (deletion is explicit via
//! `delete_aliases`).
//!
//! A row's content (title line and body), task state, tags (the headline tag
//! group) and `:PROPERTIES:` drawer are written: a new row is created with
//! them, and an existing row's differences from the projection become writes,
//! its state before its content so the engine never reads a new title as a
//! keyword to converge. A drawer line removed from an existing row removes
//! that property. The drawer line order is written as the authored order
//! whenever org would otherwise render the lines in another order, and a
//! row's keyword lines (`#+CAPTION: x`) are written in their place, both as
//! the org parser read them from the text ([`ParsedCarrier`]). An edited
//! row is planned only when org writes it back as it stands; any other edit,
//! and any text the grammar cannot place, refuses the patch, naming the row.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::Result;
use anyhow::bail;
use holon_api::EdgeField;
use holon_api::EntityUri;
use holon_api::MarkSpan;
use holon_api::Tags;
use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::types::TaskState;
use holon_org_format::Alias;
use holon_org_format::AuthoredKey;
use holon_org_format::DenseBlock;
use holon_org_format::DenseParse;
use holon_org_format::KeptCarriers;
use holon_org_format::OrgBlockExt;
use holon_org_format::ParsedCarrier;
use holon_org_format::TaskKeywordVocabulary;
use holon_org_format::ValueCarrier;
use holon_org_format::drawer_key_order;
use holon_org_format::models::KeywordLine;
use holon_org_format::models::is_hidden_drawer_key;

use crate::dense_projection::BlockVersion;
use crate::dense_projection::Projection;
use crate::dense_projection::SYNTHETIC_ROOT;
use crate::dense_projection::preamble;

/// A reference to a block that may not have a real id yet (a not-yet-created
/// new block). The engine/model applier resolves `New` to a minted id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ref {
    /// The projection render root (`file_id`) — a top-level position.
    Root,
    /// An existing block.
    Existing(EntityUri),
    /// A NEW block, identified by its row index in the parsed patch.
    New(usize),
}

/// One typed operation in a patch plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchOp {
    Create {
        temp: usize,
        parent: Ref,
        after: Option<Ref>,
        /// Title line and body.
        content: RowContent,
        task_state: Option<TaskState>,
        attributes: RowAttributes,
        /// The row's parser carriers org would not render unaided.
        carriers: Vec<ParsedCarrier>,
    },
    /// Replace the whole content: title line and body.
    SetContent {
        block_id: EntityUri,
        content: RowContent,
    },
    SetState {
        block_id: EntityUri,
        task_state: Option<TaskState>,
    },
    SetTags {
        block_id: EntityUri,
        tags: Tags,
    },
    /// `value: None` removes the property.
    SetProperty {
        block_id: EntityUri,
        key: AuthoredKey,
        value: Option<String>,
    },
    /// Replace a parser carrier with the one the row's text gives.
    SetCarrier {
        block_id: EntityUri,
        carrier: ParsedCarrier,
    },
    Move {
        block_id: EntityUri,
        parent: Ref,
        after: Option<Ref>,
    },
    /// Delete the block with its whole subtree.
    Delete {
        block_id: EntityUri,
    },
}

/// The tags and drawer properties of a dense row that `dense_patch` writes.
/// Drawer keys the org parser lifts into typed fields (edge fields,
/// `COLLAPSED`, `WIDGET_ONLY`, priority) are not among the properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowAttributes {
    pub tags: Tags,
    pub properties: BTreeMap<String, String>,
}

impl RowAttributes {
    pub fn of(block: &Block) -> RowAttributes {
        let properties = block
            .drawer_properties()
            .into_iter()
            .filter(|(key, _)| holon_org_format::TypedDrawerKey::parse(key).is_none())
            .collect();
        RowAttributes {
            tags: block.tags(),
            properties,
        }
    }
}

/// A dense row as the org parser reads it. An existing row is edited exactly
/// when its view differs from the one its projection text parses to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowView {
    pub title: String,
    pub body: Option<String>,
    pub task_state: Option<TaskState>,
    pub attributes: RowAttributes,
    pub uncarried: UncarriedFields,
    pub drawer: Vec<(String, String)>,
    pub keyword_lines: Vec<KeywordLine>,
}

impl RowView {
    pub fn of(row: &DenseBlock) -> Result<RowView> {
        Ok(RowView {
            title: row.block.org_title(),
            body: row.block.body(),
            task_state: row.block.task_state(),
            attributes: RowAttributes::of(&row.block),
            uncarried: UncarriedFields::of(&row.block),
            drawer: row.drawer.clone(),
            keyword_lines: row.block.keyword_lines()?,
        })
    }
}

impl RowView {
    /// The drawer keys in line order.
    fn drawer_keys(&self) -> Vec<String> {
        self.drawer.iter().map(|(k, _)| k.clone()).collect()
    }
}

/// Whether org renders a drawer holding `keys` under `authored` in another
/// order.
fn reorders(authored: &[String], keys: Vec<String>) -> bool {
    drawer_key_order(authored, keys.clone()) != keys
}

/// The parser carriers of `row` to write: its drawer order when org renders
/// its drawer in another order under `authored`, and its keyword lines when
/// they are not `shown`'s (`None`: a new row).
fn carriers_to_write(
    row: &DenseBlock,
    view: &RowView,
    shown: Option<&RowView>,
    authored: &[String],
) -> Vec<ParsedCarrier> {
    let mut carriers = Vec::new();
    if reorders(authored, view.drawer_keys()) {
        carriers.push(row.drawer_order.clone());
    }
    if view.keyword_lines != shown.map(|s| s.keyword_lines.clone()).unwrap_or_default() {
        carriers.push(row.keyword_lines.clone());
    }
    carriers
}

/// Refuse a row whose written text holds a keyword line org reads for the
/// whole page and the projection did not show for it (`shown`). org-get-title
/// reads `#+TITLE:` inside every block, so the check sees inside blocks.
fn refuse_page_keywords(
    row: &DenseBlock,
    todo_keywords: Option<&[TaskState]>,
    written: &Written,
    shown: &[String],
    label: &str,
) -> Result<()> {
    let lines = row.rendered_lines(todo_keywords, written.kept, written.carriers)?;
    if let Some(line) = lines
        .iter()
        .filter(|line| !shown.contains(line))
        .map(|line| line.trim())
        .find(|line| {
            holon_org_format::declares_page_keyword(line)
                && !holon_org_format::is_page_id_keyword(line)
        })
    {
        bail!(
            "{label}: org reads the line {line:?} as a keyword of the whole page, not as this \
             row's text, so it would not be written here — write it as `,{line}` to keep it as \
             text"
        );
    }
    Ok(())
}

/// How an error names a patch row.
fn row_label(db: &DenseBlock) -> String {
    match &db.alias {
        Some(alias) => format!("row {{#{alias}}}"),
        None => format!("new row {:?}", db.block.org_title()),
    }
}

/// How a store holds a row once a plan wrote it: the carriers it kept, and
/// the parser carriers the plan writes.
struct Written<'a> {
    kept: &'a KeptCarriers,
    carriers: &'a [ParsedCarrier],
}

/// `org-special-properties`: org derives each from the entry, in any case.
const ORG_SPECIAL_PROPERTIES: &[&str] = &[
    "ALLTAGS",
    "BLOCKED",
    "CLOCKSUM",
    "CLOCKSUM_T",
    "CLOSED",
    "DEADLINE",
    "FILE",
    "ITEM",
    "PRIORITY",
    "SCHEDULED",
    "TAGS",
    "TIMESTAMP",
    "TIMESTAMP_IA",
    "TODO",
];

/// Refuse a row whose edited headline, drawer lines, keyword lines or body
/// org would not write back as they stand. `baseline` holds the drawer lines
/// the projection already showed for the row; an unedited body keeps its
/// stored bytes, so it is not checked. `todo_keywords` are the ones the edited
/// text declares.
fn refuse_inexact(
    row: &DenseBlock,
    todo_keywords: Option<&[TaskState]>,
    written: &Written,
    baseline: &[(String, String)],
    headline_changed: bool,
    body_changed: bool,
    label: &str,
) -> Result<()> {
    for (i, (key, _)) in row.drawer.iter().enumerate() {
        for (other, _) in &row.drawer[..i] {
            if other == key {
                bail!("{label}: drawer key `{key}` appears more than once; a key holds one value");
            }
            let edited = || {
                row.drawer
                    .iter()
                    .any(|line| (&line.0 == key || &line.0 == other) && !baseline.contains(line))
            };
            if other.to_lowercase() == key.to_lowercase() && edited() {
                bail!(
                    "{label}: drawer keys `:{other}:` and `:{key}:` are one org property, as org \
                     reads a key in any case, so the drawer cannot hold both"
                );
            }
        }
    }
    for (key, _) in row.drawer.iter().filter(|line| !baseline.contains(line)) {
        if let Err(e) = ValueCarrier::HeadlineDrawer.key(key) {
            bail!("{label}: {e}");
        }
        if is_hidden_drawer_key(key) {
            bail!(
                "{label}: drawer key `{key}` names a Holon field, which org never writes as a property"
            );
        }
        if ORG_SPECIAL_PROPERTIES
            .iter()
            .any(|special| key.eq_ignore_ascii_case(special))
        {
            bail!("{label}: org computes `:{key}:` and never reads it from a drawer");
        }
        if holon_api::schema::is_block_column(key)
            || key == holon_api::entity::POSITION_AFTER_BLOCK_ID_PARAM
        {
            bail!("{label}: drawer key `{key}` names a block field, not a property — rename it");
        }
    }
    let title = row.block.org_title();
    if headline_changed {
        if let Some((tag, c)) = unparsed_tag(&title) {
            bail!(
                "{label}: tag {tag:?} holds {c:?}, which an org tag group cannot represent, so \
                 org reads `:{tag}:` as title text"
            );
        }
        for tag in row.block.tags().iter() {
            if let Some(c) = Tags::unrepresentable_char(tag) {
                bail!("{label}: tag {tag:?} holds {c:?}, which an org tag group cannot represent");
            }
        }
    }
    let back = row.rendered_back(todo_keywords, written.kept, written.carriers)?;
    if let Some((key, value)) = row.drawer.iter().find(|line| !back.drawer.contains(line)) {
        bail!(
            "{label}: org does not write the drawer line `:{key}: {value}` back as a property; \
             its drawer reads back as {:?}",
            back.drawer
        );
    }
    let (back_title, back_tags) = (back.block.org_title(), back.block.tags());
    if back_title != title || back_tags != row.block.tags() {
        bail!("{label}: org writes this headline back as {back_title:?} with tags {back_tags:?}");
    }
    let lines = row.block.keyword_lines()?;
    let in_file = row
        .file_keyword_lines(written.kept, written.carriers)
        .map_err(|e| anyhow::anyhow!("{label}: org cannot write this row to its file: {e:#}"))?;
    for back_lines in [back.block.keyword_lines()?, in_file] {
        if back_lines != lines {
            bail!(
                "{label}: org writes the keyword lines of this row back as {:?}, not as {:?}",
                back_lines.iter().map(|l| &l.raw).collect::<Vec<_>>(),
                lines.iter().map(|l| &l.raw).collect::<Vec<_>>()
            );
        }
    }
    if !body_changed {
        return Ok(());
    }
    if let Some(marker) = row.block.body().and_then(|body| {
        body.lines()
            .map(str::trim)
            .find(|l| l.eq_ignore_ascii_case(":PROPERTIES:") || l.eq_ignore_ascii_case(":END:"))
            .map(str::to_string)
    }) {
        bail!(
            "{label}: its body holds `{marker}`, so org did not read the drawer as a property \
             drawer — it needs `:PROPERTIES:` right under the headline, one `:key: value` line \
             each, and `:END:`"
        );
    }
    if back.block.body() != row.block.body() {
        bail!(
            "{label}: org writes this body back as {:?}, not as {:?}",
            back.block.body(),
            row.block.body()
        );
    }
    Ok(())
}

/// Refuse a row whose text, as the agent wrote it (`raw`), is not the text org
/// writes for the row it reads: a line org takes as another element (a
/// keyword, a block) would be lost or rewritten. A `#+` line is text only when
/// written `,#+` (D230.a).
fn refuse_unread_lines(
    row: &DenseBlock,
    raw: &str,
    todo_keywords: Option<&[TaskState]>,
    store: &Written,
    label: &str,
) -> Result<()> {
    let written = row.rendered_lines(todo_keywords, store.kept, store.carriers)?;
    let wrote = holon_org_format::row_lines(raw)?;
    if written == wrote {
        return Ok(());
    }
    let at = wrote
        .iter()
        .zip(&written)
        .position(|(a, b)| a != b)
        .unwrap_or(written.len().min(wrote.len()));
    match (wrote.get(at), written.get(at)) {
        (Some(line), Some(org)) if org.trim_start_matches(',') == line.trim_start_matches(',') => {
            bail!(
                "{label}: org writes the line {line:?} of this row as `{org}` — a `#+` line is \
                 text only when written `,#+`, and inside an example, export or source block \
                 org removes the comma — write it as `{org}`"
            )
        }
        (Some(line), _) if line.trim_start().starts_with("#+") => bail!(
            "{label}: org reads the line {line:?} as an org keyword, not as this row's text, so \
             it would not be written — write it as `,{}` to keep it as text",
            line.trim_start()
        ),
        (Some(line), _) => bail!(
            "{label}: org does not read the line {line:?} back as this row's text; it writes the \
             row as {written:?}"
        ),
        (None, _) => {
            bail!("{label}: org writes this row as {written:?}, not as the text {wrote:?}")
        }
    }
}

/// The most org emphasis marks (`*` `/` `_` `+` `=` `~`) a written row's text
/// may hold. Org's read of a line grows faster than linearly in its marks.
pub const MAX_EMPHASIS_MARKS_PER_ROW: usize = 100;

/// The most rows one patch may write, a moved or deleted row included.
pub const MAX_WRITTEN_ROWS_PER_PATCH: usize = 40;

/// The most store rows the rows one patch creates or moves may land among:
/// each such row counts the rows its new parent holds. Placing a row costs
/// time in its siblings, files or none.
pub const MAX_SIBLING_WORK_PER_PATCH: usize = 20_000;

/// The most rows one patch text may hold, written or not.
pub const MAX_ROWS_PER_TEXT: usize = 1000;

/// The most blocks the org documents one patch writes into may hold
/// together. Each write and the check of the documents renders them whole.
pub const MAX_DOCUMENT_BLOCKS_PER_PATCH: usize = 2000;

/// Refuse a text holding a carriage return, naming the row of the first one.
/// Org write-back gives a file one line ending, so a CR in a row's text could
/// not be written as the agent wrote it.
fn refuse_carriage_returns(text: &str) -> Result<()> {
    let mut row = None;
    let mut in_preamble = None;
    for line in text.split('\n') {
        if holon_org_format::is_headline(line.trim_end_matches('\r')) {
            row = Some(headline_row_name(line.trim_end_matches('\r'))?);
        }
        if !line.contains('\r') {
            continue;
        }
        match &row {
            Some(row) => bail!(
                "{row}: the line {line:?} holds a carriage return (CRLF); a dense text is \
                 LF-only, as write-back gives the org file one line ending — send `\\n` line \
                 breaks"
            ),
            None => in_preamble = in_preamble.or(Some(line)),
        }
    }
    if let Some(line) = in_preamble {
        bail!(
            "the page header: the line {line:?} holds a carriage return (CRLF); a dense text is \
             LF-only — send `\\n` line breaks"
        );
    }
    Ok(())
}

/// How a refusal names the row a headline line starts.
fn headline_row_name(headline: &str) -> Result<String> {
    Ok(match holon_org_format::row_alias(headline)? {
        Some(alias) => format!("row {{#{alias}}}"),
        None => format!("new row {headline:?}"),
    })
}

/// Refuse a text over a work bound, before any parser reads it: the row at
/// which the text passes [`MAX_ROWS_PER_TEXT`], a row over
/// [`MAX_EMPHASIS_MARKS_PER_ROW`], or the row at which the written rows pass
/// [`MAX_WRITTEN_ROWS_PER_PATCH`]. A row whose lines are those dense_query
/// showed for it is not written.
fn refuse_heavy_patch(projection: &Projection, text: &str) -> Result<()> {
    let marks = |line: &str| line.chars().filter(|c| "*/_+=~".contains(*c)).count();
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (at, line) in lines.iter().enumerate() {
        if holon_org_format::is_headline(line) {
            rows.push((at, at + 1));
        } else if let Some(row) = rows.last_mut() {
            row.1 = at + 1;
        }
    }
    if let Some(&(start, _)) = rows.get(MAX_ROWS_PER_TEXT) {
        bail!(
            "{}: this is row {} of the text; dense_patch reads at most {MAX_ROWS_PER_TEXT} rows \
             per text — query a smaller subtree",
            headline_row_name(lines[start])?,
            MAX_ROWS_PER_TEXT + 1
        );
    }
    let mut written = 0;
    for (start, end) in rows {
        let row = &lines[start..end];
        let shown = holon_org_format::row_alias(row[0])?
            .and_then(|alias| projection.alias_table.id_of(&alias))
            .map(|id| &projection.records[id.as_str()].shown_lines);
        if let Some(shown) = shown {
            if holon_org_format::row_lines(&row.join("\n"))? == *shown {
                continue;
            }
        }
        let count = marks(row[0].trim_start_matches('*'))
            + row[1..].iter().map(|line| marks(line)).sum::<usize>();
        written += 1;
        let name = || headline_row_name(row[0]);
        if count > MAX_EMPHASIS_MARKS_PER_ROW {
            bail!(
                "{}: its text holds {count} emphasis marks (`*` `/` `_` `+` `=` `~`); dense_patch \
                 reads at most {MAX_EMPHASIS_MARKS_PER_ROW} per written row, since org's read of \
                 them grows faster than linearly — write fewer marks or spread the text over more \
                 rows",
                name()?
            );
        }
        if written > MAX_WRITTEN_ROWS_PER_PATCH {
            bail!(
                "{}: this is written row {written}; one dense_patch writes at most \
                 {MAX_WRITTEN_ROWS_PER_PATCH} rows — send the rows from this one on in another \
                 patch",
                name()?
            );
        }
    }
    Ok(())
}

/// `state` as the row's own document classifies it, or a refusal when that
/// document does not declare the keyword: org would not write it to the file.
fn own_keyword(
    state: &TaskState,
    vocabulary: &TaskKeywordVocabulary,
    label: &str,
) -> Result<TaskState> {
    if !vocabulary.all_keywords().contains(&state.keyword) {
        bail!(
            "{label}: `{}` is not a task keyword of this row's document, which declares {} | {} — \
             org would not write it to the file",
            state.keyword,
            vocabulary.active_keywords().join(" "),
            vocabulary.done_keywords().join(" ")
        );
    }
    Ok(crate::dense_projection::state_in(
        vocabulary,
        &state.keyword,
    ))
}

/// The first `{#alias}`-shaped run in `text`.
fn token_shaped(text: &str) -> Option<&str> {
    text.match_indices("{#").find_map(|(start, _)| {
        let inner = &text[start + 2..];
        let end = inner.find('}')?;
        let alias = inner[..end].strip_suffix('^').unwrap_or(&inner[..end]);
        // A run that is not an alias is ordinary text, not a refusal.
        Alias::parse(alias)
            .is_ok()
            .then(|| &text[start..start + 2 + end + 1])
    })
}

/// Refuse a row whose title or body holds token-shaped text the shown row did
/// not: a token belongs at the end of the headline line, anywhere else it is a
/// token the agent misplaced.
fn refuse_misplaced_token(view: &RowView, shown: Option<&RowView>, label: &str) -> Result<()> {
    for (part, text) in [("title", Some(&view.title)), ("body", view.body.as_ref())] {
        let Some(text) = text else { continue };
        let Some(token) = token_shaped(text) else {
            continue;
        };
        let shown_text = shown.map(|s| format!("{}\n{}", s.title, s.body.as_deref().unwrap_or("")));
        if shown_text.is_some_and(|t| t.contains(token)) {
            continue;
        }
        bail!(
            "{label}: its {part} holds `{token}`, which is not the row's trailing token — a \
             token ends the headline line, after the tag group and a space"
        );
    }
    Ok(())
}

/// A row's content as the store holds it: the text, and the inline marks that
/// index it by Unicode scalar offset. A plan writes it as parsed, so the store
/// never reads it as org source again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowContent {
    pub text: String,
    pub marks: Vec<MarkSpan>,
}

impl RowContent {
    pub fn of(block: &Block) -> RowContent {
        RowContent {
            text: block.content.clone(),
            marks: block.marks.clone().unwrap_or_default(),
        }
    }

    /// The title line and the body, or `None` when a mark spans the line
    /// break between them.
    fn parts(&self) -> Option<(RowContent, Option<RowContent>)> {
        let Some((title, body)) = self.text.split_once('\n') else {
            return Some((self.clone(), None));
        };
        let cut = title.chars().count() + 1;
        let mut title_marks = Vec::new();
        let mut body_marks = Vec::new();
        for span in &self.marks {
            if span.end < cut {
                title_marks.push(span.clone());
            } else if span.start >= cut {
                body_marks.push(MarkSpan::new(
                    span.start - cut,
                    span.end - cut,
                    span.mark.clone(),
                ));
            } else {
                return None;
            }
        }
        Some((
            RowContent {
                text: title.to_string(),
                marks: title_marks,
            },
            Some(RowContent {
                text: body.to_string(),
                marks: body_marks,
            }),
        ))
    }

    fn join(title: RowContent, body: Option<RowContent>) -> RowContent {
        let Some(body) = body else {
            return title;
        };
        let cut = title.text.chars().count() + 1;
        let mut marks = title.marks;
        marks.extend(
            body.marks
                .into_iter()
                .map(|span| MarkSpan::new(span.start + cut, span.end + cut, span.mark)),
        );
        RowContent {
            text: format!("{}\n{}", title.text, body.text),
            marks,
        }
    }

    /// `self`, the stored content, with its title line replaced by `parsed`'s
    /// when the headline changed and its body when the body changed; an
    /// untouched part keeps its stored text and marks. When a mark spans the
    /// line break in either, the whole parsed content is written.
    fn edited(
        &self,
        parsed: &RowContent,
        headline_changed: bool,
        body_changed: bool,
    ) -> RowContent {
        match (self.parts(), parsed.parts()) {
            (Some((title, body)), Some((new_title, new_body))) => RowContent::join(
                if headline_changed { new_title } else { title },
                if body_changed { new_body } else { body },
            ),
            _ => parsed.clone(),
        }
    }
}

/// The first tag of a trailing `:tag:…:` run that org reads as title text
/// because the tag holds a character an org tag cannot, with that character.
fn unparsed_tag(title: &str) -> Option<(&str, char)> {
    let group = title.strip_suffix(':')?;
    let start = group.rfind(" :")? + 2;
    group[start..]
        .split(':')
        .find_map(|tag| Tags::unrepresentable_char(tag).map(|c| (tag, c)))
}

/// The fields of a row that `dense_patch` does not write, keyed by the name a
/// refusal reports. Only the fields the row carries are present.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UncarriedFields(BTreeMap<String, String>);

impl UncarriedFields {
    pub fn of(block: &Block) -> UncarriedFields {
        let mut fields = BTreeMap::new();
        let mut put = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                fields.insert(name.to_string(), value);
            }
        };
        put("priority", block.priority().map(|p| p.letter().to_string()));
        put("SCHEDULED", block.scheduled().map(|t| t.to_string()));
        put("DEADLINE", block.deadline().map(|t| t.to_string()));
        put("COLLAPSED", block.collapsed.then(|| "t".to_string()));
        put("WIDGET_ONLY", block.widget_only.then(|| "t".to_string()));
        for field in EdgeField::ALL {
            if field != EdgeField::Tags && !field.is_empty(block) {
                put(
                    &field.column().to_uppercase(),
                    Some(format!("{:?}", field.param_value(block))),
                );
            }
        }
        UncarriedFields(fields)
    }

    /// The field names whose values differ from `baseline`.
    fn changed_from<'a>(&'a self, baseline: &'a UncarriedFields) -> Vec<&'a str> {
        let names: BTreeSet<&String> = self.0.keys().chain(baseline.0.keys()).collect();
        names
            .into_iter()
            .filter(|name| self.0.get(*name) != baseline.0.get(*name))
            .map(String::as_str)
            .collect()
    }

    /// Refuse the row when it differs from `baseline` in a field no op writes.
    /// A new row's baseline is the empty set.
    fn refuse_changes(&self, baseline: &UncarriedFields, row: &str) -> Result<()> {
        let changed = self.changed_from(baseline);
        if !changed.is_empty() {
            bail!(
                "{row}: dense_patch does not write {} — the edit would be ignored. Revert it here \
                 and make it with execute_operation (or create the row without it)",
                changed.join(", ")
            );
        }
        Ok(())
    }
}

/// A computed patch plan.
#[derive(Clone, Debug, Default)]
pub struct PatchPlan {
    /// Structural ops (Create/Move) in apply pre-order, then each row's
    /// content ops (state, content, tags, properties, drawer order), then
    /// Deletes.
    pub ops: Vec<PatchOp>,
    /// Existing blocks whose current version must equal the captured version
    /// before applying (optimistic concurrency).
    pub verify: Vec<(EntityUri, BlockVersion)>,
    /// Every row the plan writes, as the store must show it afterwards.
    pub expected: Vec<ExpectedRow>,
    /// Existing rows the text keeps where they are (no move), by label: a
    /// delete's subtree must not hold one.
    pub stays: Vec<(EntityUri, String)>,
    /// How an error names each row an op writes or deletes.
    pub labels: HashMap<Ref, String>,
    /// Each row of the text as the store holds it once the ops land; a
    /// created row under the id `block:dense-patch-new-{row}`.
    pub predicted: HashMap<Ref, Block>,
    /// The rows the ops write, in text order, then the deleted rows.
    pub written: Vec<Ref>,
}

/// A row a plan writes, as the store must show it once the plan applied.
#[derive(Clone, Debug)]
pub struct ExpectedRow {
    /// How a refusal names the row.
    pub label: String,
    pub block: Ref,
    /// The parent the row must hang under, and the sibling it must follow
    /// (`None`: the first child), for a created or moved row.
    pub place: Option<(Ref, Option<Ref>)>,
    pub view: RowView,
}

impl PatchOp {
    /// The row this op writes.
    fn row(&self) -> Ref {
        match self {
            PatchOp::Create { temp, .. } => Ref::New(*temp),
            PatchOp::SetContent { block_id, .. }
            | PatchOp::SetState { block_id, .. }
            | PatchOp::SetTags { block_id, .. }
            | PatchOp::SetProperty { block_id, .. }
            | PatchOp::SetCarrier { block_id, .. }
            | PatchOp::Move { block_id, .. }
            | PatchOp::Delete { block_id } => Ref::Existing(block_id.clone()),
        }
    }
}

impl PatchPlan {
    /// How an error names the row `op` writes.
    pub fn label_of(&self, op: &PatchOp) -> &str {
        self.labels
            .get(&op.row())
            .unwrap_or_else(|| panic!("plan_patch labels every row it writes; {op:?} has none"))
    }

    /// Count of move ops — used by the "untouched blocks don't move" invariant.
    pub fn move_count(&self) -> usize {
        self.ops
            .iter()
            .filter(|o| matches!(o, PatchOp::Move { .. }))
            .count()
    }
}

/// A conflict: existing blocks that changed since projection.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub blocks: Vec<EntityUri>,
}

/// Refuse a text the parser cannot be given as it stands, before it reads
/// it: a carriage return, written rows over a work bound, or a page
/// header other than the one dense_query emitted. A refusal names the row or
/// the page header, never the parse's own file.
pub fn refuse_unparsable_text(projection: &Projection, text: &str) -> Result<()> {
    refuse_carriage_returns(text)?;
    refuse_heavy_patch(projection, text)?;
    let header = preamble(text);
    if header == projection.header {
        return Ok(());
    }
    for line in &header {
        let indented = line.trim_start();
        if projection
            .header
            .iter()
            .any(|shown| shown.as_str() == *line)
            || !indented.starts_with('*')
        {
            continue;
        }
        if let Some(alias) = holon_org_format::row_alias(indented)? {
            bail!(
                "row {{#{alias}}}: its headline {line:?} does not start at the first column, so \
                 org reads it as text before the first row"
            );
        }
    }
    bail!(
        "the page header (the text before the first row) is {header:?}, not the header \
         dense_query emitted ({:?}) — a row's headline starts at the first column, and nothing \
         else goes before the first row",
        projection.header
    );
}

/// Refuse a text whose headline lines are not the rows the parser read,
/// naming the row whose text holds the first line they disagree on.
fn refuse_unsplit_rows(raw: &[String], parse: &DenseParse) -> Result<()> {
    if raw.len() == parse.blocks.len() {
        return Ok(());
    }
    let mut at = raw.len().min(parse.blocks.len());
    for (i, (row, db)) in raw.iter().zip(&parse.blocks).enumerate() {
        if holon_org_format::row_alias(row)? != db.alias {
            at = i;
            break;
        }
    }
    let holder = match at.checked_sub(1) {
        Some(i) => row_label(&parse.blocks[i]),
        None => "the page header".to_string(),
    };
    bail!(
        "{holder}: org reads {} rows in the text and its headline lines are {} — a line under \
         this row is a headline to one and text to the other; a headline is stars, a space and \
         its title",
        parse.blocks.len(),
        raw.len()
    )
}

/// Parent key for grouping patched siblings.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum ParentKey {
    Root,
    Id(String),
    New(usize),
}

/// Compute the patch plan. Fails loud on unknown alias, dangling parent, an
/// alias both edited and deleted, or a new top-level block against a synthetic
/// (multi-parent) render root.
pub fn plan_patch(
    projection: &Projection,
    text: &str,
    parse: &DenseParse,
    delete_aliases: &[Alias],
) -> Result<PatchPlan> {
    let todo_keywords = parse.todo_keywords.as_deref();
    refuse_unparsable_text(projection, text)?;
    if !projection.unreadable.is_empty() {
        let rows: Vec<String> = projection
            .unreadable
            .iter()
            .map(|(alias, why)| format!("row {{#{alias}}}: {why}"))
            .collect();
        bail!(
            "this projection cannot be patched, because dense_query could not show every row \
             exactly (`unfaithful_rows`): {}. Edit those blocks with execute_operation, or query \
             without them",
            rows.join("; ")
        );
    }
    // Resolve deletes.
    let mut delete_ids: Vec<EntityUri> = Vec::new();
    let mut deleted_set: HashSet<String> = HashSet::new();
    let mut labels: HashMap<Ref, String> = HashMap::new();
    for a in delete_aliases {
        let id = projection
            .alias_table
            .id_of(a)
            .ok_or_else(|| anyhow::anyhow!("delete: unknown alias {a}"))?;
        labels.insert(Ref::Existing(id.clone()), format!("row {{#{a}}}"));
        delete_ids.push(id.clone());
        deleted_set.insert(id.as_str().to_string());
    }

    // Row identity + parse_id index.
    let mut parse_id_to_row: HashMap<String, usize> = HashMap::new();
    for (i, db) in parse.blocks.iter().enumerate() {
        parse_id_to_row.insert(db.parse_id.as_str().to_string(), i);
    }
    for db in &parse.blocks {
        if db.block.content_type != ContentType::Text {
            let holder = db
                .parent_parse_id
                .as_ref()
                .map(|parent| row_label(&parse.blocks[parse_id_to_row[parent.as_str()]]))
                .unwrap_or_else(|| "the text before the first row".to_string());
            bail!(
                "{holder}: org reads lines of this row as a {} block, which the row cannot hold as \
                 its text — write each `#+` line of it as `,#+` to keep it as text",
                db.block.content_type
            );
        }
    }
    let raw = holon_org_format::dense_rows(text);
    refuse_unsplit_rows(&raw, parse)?;
    // Identity of each row.
    let mut row_ident: Vec<Ref> = Vec::with_capacity(parse.blocks.len());
    for (i, db) in parse.blocks.iter().enumerate() {
        match &db.alias {
            Some(a) => {
                let id = projection.alias_table.id_of(a).ok_or_else(|| {
                    anyhow::anyhow!("row {{#{a}}}: the alias is not in this projection")
                })?;
                if deleted_set.contains(id.as_str()) {
                    bail!("alias {a} is both present in the patch text and in the delete list");
                }
                if row_ident.contains(&Ref::Existing(id.clone())) {
                    bail!("row {{#{a}}}: the alias names more than one row");
                }
                row_ident.push(Ref::Existing(id.clone()));
            }
            None => row_ident.push(Ref::New(i)),
        }
    }

    // Patched parent of each row + children order per parent (parse order is
    // pre-order, so append preserves sibling order).
    let mut row_parent: Vec<ParentKey> = Vec::with_capacity(parse.blocks.len());
    let mut children: HashMap<ParentKey, Vec<usize>> = HashMap::new();
    for (i, db) in parse.blocks.iter().enumerate() {
        let pkey = match &db.parent_parse_id {
            None => ParentKey::Root,
            Some(pid) => {
                let prow = *parse_id_to_row.get(pid.as_str()).ok_or_else(|| {
                    anyhow::anyhow!("patch row {i} has a dangling parent reference")
                })?;
                match &row_ident[prow] {
                    Ref::Existing(id) => ParentKey::Id(id.as_str().to_string()),
                    Ref::New(idx) => ParentKey::New(*idx),
                    Ref::Root => ParentKey::Root,
                }
            }
        };
        row_parent.push(pkey.clone());
        children.entry(pkey).or_default().push(i);
    }

    // Helper: the `after` ref for a row = the identity of the immediately
    // preceding sibling under the same patched parent, or None if first.
    let after_ref = |row: usize| -> Option<Ref> {
        let pkey = &row_parent[row];
        let sibs = &children[pkey];
        let pos = sibs
            .iter()
            .position(|&r| r == row)
            .expect("row is its own sibling");
        if pos == 0 {
            None
        } else {
            Some(row_ident[sibs[pos - 1]].clone())
        }
    };

    // Helper: projection parent key of an existing block.
    let proj_parent_key = |id: &EntityUri| -> ParentKey {
        let rec = &projection.records[id.as_str()];
        match &rec.proj_parent {
            None => ParentKey::Root,
            Some(p) => ParentKey::Id(p.as_str().to_string()),
        }
    };

    let mut structural: Vec<PatchOp> = Vec::new();
    let mut content: Vec<PatchOp> = Vec::new();
    let mut verify: Vec<(EntityUri, BlockVersion)> = Vec::new();
    let mut verified: HashSet<String> = HashSet::new();
    let mark_verify = |id: &EntityUri,
                       verify: &mut Vec<(EntityUri, BlockVersion)>,
                       verified: &mut HashSet<String>| {
        if verified.insert(id.as_str().to_string()) {
            verify.push((id.clone(), projection.records[id.as_str()].version.clone()));
        }
    };

    // Which existing blocks are reparented (parent changed) — needed so the
    // reorder LIS only ranks blocks that stayed under the same parent.
    let mut reparented: HashSet<String> = HashSet::new();
    for (i, ident) in row_ident.iter().enumerate() {
        if let Ref::Existing(id) = ident {
            if row_parent[i] != proj_parent_key(id) {
                reparented.insert(id.as_str().to_string());
            }
        }
    }

    // Reorder detection: per patched parent, among existing rows that stayed
    // under this parent, keep the longest run whose projection order is
    // preserved (LIS by proj_index); the rest move.
    let mut needs_reorder_move: HashSet<String> = HashSet::new();
    for (pkey, rows) in &children {
        // existing, same-parent (not reparented-in) rows in patched order
        let stable_candidates: Vec<(usize, &EntityUri)> = rows
            .iter()
            .filter_map(|&r| match &row_ident[r] {
                Ref::Existing(id)
                    if !reparented.contains(id.as_str()) && proj_parent_key(id) == *pkey =>
                {
                    Some((r, id))
                }
                _ => None,
            })
            .collect();
        if stable_candidates.len() < 2 {
            continue;
        }
        let ranks: Vec<usize> = stable_candidates
            .iter()
            .map(|(_, id)| projection.records[id.as_str()].proj_index)
            .collect();
        let keep = lis_indices(&ranks);
        for (pos, (_, id)) in stable_candidates.iter().enumerate() {
            if !keep.contains(&pos) {
                needs_reorder_move.insert(id.as_str().to_string());
            }
        }
    }

    // Emit structural ops in patched pre-order.
    let mut expected: Vec<ExpectedRow> = Vec::new();
    let mut stays: Vec<(EntityUri, String)> = Vec::new();
    // The vocabulary of the document each row lands in.
    let mut row_vocabulary: Vec<Option<TaskKeywordVocabulary>> = Vec::new();
    let mut existing_row: HashMap<&str, usize> = HashMap::new();
    // Whether the row moves, or sits under a row that does: its file renders
    // it anew.
    let mut travels: Vec<bool> = Vec::new();
    for (i, db) in parse.blocks.iter().enumerate() {
        let label = row_label(db);
        labels.insert(row_ident[i].clone(), label.clone());
        let mut view = RowView::of(db).map_err(|e| anyhow::anyhow!("{label}: {e:#}"))?;
        let stays_in_place = match &row_ident[i] {
            Ref::Existing(id) => {
                !reparented.contains(id.as_str()) && !needs_reorder_move.contains(id.as_str())
            }
            _ => false,
        };
        // A parent the patch hands to another document takes its rows along.
        let rehomed_parent = match &row_parent[i] {
            ParentKey::Id(parent) => existing_row.get(parent.as_str()).and_then(|&row| {
                (row_vocabulary[row].as_ref()
                    != Some(&projection.records[parent.as_str()].vocabulary))
                .then_some(row)
            }),
            _ => None,
        };
        let vocabulary = match (&row_ident[i], &row_parent[i], rehomed_parent) {
            (_, _, Some(row)) => row_vocabulary[row].clone(),
            (Ref::Existing(id), _, None) if stays_in_place => {
                Some(projection.records[id.as_str()].vocabulary.clone())
            }
            (_, ParentKey::Root, None) => projection.root_vocabulary.clone(),
            (_, ParentKey::Id(parent), None) => {
                Some(projection.records[parent.as_str()].vocabulary.clone())
            }
            (_, ParentKey::New(row), None) => row_vocabulary[*row].clone(),
        };
        row_vocabulary.push(vocabulary.clone());
        let writes_state = match &row_ident[i] {
            Ref::Existing(id) => {
                existing_row.insert(id.as_str(), i);
                let record = &projection.records[id.as_str()];
                !stays_in_place
                    || vocabulary.as_ref() != Some(&record.vocabulary)
                    || record
                        .shown
                        .as_ref()
                        .is_some_and(|shown| shown.task_state != view.task_state)
            }
            _ => true,
        };
        if let (Some(state), Some(vocabulary), true) =
            (view.task_state.clone(), &vocabulary, writes_state)
        {
            view.task_state = Some(own_keyword(&state, vocabulary, &label)?);
        }
        match &row_ident[i] {
            Ref::New(idx) => {
                travels.push(true);
                if let Some(id) = &db.authored_id {
                    bail!(
                        "{label}: an `:ID: {}` line — a new row gets a minted id, and an existing \
                         row is named by its `{{#alias}}` token",
                        id.as_str()
                    );
                }
                refuse_misplaced_token(&view, None, &label)?;
                view.uncarried
                    .refuse_changes(&UncarriedFields::default(), &label)?;
                let carriers = carriers_to_write(db, &view, None, &[]);
                let written = Written {
                    kept: &KeptCarriers::default(),
                    carriers: &carriers,
                };
                refuse_page_keywords(db, todo_keywords, &written, &[], &label)?;
                refuse_inexact(db, todo_keywords, &written, &[], true, true, &label)?;
                refuse_unread_lines(db, &raw[i], todo_keywords, &written, &label)?;
                let parent = parent_ref(&row_parent[i], &projection.file_id, &label)?;
                structural.push(PatchOp::Create {
                    temp: *idx,
                    parent: parent.clone(),
                    after: after_ref(i),
                    carriers,
                    content: RowContent::of(&db.block),
                    task_state: view.task_state.clone(),
                    attributes: view.attributes.clone(),
                });
                expected.push(ExpectedRow {
                    label,
                    block: Ref::New(*idx),
                    place: Some((parent, after_ref(i))),
                    view,
                });
            }
            Ref::Existing(id) => {
                let moved =
                    reparented.contains(id.as_str()) || needs_reorder_move.contains(id.as_str());
                let under_a_traveller = match &row_parent[i] {
                    ParentKey::Id(parent) => existing_row
                        .get(parent.as_str())
                        .is_some_and(|&row| travels[row]),
                    _ => false,
                };
                travels.push(moved || under_a_traveller);
                let mut place = None;
                if !moved {
                    stays.push((id.clone(), label.clone()));
                }
                if moved {
                    let to = parent_ref(&row_parent[i], &projection.file_id, &label)?;
                    structural.push(PatchOp::Move {
                        block_id: id.clone(),
                        parent: to.clone(),
                        after: after_ref(i),
                    });
                    mark_verify(id, &mut verify, &mut verified);
                    place = Some((to, after_ref(i)));
                }
                let rec = &projection.records[id.as_str()];
                let shown = rec.shown.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "{label}: the projection shows this block without a token, so no row \
                         names it"
                    )
                })?;
                if let Some(authored) = &db.authored_id {
                    if authored.as_str() != id.id() {
                        bail!(
                            "{label}: `:ID: {}` is not this row's id `{}` — a row is named by its \
                             `{{#alias}}` token",
                            authored.as_str(),
                            id.id()
                        );
                    }
                }
                refuse_misplaced_token(&view, Some(shown), &label)?;
                view.uncarried.refuse_changes(&shown.uncarried, &label)?;
                if view == *shown {
                    // A row whose text the agent left as shown is no edit, even
                    // when org reads it otherwise (it is disclosed in
                    // `unfaithful_rows`). Text org drops is an edit to judge.
                    if holon_org_format::row_lines(&raw[i])? != rec.shown_lines {
                        let unwritten = Written {
                            kept: &rec.kept,
                            carriers: &[],
                        };
                        refuse_unread_lines(db, &raw[i], todo_keywords, &unwritten, &label)?;
                    }
                    if travels[i] {
                        expected.push(ExpectedRow {
                            label,
                            block: Ref::Existing(id.clone()),
                            place,
                            view,
                        });
                    }
                    continue;
                }
                let headline_changed =
                    view.title != shown.title || view.attributes.tags != shown.attributes.tags;
                let body_changed = view.body != shown.body;
                let state_changed = view.task_state != shown.task_state;
                if rec.task_state != shown.task_state
                    && (headline_changed || body_changed || state_changed)
                {
                    let keyword = shown
                        .task_state
                        .as_ref()
                        .map(|s| s.keyword.as_str())
                        .unwrap_or_default();
                    bail!(
                        "{label}: the store holds this block with no task state, but its title \
                         starts with `{keyword}`, which org reads as the task keyword, so its \
                         title, body or state cannot be edited exactly here — edit it with \
                         execute_operation"
                    );
                }
                let carriers = carriers_to_write(db, &view, Some(shown), &rec.drawer_order);
                let written = Written {
                    kept: &rec.kept,
                    carriers: &carriers,
                };
                refuse_page_keywords(db, todo_keywords, &written, &rec.shown_lines, &label)?;
                refuse_inexact(
                    db,
                    todo_keywords,
                    &written,
                    &shown.drawer,
                    headline_changed,
                    body_changed,
                    &label,
                )?;
                refuse_unread_lines(db, &raw[i], todo_keywords, &written, &label)?;
                mark_verify(id, &mut verify, &mut verified);
                let content_before = content.len();
                if state_changed {
                    content.push(PatchOp::SetState {
                        block_id: id.clone(),
                        task_state: view.task_state.clone(),
                    });
                }
                if headline_changed || body_changed {
                    let stored = RowContent::of(&rec.stored);
                    let written =
                        stored.edited(&RowContent::of(&db.block), headline_changed, body_changed);
                    if written != stored {
                        content.push(PatchOp::SetContent {
                            block_id: id.clone(),
                            content: written,
                        });
                    }
                }
                if headline_changed && view.attributes.tags != rec.tags {
                    content.push(PatchOp::SetTags {
                        block_id: id.clone(),
                        tags: view.attributes.tags.clone(),
                    });
                }
                let old = &shown.attributes.properties;
                let new = &view.attributes.properties;
                let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
                for key in keys {
                    if new.get(key) != old.get(key) {
                        content.push(PatchOp::SetProperty {
                            block_id: id.clone(),
                            key: AuthoredKey::new(key),
                            value: new.get(key).cloned(),
                        });
                    }
                }
                for carrier in carriers {
                    content.push(PatchOp::SetCarrier {
                        block_id: id.clone(),
                        carrier,
                    });
                }
                if content.len() == content_before {
                    bail!(
                        "{label}: differs from the row dense_query showed, yet plans no write \
                         (a dense_patch defect — report it): {view:?} vs {shown:?}"
                    );
                }
                expected.push(ExpectedRow {
                    label,
                    block: Ref::Existing(id.clone()),
                    place,
                    view,
                });
            }
            Ref::Root => unreachable!("a row is never Root"),
        }
    }

    let mut ops = structural;
    ops.extend(content);
    let written = written_rows(&row_ident, &ops, &delete_ids);
    if let Some(row) = written.get(MAX_WRITTEN_ROWS_PER_PATCH) {
        bail!(
            "{}: this is written row {}, a moved or deleted row included; one dense_patch writes \
             at most {MAX_WRITTEN_ROWS_PER_PATCH} rows — send the rows from this one on in another \
             patch",
            labels[row],
            MAX_WRITTEN_ROWS_PER_PATCH + 1
        );
    }
    refuse_rows_out_of_place(projection, &row_ident, &row_parent, &ops, &labels)?;
    let predicted = refuse_rows_not_written_exactly(
        projection,
        &raw,
        &row_ident,
        &row_vocabulary,
        &ops,
        &labels,
    )?;
    let predicted = row_ident.into_iter().zip(predicted).collect();
    for id in &delete_ids {
        mark_verify(id, &mut verify, &mut verified);
        ops.push(PatchOp::Delete {
            block_id: id.clone(),
        });
    }

    Ok(PatchPlan {
        ops,
        verify,
        expected,
        stays,
        labels,
        predicted,
        written,
    })
}

/// The rows `ops` write, in text order, then the rows `deletes` delete.
fn written_rows(rows: &[Ref], ops: &[PatchOp], deletes: &[EntityUri]) -> Vec<Ref> {
    let touched: HashSet<Ref> = ops.iter().map(PatchOp::row).collect();
    rows.iter()
        .filter(|row| touched.contains(row))
        .cloned()
        .chain(deletes.iter().map(|id| Ref::Existing(id.clone())))
        .collect()
}

/// Refuse each row the store would not show as the agent wrote it: `ops`
/// applied to the stored rows, projected again, must give every row of `raw`
/// its text and its headline stars. Returns those predicted rows.
fn refuse_rows_not_written_exactly(
    projection: &Projection,
    raw: &[String],
    row_ident: &[Ref],
    row_vocabulary: &[Option<TaskKeywordVocabulary>],
    ops: &[PatchOp],
    labels: &HashMap<Ref, String>,
) -> Result<Vec<Block>> {
    let id_of = |r: &Ref| match r {
        Ref::Root => projection.file_id.clone(),
        Ref::Existing(id) => id.clone(),
        Ref::New(row) => EntityUri::block(&format!("dense-patch-new-{row}")),
    };
    let mut predicted: Vec<Block> = row_ident
        .iter()
        .map(|r| match r {
            Ref::Existing(id) => projection.records[id.as_str()].stored.clone(),
            Ref::New(_) => Block::new_text(id_of(r), projection.file_id.clone(), ""),
            Ref::Root => unreachable!("a row is never Root"),
        })
        .collect();
    let at: HashMap<EntityUri, usize> = predicted
        .iter()
        .enumerate()
        .map(|(row, block)| (block.id.clone(), row))
        .collect();
    for op in ops {
        let Some(&row) = at.get(&id_of(&op.row())) else {
            continue;
        };
        let block = &mut predicted[row];
        match op {
            PatchOp::Create {
                parent,
                content,
                task_state,
                attributes,
                carriers,
                ..
            } => {
                block.parent_id = id_of(parent);
                write_content(block, content);
                block.set_task_state(task_state.clone());
                block.set_tags(attributes.tags.clone());
                for (key, value) in &attributes.properties {
                    block.set_property(AuthoredKey::new(key).property(), value.clone());
                }
                carriers
                    .iter()
                    .for_each(|carrier| write_carrier(block, carrier));
            }
            PatchOp::SetContent { content, .. } => write_content(block, content),
            PatchOp::SetState { task_state, .. } => block.set_task_state(task_state.clone()),
            PatchOp::SetTags { tags, .. } => block.set_tags(tags.clone()),
            PatchOp::SetProperty { key, value, .. } => match value {
                Some(value) => block.set_property(key.property(), value.clone()),
                None => {
                    block.properties.remove(&key.property());
                }
            },
            PatchOp::SetCarrier { carrier, .. } => write_carrier(block, carrier),
            PatchOp::Move { parent, .. } => block.parent_id = id_of(parent),
            PatchOp::Delete { .. } => unreachable!("a deleted block is not a row"),
        }
    }

    let mut vocabularies: HashMap<String, TaskKeywordVocabulary> = HashMap::new();
    for (row, (block, vocabulary)) in predicted.iter().zip(row_vocabulary).enumerate() {
        let Some(vocabulary) = vocabulary else {
            bail!(
                "{}: it lands at the top of a projection that spans several parents, which \
                 is no document",
                labels[&row_ident[row]]
            );
        };
        vocabularies.insert(block.parent_id.as_str().to_string(), vocabulary.clone());
    }
    let built = crate::dense_projection::build_projection(
        predicted.clone(),
        &crate::dense_projection::DocVocabularies::ByParent(vocabularies),
    )?;
    let shown_rows = holon_org_format::dense_rows(&built.dense_text);
    let text_blocks: Vec<&str> = built
        .ordered_blocks
        .iter()
        .filter(|b| b.content_type == ContentType::Text)
        .map(|b| b.id.as_str())
        .collect();
    assert_eq!(
        shown_rows.len(),
        text_blocks.len(),
        "a dense text holds one row per text block:\n{}",
        built.dense_text
    );
    let shown_row: HashMap<&str, &String> = text_blocks.into_iter().zip(&shown_rows).collect();
    for (row, block) in predicted.iter().enumerate() {
        let label = &labels[&row_ident[row]];
        if let Some(state) = block
            .task_state()
            .filter(|state| holon_org_format::asks_nothing(&state.keyword, &block.content))
        {
            bail!(
                "{label}: a `{}` row with no text asks nothing, and the store drops the keyword \
                 of an empty question; write the question after it",
                state.keyword
            );
        }
        let Some(shown) = shown_row.get(block.id.as_str()) else {
            bail!("{label}: the store would hold this row where dense_query does not show it");
        };
        let (written, stored) = (
            holon_org_format::row_lines(&raw[row])?,
            holon_org_format::row_lines(shown)?,
        );
        if written != stored {
            bail!(
                "{label}: the store would hold this row as {stored:?}, not as the text written \
                 {written:?}"
            );
        }
        let (want, got) = (headline_stars(&raw[row]), headline_stars(shown));
        if want != got {
            bail!("{label}: its headline has {want} stars; at its place org writes {got}");
        }
    }
    Ok(predicted)
}

/// Refuse each row `ops` would not leave where the text puts it: under the
/// text's parent, right after the row the text shows before it under that
/// parent (first when none). Siblings the text does not show are skipped.
fn refuse_rows_out_of_place(
    projection: &Projection,
    row_ident: &[Ref],
    row_parent: &[ParentKey],
    ops: &[PatchOp],
    labels: &HashMap<Ref, String>,
) -> Result<()> {
    let id_of = |r: &Ref| match r {
        Ref::Root => projection.file_id.clone(),
        Ref::Existing(id) => id.clone(),
        Ref::New(row) => EntityUri::block(&format!("dense-patch-new-{row}")),
    };
    let mut stored: Vec<&crate::dense_projection::ProjectedBlock> =
        projection.records.values().collect();
    stored.sort_by_key(|record| record.proj_index);
    let mut children: HashMap<EntityUri, Vec<Ref>> = HashMap::new();
    for record in stored {
        children
            .entry(record.true_parent.clone())
            .or_default()
            .push(Ref::Existing(record.block_id.clone()));
    }
    for op in ops {
        let (row, parent, after) = match op {
            PatchOp::Create {
                temp,
                parent,
                after,
                ..
            } => (Ref::New(*temp), parent, after),
            PatchOp::Move {
                block_id,
                parent,
                after,
            } => {
                let row = Ref::Existing(block_id.clone());
                children
                    .values_mut()
                    .for_each(|siblings| siblings.retain(|sibling| *sibling != row));
                (row, parent, after)
            }
            _ => continue,
        };
        let siblings = children.entry(id_of(parent)).or_default();
        let at = match after {
            None => 0,
            Some(after) => {
                1 + siblings
                    .iter()
                    .position(|sibling| sibling == after)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "{}: it is written after {}, which is not under its parent",
                            labels[&row],
                            labels[after]
                        )
                    })?
            }
        };
        siblings.insert(at, row);
    }

    let in_text: HashSet<&Ref> = row_ident.iter().collect();
    let mut placed_at: HashMap<&Ref, (EntityUri, Option<&Ref>)> = HashMap::new();
    for (parent, siblings) in &children {
        let mut before = None;
        for sibling in siblings.iter().filter(|sibling| in_text.contains(sibling)) {
            placed_at.insert(sibling, (parent.clone(), before));
            before = Some(sibling);
        }
    }
    let synthetic_root = projection.file_id.id() == SYNTHETIC_ROOT;
    let mut last_under: HashMap<&ParentKey, &Ref> = HashMap::new();
    for (row, parent) in row_ident.iter().zip(row_parent) {
        let previous = last_under.insert(parent, row);
        if synthetic_root && *parent == ParentKey::Root {
            continue;
        }
        let want = (
            id_of(&parent_ref(parent, &projection.file_id, &labels[row])?),
            previous,
        );
        let got = placed_at.get(row).cloned().ok_or_else(|| {
            anyhow::anyhow!("{}: the writes would leave it in no place", labels[row])
        })?;
        if got != want {
            let name =
                |r: Option<&Ref>| r.map_or("first".to_string(), |r| format!("after {}", labels[r]));
            bail!(
                "{}: the writes would put it under {} {}, where the text puts it under {} {}",
                labels[row],
                got.0,
                name(got.1),
                want.0,
                name(want.1)
            );
        }
    }
    Ok(())
}

fn headline_stars(row: &str) -> usize {
    row.chars().take_while(|&c| c == '*').count()
}

fn write_content(block: &mut Block, content: &RowContent) {
    block.content = content.text.clone();
    block.marks = (!content.marks.is_empty()).then(|| content.marks.clone());
}

fn write_carrier(block: &mut Block, carrier: &ParsedCarrier) {
    match carrier.value() {
        Some(value) => block.set_property(carrier.key(), value.to_string()),
        None => {
            block.properties.remove(carrier.key());
        }
    }
}

/// `plan` with each row its ops write named by its `Debug` form.
#[cfg(test)]
pub(crate) fn labelled(mut plan: PatchPlan) -> PatchPlan {
    for op in &plan.ops {
        let row = op.row();
        let label = format!("row {row:?}");
        plan.labels.insert(row, label);
    }
    plan
}

fn parent_ref(pkey: &ParentKey, file_id: &EntityUri, label: &str) -> Result<Ref> {
    match pkey {
        ParentKey::Root => {
            if file_id.id() == SYNTHETIC_ROOT {
                bail!(
                    "{label}: a top-level row has no page to go to, since this projection spans \
                     several pages. Put it under one of the pages' rows instead."
                );
            }
            Ok(Ref::Root)
        }
        ParentKey::Id(id) => Ok(Ref::Existing(EntityUri::parse(id)?)),
        ParentKey::New(idx) => Ok(Ref::New(*idx)),
    }
}

/// Longest strictly-increasing subsequence — returns the set of POSITIONS in
/// `seq` that belong to one LIS (patience sorting with parent links).
fn lis_indices(seq: &[usize]) -> HashSet<usize> {
    let n = seq.len();
    if n == 0 {
        return HashSet::new();
    }
    let mut tails: Vec<usize> = Vec::new(); // positions of pile tops
    let mut tails_val: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        // first tail with value >= seq[i]  (strictly increasing → lower_bound)
        let pos = tails_val.partition_point(|&v| v < seq[i]);
        if pos == tails.len() {
            tails.push(i);
            tails_val.push(seq[i]);
        } else {
            tails[pos] = i;
            tails_val[pos] = seq[i];
        }
        prev[i] = if pos > 0 { Some(tails[pos - 1]) } else { None };
    }
    let mut out = HashSet::new();
    let mut k = tails.last().copied();
    while let Some(i) = k {
        out.insert(i);
        k = prev[i];
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use holon_org_format::parse_dense;

    use super::*;
    use crate::dense_projection::build_projection;

    /// A page whose rows carry every field a patch does not write, projected
    /// the way `dense_query` projects it.
    const PAGE: &str = "\
#+TITLE: Uncarried
* TODO [#A] Planned work
SCHEDULED: <2026-09-01 Tue> DEADLINE: <2026-09-05 Sat>
:PROPERTIES:
:ID: uc-planned
:REQUIRES: uc-blocker
:END:
Body line of the planned work.
* Blocker
:PROPERTIES:
:ID: uc-blocker
:COLLAPSED: t
:END:
** Hidden child
:PROPERTIES:
:ID: uc-child
:END:
";

    fn projected() -> (Projection, String) {
        let parsed = holon_org_format::parse_org_file(
            Path::new("/vault/uncarried.org"),
            PAGE,
            &EntityUri::no_parent(),
            Path::new("/vault"),
        )
        .expect("page parses");
        let built = build_projection(
            parsed.blocks,
            &crate::dense_projection::DocVocabularies::Uniform(
                holon_org_format::TaskKeywordVocabulary::default(),
            ),
        )
        .expect("projection builds");
        (Projection::new("test".into(), &built), built.dense_text)
    }

    fn plan(projection: &Projection, text: &str) -> Result<PatchPlan> {
        plan_patch(
            projection,
            text,
            &parse_dense(text).expect("dense text parses"),
            &[],
        )
    }

    fn edit(dense: &str, from: &str, to: &str) -> String {
        assert!(
            dense.contains(from),
            "{from:?} is not in the projection:\n{dense}"
        );
        dense.replacen(from, to, 1)
    }

    /// The text `* Blocker`, `** Hidden child`, `* Planned work`: a move of
    /// `Planned work` to the front leaves the rows out of the text's order.
    #[test]
    fn a_move_the_text_does_not_ask_for_is_refused_by_the_rows_name() {
        let (projection, _) = projected();
        let id = |content: &str| {
            projection
                .records
                .values()
                .find(|r| r.stored.content.contains(content))
                .unwrap_or_else(|| panic!("no {content} record"))
                .block_id
                .clone()
        };
        let (blocker, child, planned) = (id("Blocker"), id("Hidden child"), id("Planned work"));
        let row_ident = vec![
            Ref::Existing(blocker.clone()),
            Ref::Existing(child),
            Ref::Existing(planned.clone()),
        ];
        let row_parent = vec![
            ParentKey::Root,
            ParentKey::Id(blocker.as_str().to_string()),
            ParentKey::Root,
        ];
        let labels: HashMap<Ref, String> = row_ident
            .iter()
            .enumerate()
            .map(|(row, r)| (r.clone(), format!("row {{#{row}}}")))
            .collect();
        let move_after = |after: Option<Ref>| {
            vec![PatchOp::Move {
                block_id: planned.clone(),
                parent: Ref::Root,
                after,
            }]
        };

        let err = refuse_rows_out_of_place(
            &projection,
            &row_ident,
            &row_parent,
            &move_after(None),
            &labels,
        )
        .expect_err("the move puts `Planned work` first, the text puts it after `Blocker`");
        let msg = format!("{err:#}");
        assert!(
            msg.starts_with("row {#") && msg.contains("where the text puts it"),
            "the refusal names a row the move leaves out of place: {msg}"
        );
        refuse_rows_out_of_place(
            &projection,
            &row_ident,
            &row_parent,
            &move_after(Some(Ref::Existing(blocker))),
            &labels,
        )
        .expect("the move the text asks for is admitted");
    }

    #[test]
    fn an_unedited_projection_plans_no_op() {
        let (projection, dense) = projected();
        let plan = plan(&projection, &dense).expect("an unedited projection plans");
        assert!(plan.ops.is_empty(), "ops: {:?}\n{dense}", plan.ops);
    }

    #[test]
    fn an_edit_of_an_uncarried_field_refuses_the_patch() {
        let (projection, dense) = projected();
        for (field, from, to) in [
            ("priority", "[#A]", "[#B]"),
            ("SCHEDULED", "<2026-09-01 Tue>", "<2026-09-02 Wed>"),
            ("DEADLINE", "<2026-09-05 Sat>", "<2026-09-06 Sun>"),
            ("REQUIRES", ":REQUIRES: uc-blocker", ":REQUIRES: uc-child"),
            ("COLLAPSED", ":COLLAPSED: t\n", ""),
        ] {
            let text = edit(&dense, from, to);
            let err = plan(&projection, &text)
                .expect_err("an ignored edit must refuse the patch, not vanish");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("row {#") && msg.contains(field),
                "the refusal of a {field} edit must name the row and {field}: {msg}\n{text}"
            );
        }
    }
}
