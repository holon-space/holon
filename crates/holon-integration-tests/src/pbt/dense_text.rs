//! The edits an agent makes to `dense_query` text, as text: shared by the
//! keystone's `DenseProjectionEdit` and the real-engine `dense_patch`
//! property, so both drive `dense_patch` with the same edits.

use std::collections::BTreeMap;

use holon_api::Tags;
use holon_org_format::ValueCarrier;
pub use holon_pbt_core::capabilities::NewRowPlace;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

/// One edit of one existing row.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RowTextEdit {
    /// Replace the title, keeping keyword, tags and token.
    Retitle(String),
    /// Replace the body lines under the drawer.
    SetBody(Vec<String>),
    /// Set or remove the task keyword.
    SetState(Option<String>),
    /// Replace the tag group.
    Retag(Vec<String>),
    /// Replace the drawer line of a key, or add one.
    SetProperty(String, String),
    /// Remove the n-th drawer line (modulo the line count).
    DropProperty(usize),
    /// Move the row to the front of the text.
    MoveToFront,
    /// Nest the row one level deeper, under the row before it.
    Demote,
}

/// A row an agent adds (no token).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NewRowText {
    pub title: String,
    pub body: Vec<String>,
    pub state: Option<String>,
    pub tags: Tags,
    pub properties: BTreeMap<String, String>,
}

/// The shape `place` gives a new row in a text of `rows` rows, for reach
/// counts.
pub fn new_row_place_shape(place: NewRowPlace, rows: usize) -> &'static str {
    match place {
        NewRowPlace::Last => "new row last",
        NewRowPlace::Before(n) if n % rows == 0 => "new row before every other row",
        NewRowPlace::Before(_) => "new row between two rows",
        NewRowPlace::FirstChildOf(_) => "new row as a row's first child",
    }
}

pub fn new_row_place() -> BoxedStrategy<NewRowPlace> {
    prop_oneof![
        1 => Just(NewRowPlace::Last),
        1 => Just(NewRowPlace::Before(0)),
        1 => (1usize..6).prop_map(NewRowPlace::Before),
        1 => (0usize..6).prop_map(NewRowPlace::FirstChildOf),
    ]
    .boxed()
}

