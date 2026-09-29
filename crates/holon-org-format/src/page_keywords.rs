//! A page's title and task keywords, as org reads them: the title from every
//! `#+TITLE:` line of the file, joined with one space (org-get-title scans the
//! whole buffer, blocks included); the task keywords from every TYP_TODO, then
//! TODO, then SEQ_TODO keyword element (org-collect-keywords, none inside an
//! example, export, source, verse or comment block). The lines stay where the
//! author wrote them; a value Holon changes is written back into them.

use std::collections::HashMap;

use holon_api::RenderLoss;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::TaskState;
use orgize::SyntaxKind;
use orgize::SyntaxNode;

use crate::models::OrgDocumentExt;

/// The value of a keyword line whose key is one of `keys`, in any case.
pub(crate) fn keyword_value<'a>(line: &'a str, keys: &[&str]) -> Option<&'a str> {
    let (key, value) = line.trim().strip_prefix("#+")?.split_once(':')?;
    keys.iter()
        .any(|k| key.eq_ignore_ascii_case(k))
        .then(|| value.trim())
}

/// The org keyword kind of a task-keyword line; org orders the ring by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RingKind {
    Type,
    Todo,
    Sequence,
}

/// One keyword of a task-keyword line: the keyword, whether it is a done
/// state, and the word as authored (with its fast-access key, `NEXT(n)`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct RingWord {
    keyword: String,
    done: bool,
    raw: String,
}

impl RingWord {
    fn new(raw: &str, done: bool) -> Self {
        let keyword = match (raw.find('('), raw.ends_with(')')) {
            (Some(open), true) => &raw[..open],
            _ => raw,
        };
        Self {
            keyword: keyword.to_string(),
            done,
            raw: raw.to_string(),
        }
    }
}

/// A task-keyword line's keywords: before `|` active, after it done; with no
/// `|`, the last one is done, as org reads it. A trailing `(...)` is a
/// fast-access key, not part of the keyword (org-remove-keyword-keys).
fn ring_line(line: &str) -> Option<(RingKind, Vec<RingWord>)> {
    let (kind, value) = [
        (RingKind::Type, "TYP_TODO"),
        (RingKind::Todo, "TODO"),
        (RingKind::Sequence, "SEQ_TODO"),
    ]
    .into_iter()
    .find_map(|(kind, key)| keyword_value(line, &[key]).map(|v| (kind, v)))?;
    let words: Vec<&str> = value.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let keywords = match words.iter().position(|w| *w == "|") {
        Some(bar) => words[..bar]
            .iter()
            .map(|w| RingWord::new(w, false))
            .chain(
                words[bar + 1..]
                    .iter()
                    .filter(|w| **w != "|")
                    .map(|w| RingWord::new(w, true)),
            )
            .collect(),
        None => {
            let last = words.len() - 1;
            words
                .iter()
                .enumerate()
                .map(|(i, w)| RingWord::new(w, i == last))
                .collect()
        }
    };
    Some((kind, keywords))
}

/// The lines of `text` org reads as `#+TITLE:` lines: indentation allowed, key
/// in any case, anywhere in the file (org-macro--find-keyword-value).
pub(crate) fn title_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| keyword_value(line.trim_start_matches([' ', '\t']), &["TITLE"]).is_some())
}

/// The page title the lines declare: every TITLE value joined with one space,
/// trimmed; none when that is empty.
pub(crate) fn title_of<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let title = lines
        .into_iter()
        .filter_map(|line| keyword_value(line, &["TITLE"]))
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// The task-keyword ring the lines declare, in org's order; none when no line
/// declares a keyword.
pub(crate) fn ring_of<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<Vec<TaskState>> {
    let mut rings: Vec<(RingKind, Vec<RingWord>)> =
        lines.into_iter().filter_map(ring_line).collect();
    rings.sort_by_key(|(kind, _)| *kind);
    let states: Vec<TaskState> = rings
        .into_iter()
        .flat_map(|(_, words)| words)
        .map(|word| {
            if word.done {
                TaskState::done(&word.keyword)
            } else {
                TaskState::active(&word.keyword)
            }
        })
        .collect();
    (!states.is_empty()).then_some(states)
}

/// The keyword elements of a parsed file, in file order: orgize, as org,
/// reads none inside an example, export, source, verse or comment block.
pub(crate) fn document_keyword_lines(root: &SyntaxNode) -> Vec<String> {
    root.descendants()
        .filter(|n| n.kind() == SyntaxKind::KEYWORD)
        .map(|n| n.to_string())
        .collect()
}

/// A page's title and task keywords as a file reads.
pub(crate) struct Reading {
    title: Option<String>,
    ring: Vec<(String, bool)>,
}

impl Reading {
    /// The values the parser read into `document`.
    pub(crate) fn of(document: &Block) -> Self {
        Self {
            title: document.file_title(),
            ring: declared(document.todo_keywords()),
        }
    }

    /// The values `lines` alone declare.
    pub(crate) fn of_lines<'a>(lines: impl IntoIterator<Item = &'a str> + Clone) -> Self {
        Self {
            title: title_of(lines.clone()),
            ring: declared(ring_of(lines)),
        }
    }
}