/// A dense text as its header lines and its rows; a row is a headline and
/// every line up to the next headline.
#[derive(Clone, Debug)]
pub struct DenseRows {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Org's headline (emacs 30.2): stars at the line's start, then a space.
fn is_headline(line: &str) -> bool {
    let rest = line.trim_start_matches('*');
    rest.len() < line.len() && rest.starts_with(' ')
}

/// The keywords the text's `#+TODO:` line declares.
fn declared_keywords(header: &[String]) -> Vec<String> {
    header
        .iter()
        .find_map(|l| l.strip_prefix("#+TODO:"))
        .map(|spec| {
            spec.split_whitespace()
                .filter(|w| *w != "|")
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A headline split into the parts an edit replaces.
struct Headline {
    stars: String,
    keyword: Option<String>,
    title: String,
    tags: Vec<String>,
    token: Option<String>,
}

impl Headline {
    fn parse(line: &str, keywords: &[String]) -> Headline {
        let rest = line.trim_start_matches('*');
        let stars = line[..line.len() - rest.len()].to_string();
        let rest = rest.trim();
        let (before, token) = match rest.rfind("{#") {
            Some(at) if rest.ends_with('}') && (at == 0 || rest[..at].ends_with(' ')) => {
                (rest[..at].trim_end(), Some(rest[at..].to_string()))
            }
            _ => (rest, None),
        };
        let (title, tags) = holon_org_format::parser::split_headline_tags(before);
        let (keyword, title) = match title.split_once(' ') {
            Some((word, title)) if keywords.iter().any(|k| k == word) => {
                (Some(word.to_string()), title.to_string())
            }
            _ if keywords.contains(&title) => (Some(title.clone()), String::new()),
            _ => (None, title),
        };
        Headline {
            stars,
            keyword,
            title,
            tags,
            token,
        }
    }

    fn render(&self) -> String {
        let mut line = self.stars.clone();
        if let Some(keyword) = &self.keyword {
            line.push(' ');
            line.push_str(keyword);
        }
        line.push(' ');
        line.push_str(&self.title);
        if !self.tags.is_empty() {
            line.push_str(&format!(" :{}:", self.tags.join(":")));
        }
        if let Some(token) = &self.token {
            line.push(' ');
            line.push_str(token);
        }
        line
    }
}

fn drawer_line(key: &str, value: &str) -> String {
    format!(":{key}: {}", ValueCarrier::HeadlineDrawer.encode(value))
}

/// The index range of a row's drawer lines (between the markers), when it has
/// a drawer.
fn drawer_span(row: &[String]) -> Option<(usize, usize)> {
    if row.get(1).map(String::as_str) != Some(":PROPERTIES:") {
        return None;
    }
    let end = row
        .iter()
        .position(|l| l == ":END:")
        .expect("a drawer dense_query renders is closed");
    Some((2, end))
}

impl DenseRows {
    pub fn split(text: &str) -> DenseRows {
        let mut header = Vec::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for line in text.lines() {
            if is_headline(line) {
                rows.push(vec![line.to_string()]);
            } else if let Some(row) = rows.last_mut() {
                row.push(line.to_string());
            } else {
                header.push(line.to_string());
            }
        }
        DenseRows { header, rows }
    }

    pub fn join(&self) -> String {
        let mut out = self.header.join("\n");
        for row in &self.rows {
            out.push('\n');
            out.push_str(&row.join("\n"));
        }
        out.push('\n');
        out
    }

    /// The `{#alias}` of row `row`, without braces and gap marker.
    pub fn alias(&self, row: usize) -> Option<String> {
        let head = &self.rows[row][0];
        let token = head[head.rfind("{#")? + 2..].trim_end().strip_suffix('}')?;
        Some(token.trim_end_matches('^').to_string())
    }

    pub fn edit(&mut self, row: usize, edit: &RowTextEdit) {
        self.edit_all(row, std::slice::from_ref(edit));
    }

    /// Apply `edits` to one row against ONE reading of its headline: a
    /// keyword-headed title written onto a row with no keyword must not be
    /// read back as a keyword by the next edit.
    pub fn edit_all(&mut self, row: usize, edits: &[RowTextEdit]) {
        let keywords = declared_keywords(&self.header);
        let r = &mut self.rows[row];
        let mut head = Headline::parse(&r[0], &keywords);
        for edit in edits {
            match edit {
                RowTextEdit::Retitle(title) => head.title = title.clone(),
                RowTextEdit::SetState(state) => head.keyword = state.clone(),
                RowTextEdit::Retag(tags) => head.tags = tags.clone(),
                RowTextEdit::SetBody(body) => {
                    let body_start = drawer_span(r).map_or(1, |(_, end)| end + 1);
                    r.truncate(body_start);
                    r.extend(body.iter().cloned());
                }
                RowTextEdit::SetProperty(key, value) => {
                    let line = drawer_line(key, value);
                    match drawer_span(r) {
                        Some((start, end)) => {
                            let prefix = format!(":{key}:");
                            match (start..end).find(|&i| r[i].starts_with(&prefix)) {
                                Some(i) => r[i] = line,
                                None => r.insert(end, line),
                            }
                        }
                        None => {
                            r.splice(
                                1..1,
                                [":PROPERTIES:".to_string(), line, ":END:".to_string()],
                            );
                        }
                    }
                }
                RowTextEdit::DropProperty(n) => {
                    if let Some((start, end)) = drawer_span(r).filter(|(start, end)| end > start) {
                        r.remove(start + n % (end - start));
                    }
                }
                RowTextEdit::Demote => head.stars.push('*'),
                RowTextEdit::MoveToFront => {}
            }
        }
        r[0] = head.render();
        if edits.contains(&RowTextEdit::MoveToFront) {
            let moved = self.rows.remove(row);
            self.rows.insert(0, moved);
        }
    }

    pub fn insert(&mut self, new: &NewRowText, place: NewRowPlace) {
        let stars = |row: &[String]| row[0].len() - row[0].trim_start_matches('*').len();
        let (at, level) = match place {
            NewRowPlace::Last => (self.rows.len(), 1),
            NewRowPlace::Before(n) => {
                let at = n % self.rows.len();
                (at, stars(&self.rows[at]))
            }
            NewRowPlace::FirstChildOf(n) => {
                let parent = n % self.rows.len();
                (parent + 1, stars(&self.rows[parent]) + 1)
            }
        };
        let head = Headline {
            stars: "*".repeat(level),
            keyword: new.state.clone(),
            title: new.title.clone(),
            tags: new.tags.iter().cloned().collect(),
            token: None,
        };
        let mut row = vec![head.render()];
        if !new.properties.is_empty() {
            row.push(":PROPERTIES:".to_string());
            row.extend(new.properties.iter().map(|(k, v)| drawer_line(k, v)));
            row.push(":END:".to_string());
        }
        row.extend(new.body.iter().cloned());
        self.rows.insert(at, row);
    }

    pub fn remove(&mut self, row: usize) {
        self.rows.remove(row);
    }
}

/// Titles an agent writes: plain, with a colon, and keyword-headed (which the
/// engine converges unless the row's state is written first).
pub fn dense_title() -> BoxedStrategy<String> {
    prop_oneof![
        4 => "[A-Z][a-z]{1,6}( [a-z]{1,5})?",
        1 => "[A-Z][a-z]{1,5}: [a-z]{1,4}",
        2 => "(TODO|DONE) [a-z]{1,4}",
    ]
    .boxed()
}

/// Body lines under a headline, none of which org reads as a headline: text,
/// and the org elements a body line can spell — a keyword line (org reads it
/// as a keyword, not as text), the same line written `,#+`, a table, a block,
/// a drawer-shaped run, a rule, a footnote, a blank line, a `#+` line inside
/// an example or export block, and a babel call.
pub fn dense_body() -> BoxedStrategy<Vec<String>> {
    prop::collection::vec(body_element(), 0..3)
        .prop_map(|lines| lines.concat())
        .boxed()
}

/// [`dense_body`] as an agent edits it, with lines the store never holds too:
/// page keyword lines org reads for the whole page, a `,*` line, and verse,
/// src and center blocks.
pub fn edited_dense_body() -> BoxedStrategy<Vec<String>> {
    let block = |kind: &'static str, line: &'static str| {
        Just(vec![
            format!("#+begin_{kind}"),
            line.to_string(),
            format!("#+end_{kind}"),
        ])
    };
    let [title, filetags, category, property] = PAGE_KEYWORD_LINES;
    let line = |line: &'static str| Just(vec![line.to_string()]);
    prop::collection::vec(
        prop_oneof![
            30 => body_element(),
            5 => line(title),
            5 => line(filetags),
            5 => line(category),
            5 => line(property),
            4 => line(COMMA_STAR_LINE),
            4 => block("verse", "a verse"),
            4 => block("src", "x = 1"),
            4 => block("center", "centered"),
            6 => mark_run_line().prop_map(|line| vec![line]),
        ],
        0..4,
    )
    .prop_map(|lines| lines.concat())
    .boxed()
}

/// A line holding a run of one emphasis marker, alone or next to a word.
fn mark_run_line() -> BoxedStrategy<String> {
    (
        proptest::sample::select(MARK_RUN_MARKERS),
        1usize..=120,
        0usize..3,
    )
        .prop_map(|(marker, n, at)| {
            let run = marker.to_string().repeat(n);
            match at {
                0 => run,
                1 => format!("x {run}"),
                _ => format!("{run}x y"),
            }
        })
        .boxed()
}

const MARK_RUN_MARKERS: &[char] = &['=', '/', '*', '+', '~', '_'];

/// Keyword lines org reads for the whole page wherever they stand.
const PAGE_KEYWORD_LINES: [&str; 4] = [
    "#+TITLE: a page",
    "#+FILETAGS: :x:",
    "#+CATEGORY: c",
    "#+PROPERTY: foo bar",
];
const COMMA_STAR_LINE: &str = ",* a star line";
/// A `#+` line inside an example or export block, where org removes a comma.
const BLOCK_HASH_LINE: &str = "#+in a block";
const BABEL_CALL_LINE: &str = "#+CALL: f()";

fn body_element() -> BoxedStrategy<Vec<String>> {
    let one = |line: &'static str| Just(vec![line.to_string()]);
    let text = |pattern: &'static str| {
        proptest::string::string_regex(pattern)
            .expect("valid regex")
            .prop_map(|line| vec![line])
    };
    prop_oneof![
            8 => text("[a-z][a-z ]{0,8}[a-z]"),
            2 => text("- [a-z]{1,4}"),
            2 => text("  [a-z]{1,4}"),
            2 => text("[a-z]{1,3} \\{#[0-9]\\}"),
            3 => one("#+CAPTION: a figure"),
            3 => one(",#+CAPTION: a figure"),
            3 => one("| a | b |"),
            3 => Just(["#+begin_quote", "q", "#+end_quote"].map(str::to_string).to_vec()),
            3 => Just([":NOTE:", "x", ":END:"].map(str::to_string).to_vec()),
            3 => one("-----"),
            3 => one("[fn:1] a note"),
            3 => one(""),
            2 => Just([
                "#+begin_example",
                BLOCK_HASH_LINE,
                "#+end_example"
            ].map(str::to_string).to_vec()),
            2 => Just([
                "#+begin_export html",
                BLOCK_HASH_LINE,
                "#+end_export"
            ].map(str::to_string).to_vec()),
            2 => one(BABEL_CALL_LINE),
    ]
    .boxed()
}

/// The shape of a body line [`dense_body`] draws, for reach counts.
pub fn body_line_shape(line: &str) -> &'static str {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("#+begin_verse") {
        "body verse block"
    } else if lower.starts_with("#+begin_src") {
        "body src block"
    } else if lower.starts_with("#+begin_center") {
        "body center block"
    } else if lower.starts_with("#+begin_") || lower.starts_with("#+end_") {
        "body block delimiter"
    } else if line.len() > 1
        && MARK_RUN_MARKERS
            .iter()
            .any(|&m| line.chars().filter(|&c| c == m).count() > 1)
    {
        "body mark run"
    } else if line == COMMA_STAR_LINE {
        "body comma-escaped star line"
    } else if lower.starts_with("#+filetags:") {
        "body FILETAGS line"
    } else if lower.starts_with("#+category:") {
        "body CATEGORY line"
    } else if lower.starts_with("#+property:") {
        "body PROPERTY line"
    } else if line == BLOCK_HASH_LINE {
        "body #+ line inside an example or export block"
    } else if line == BABEL_CALL_LINE {
        "body babel call line"
    } else if lower.starts_with("#+title:") {
        "body page keyword line"
    } else if line.starts_with("#+") {
        "body keyword line"
    } else if line.starts_with(",#+") {
        "body escaped keyword line"
    } else if line.starts_with('|') {
        "body table line"
    } else if line.starts_with(':') && line.ends_with(':') {
        "body drawer-shaped line"
    } else if line == "-----" {
        "body rule"
    } else if line.starts_with("[fn:") {
        "body footnote"
    } else if line.is_empty() {
        "body blank line"
    } else {
        "body text line"
    }
}

pub fn dense_state() -> BoxedStrategy<Option<String>> {
    prop::option::of(prop::sample::select(vec!["TODO", "DOING", "DONE"]).prop_map(str::to_string))
        .boxed()
}

/// Tags and drawer keys no profile, query or rule gives a meaning, so the only
/// thing they exercise is that the write carries them.
const DENSE_TAG_POOL: &[&str] = &["ops", "review", "later"];
const DENSE_PROPERTY_KEYS: &[&str] = &["owner", "area", "note"];
/// Values an org drawer line holds only as a JSON string literal.
const DENSE_LITERAL_VALUES: &[&str] = &["", " padded ", "two\nlines", "\"quoted\""];

pub fn dense_tags() -> BoxedStrategy<Tags> {
    proptest::sample::subsequence(DENSE_TAG_POOL, 0..=2)
        .prop_map(|tags| Tags::from_tag_iter(tags.into_iter().map(str::to_string)))
        .boxed()
}

pub fn dense_value() -> BoxedStrategy<String> {
    prop_oneof![
        2 => proptest::string::string_regex("[a-z0-9]{1,4}").expect("valid regex"),
        1 => proptest::sample::select(DENSE_LITERAL_VALUES).prop_map(str::to_string),
    ]
    .boxed()
}

pub fn dense_properties() -> BoxedStrategy<BTreeMap<String, String>> {
    proptest::sample::subsequence(DENSE_PROPERTY_KEYS, 0..=2)
        .prop_flat_map(|keys| {
            let n = keys.len();
            proptest::collection::vec(dense_value(), n).prop_map(move |values| {
                keys.iter()
                    .map(|k| k.to_string())
                    .zip(values)
                    .collect::<BTreeMap<_, _>>()
            })
        })
        .boxed()
}

/// Tags an agent writes, some with a character org's tag grammar refuses.
const EDITED_TAG_POOL: &[&str] = &["ops", "review", "a-b", "a.b", "x@y", "n#1", "p%", "c+"];

/// Drawer keys an agent writes beyond [`DENSE_PROPERTY_KEYS`]: case variants
/// of them, keys the parser reads as typed fields, and unusual spellings.
const EDITED_PROPERTY_KEYS: &[&str] = &[
    "owner",
    "area",
    "note",
    "Owner",
    "AREA",
    "Task_State",
    "TASK_STATE_CATEGORY",
    "WIDGET_ONLY",
    "Foo+",
    "a-b",
    "x.y",
];

pub fn row_text_edit() -> BoxedStrategy<RowTextEdit> {
    prop_oneof![
        1 => dense_title().prop_map(RowTextEdit::Retitle),
        2 => edited_dense_body().prop_map(RowTextEdit::SetBody),
        1 => dense_state().prop_map(RowTextEdit::SetState),
        1 => Just(RowTextEdit::SetState(None)),
        2 => proptest::sample::subsequence(EDITED_TAG_POOL, 0..=2)
            .prop_map(|t| RowTextEdit::Retag(t.into_iter().map(str::to_string).collect())),
        2 => (
            proptest::sample::select(EDITED_PROPERTY_KEYS).prop_map(str::to_string),
            dense_value()
        )
            .prop_map(|(k, v)| RowTextEdit::SetProperty(k, v)),
        1 => (0usize..3).prop_map(RowTextEdit::DropProperty),
        1 => Just(RowTextEdit::MoveToFront),
        1 => Just(RowTextEdit::Demote),
    ]
    .boxed()
}

pub fn new_row_text() -> BoxedStrategy<NewRowText> {
    (
        dense_title(),
        edited_dense_body(),
        dense_state(),
        dense_tags(),
        dense_properties(),
    )
        .prop_map(|(title, body, state, tags, properties)| NewRowText {
            title,
            body,
            state,
            tags,
            properties,
        })
        .boxed()
}