fn declared(ring: Option<Vec<TaskState>>) -> Vec<(String, bool)> {
    ring.unwrap_or_default()
        .into_iter()
        .filter(|s| s.is_active() || s.is_done())
        .map(|s| (s.keyword.clone(), s.is_done()))
        .collect()
}

/// Where a page keyword line is kept.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LineAt {
    /// The page's header lines, by index.
    Header(usize),
    /// A block's keyword lines, by index.
    Block(EntityUri, usize),
}

/// How the renderer writes the page's keyword lines: each line it rewrites
/// (`Some`) or removes (`None`), and the lines it adds to the header.
#[derive(Default)]
pub(crate) struct Edits {
    pub(crate) lines: HashMap<LineAt, Option<String>>,
    pub(crate) header_additions: Vec<String>,
}

impl Edits {
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.header_additions.is_empty()
    }
}

/// The edits that make the page's keyword lines (`lines`, in file order) read
/// as the page's title and task keywords, where the file written without
/// edits reads as `current`. Lines Holon did not change stay as authored;
/// every removed line is recorded in `losses`.
pub(crate) fn edits(
    page: &Block,
    current: &Reading,
    lines: &[(LineAt, String)],
    losses: &mut Vec<RenderLoss>,
) -> Edits {
    let mut edits = Edits::default();
    let mut remove = |edits: &mut Edits, at: &LineAt, raw: &str, why: &str| {
        let detail = format!("the line {:?} {why}; it is removed", raw.trim());
        tracing::warn!(page = %page.id, "org render: {detail}");
        losses.push(RenderLoss {
            block: page.id.clone(),
            detail,
        });
        edits.lines.insert(at.clone(), None);
    };

    let wanted_title = page
        .file_title()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    if current.title != wanted_title {
        let title_lines: Vec<&(LineAt, String)> = lines
            .iter()
            .filter(|(_, raw)| keyword_value(raw, &["TITLE"]).is_some())
            .collect();
        match (&wanted_title, title_lines.split_first()) {
            (Some(title), Some(((at, raw), rest))) => {
                edits.lines.insert(
                    at.clone(),
                    Some(crate::models::in_authored_line(
                        raw,
                        &format!("#+TITLE: {title}\n"),
                    )),
                );
                for (at, raw) in rest {
                    remove(
                        &mut edits,
                        at,
                        raw,
                        &format!("held part of the old title; the page is now titled {title:?}"),
                    );
                }
            }
            (None, _) => {
                for (at, raw) in &title_lines {
                    remove(&mut edits, at, raw, "held the title the page no longer has");
                }
            }
            (Some(title), None) => edits.header_additions.push(format!("#+TITLE: {title}\n")),
        }
    }

    let wanted_ring = declared(page.todo_keywords());
    if current.ring != wanted_ring {
        let ring_lines: Vec<RingLine> = lines
            .iter()
            .filter_map(|(at, raw)| ring_line(raw).map(|(kind, words)| (at, raw, kind, words)))
            .collect();
        let state: HashMap<&str, bool> = wanted_ring
            .iter()
            .map(|(k, done)| (k.as_str(), *done))
            .collect();
        let added: Vec<RingWord> = wanted_ring
            .iter()
            .filter(|(k, _)| {
                !ring_lines
                    .iter()
                    .any(|(_, _, _, words)| words.iter().any(|w| &w.keyword == k))
            })
            .map(|(k, done)| RingWord::new(k, *done))
            .collect();
        let target = ring_lines
            .iter()
            .position(|(_, _, kind, _)| *kind != RingKind::Type);
        for (i, (at, raw, _, words)) in ring_lines.iter().enumerate() {
            let mut kept: Vec<RingWord> = words
                .iter()
                .filter_map(|w| {
                    state.get(w.keyword.as_str()).map(|done| RingWord {
                        done: *done,
                        ..w.clone()
                    })
                })
                .collect();
            if Some(i) == target {
                kept.extend(added.iter().cloned());
            }
            if kept == *words {
                continue;
            }
            if kept.is_empty() {
                remove(
                    &mut edits,
                    at,
                    raw,
                    "declared no task keyword the page still has",
                );
                continue;
            }
            edits.lines.insert(
                (*at).clone(),
                Some(crate::models::in_authored_line(
                    raw,
                    &ring_declaration(&kept),
                )),
            );
        }
        if target.is_none() && !added.is_empty() {
            edits.header_additions.push(ring_declaration(&added));
        }
    }
    edits
}

/// A task-keyword line: where it is kept, its raw text, its kind and keywords.
type RingLine<'a> = (&'a LineAt, &'a String, RingKind, Vec<RingWord>);

/// `#+TODO: active | done` for `words`, each as authored.
fn ring_declaration(words: &[RingWord]) -> String {
    let part = |done: bool| -> Vec<&str> {
        words
            .iter()
            .filter(|w| w.done == done)
            .map(|w| w.raw.as_str())
            .collect()
    };
    let (active, done) = (part(false), part(true));
    let mut line = "#+TODO:".to_string();
    if !active.is_empty() {
        line.push_str(&format!(" {}", active.join(" ")));
    }
    if !done.is_empty() {
        line.push_str(&format!(" | {}", done.join(" ")));
    }
    line.push('\n');
    line
}
