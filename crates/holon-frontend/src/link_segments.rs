//! Read-only ("rendered_text") link rendering: split a block's `content` into
//! alternating plain-text and link runs from its inline `Link` marks, then
//! route a click on one of those runs to its verb.
//!
//! This is the syntax-neutral, frontend-agnostic core every frontend consumes.
//! Each decision here — where the runs break, which mark kinds make a block
//! "styled", what a clicked target does — is a pure function over data both a
//! `ReactiveViewModel` and a snapshot `ViewModel` can supply, so the platform
//! layers keep only their paint and dispatch mechanics.
//!
//! Mark offsets are Unicode-scalar (`char`) positions (see
//! `holon_api::MarkSpan`); the returned segment text is sliced from `content`
//! at the corresponding byte offsets.

use holon_api::DataRow;
use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_api::link_parser::LinkTargetClassifier;
use holon_api::marks_from_json;

use crate::operations::OperationIntent;

/// One run of a block's content: either plain text or a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSegment {
    /// The exact substring of `content` this run covers.
    pub text: String,
    /// Where `text` sits in `content`. Hit-testing a clicked byte offset back
    /// to its run needs this; a frontend that only paints the runs in order
    /// can ignore it.
    pub byte_range: std::ops::Range<usize>,
    /// `Some` iff this run is a link; carries the link target.
    pub link_target: Option<EntityRef>,
}

impl ContentSegment {
    pub fn is_link(&self) -> bool {
        self.link_target.is_some()
    }
}

/// One run of a block's content carrying everything a read-mode renderer must
/// paint for it: the text, whether it is a link, and its style attributes.
///
/// [`link_content_segments`] answers only "where do the links break"; a
/// frontend that paints marks needs the style boundaries in the SAME ordered
/// pass, because both partitions describe the same character sequence and a
/// renderer emits one node per run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSegment {
    pub text: String,
    pub byte_range: std::ops::Range<usize>,
    pub link_target: Option<EntityRef>,
    pub flags: holon_api::StyleFlags,
}

impl StyledSegment {
    pub fn is_link(&self) -> bool {
        self.link_target.is_some()
    }
}

/// Split `content` at both link boundaries and style-mark boundaries, so one
/// ordered, contiguous pass of runs carries link targets and paint attributes
/// together. Runs tile `content` exactly; unstyled, unlinked stretches come
/// back with default flags and no target.
pub fn styled_link_segments(content: &str, marks: &[MarkSpan]) -> Vec<StyledSegment> {
    let links = link_content_segments(content, marks);
    let styles = holon_api::style_fingerprint(content, marks);

    let mut cuts: Vec<usize> = Vec::with_capacity(links.len() * 2 + styles.len() * 2);
    cuts.push(0);
    cuts.push(content.len());
    for s in &links {
        cuts.push(s.byte_range.start);
        cuts.push(s.byte_range.end);
    }
    for r in &styles {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    cuts.sort_unstable();
    cuts.dedup();

    cuts.windows(2)
        .map(|w| {
            let (start, end) = (w[0], w[1]);
            StyledSegment {
                text: content[start..end].to_string(),
                byte_range: start..end,
                link_target: links
                    .iter()
                    .find(|s| s.byte_range.start <= start && end <= s.byte_range.end)
                    .and_then(|s| s.link_target.clone()),
                // `style_fingerprint` omits unstyled gaps by design, so "no
                // covering run" means this stretch carries no marks.
                flags: match styles.iter().find(|r| r.start <= start && end <= r.end) {
                    Some(r) => r.flags.clone(),
                    None => holon_api::StyleFlags::default(),
                },
            }
        })
        .collect()
}

/// The link target covering `byte_offset`, if that offset lands on a link run.
pub fn link_at_offset(segments: &[ContentSegment], byte_offset: usize) -> Option<&EntityRef> {
    segments
        .iter()
        .find(|s| s.is_link() && s.byte_range.contains(&byte_offset))
        .and_then(|s| s.link_target.as_ref())
}

/// Read mode renders styled runs whenever the block has content AND any mark of
/// any kind. Gating on *Link* marks only (dogfood 2026-07-22 bug 1) drops a
/// block whose marks are Bold/Italic/Underline to plain text and silently
/// loses its formatting, even though the editor styles the same marks.
pub fn wants_styled_render(content: &str, marks: &[MarkSpan]) -> bool {
    !content.is_empty() && !marks.is_empty()
}

/// The block's inline marks, read off its entity row — the read boundary for
/// the PROJECTION path (D27.a).
///
/// Both frontends render from a `DataRow`, never from a typed `Block`, so the
/// heal in `Block::from_row` is on a different consumer's path and cannot cover
/// this one. This is where a raw `marks` column becomes spans for anything that
/// paints, which makes it the only place the `(content, marks)` pair can be
/// checked before `link_content_segments` — whose contract is to ASSERT on an
/// out-of-range span — is handed the result.
///
/// Two failure shapes, both disclosed and neither fatal, because
/// `execute_raw_sql` / `insert_data` let an agent write arbitrary JSON into
/// this column and a stray write must not make a page un-openable:
/// - marks that do not parse (malformed JSON, or an inverted span, which is a
///   hard error since D27.a) → the block renders as plain text;
/// - marks present with no `content` to check them against → dropped, since
///   nothing has validated them.
fn read_marks(entity: &DataRow) -> Option<&str> {
    match entity.get("marks") {
        Some(Value::String(s)) | Some(Value::Json(s)) if !s.is_empty() && s != "[]" => Some(s),
        _ => None,
    }
}

pub fn marks_of(entity: &DataRow) -> Vec<MarkSpan> {
    let Some(raw) = read_marks(entity) else {
        return Vec::new();
    };
    // A row missing `id` is already anomalous; the label only names the row in
    // the log, so a placeholder keeps the disclosure readable.
    let block_id = entity
        .get("id")
        .and_then(|v| v.as_string())
        .unwrap_or("<row without id>"); // ALLOW(fallback): log label only

    let mut marks = match marks_from_json(raw) {
        Ok(marks) => marks,
        // ALLOW(degraded_render): surfaced at ERROR below, never swallowed (D27.a)
        Err(e) => {
            tracing::error!(
                block_id,
                error = %e,
                marks_json = %raw,
                "block marks are unreadable; rendering this block as PLAIN TEXT \
                 (degraded). Corrupt persisted marks for this block."
            );
            return Vec::new();
        }
    };

    let Some(content) = entity.get("content").and_then(|v| v.as_string()) else {
        tracing::error!(
            block_id,
            mark_count = marks.len(),
            "block row carries marks but no content column to check them against; \
             DROPPING the marks (degraded)."
        );
        return Vec::new();
    };

    holon_api::canonicalize_marks_against(content, &mut marks, block_id);
    marks
}

/// What a click on a `rendered_text` offset must do, decided by the link kind
/// under the cursor. Kept as a value so the routing is testable without a
/// window: the whole class of bugs here is a target reaching the WRONG verb.
#[derive(Debug, PartialEq, Clone)]
pub enum LinkClickAction {
    /// Hand a web address to the platform opener. A URL names no entity, so it
    /// must never travel as a `navigation.focus` `block_id`.
    OpenUrl(String),
    /// Navigate the main region to this entity URI.
    Navigate(String),
    /// Create the page chain for a dangling wiki name, then navigate to it.
    FollowDangling(String),
    /// Not a followable link: place the caret at the clicked offset.
    SeedCaret,
}

/// Route a clicked link target to its verb.
///
/// Two gates stand between a scheme-shaped target and navigation, and they ask
/// different questions. `classifier` answers "is this scheme registered at
/// all" — an unregistered one must never mint a page. The `block` check
/// answers "can a region actually SHOW this": a focus root reaches the screen
/// only through `focus_roots JOIN block`, so a registered-but-viewless scheme
/// (`tag:`, `person:`, a sidecar entity) would navigate to an empty panel.
/// Both fall back to caret placement, the same benign outcome as clicking
/// ordinary text.
///
/// Whether the named block INSTANCE exists is not knowable here (no synchronous
/// read); `navigation.focus` owns that precondition and refuses loudly.
pub fn link_click_action(
    target: Option<&EntityRef>,
    classifier: &LinkTargetClassifier,
) -> LinkClickAction {
    match target {
        Some(EntityRef::External { url }) => LinkClickAction::OpenUrl(url.clone()),
        Some(target @ EntityRef::Scheme { .. }) => match target.entity_uri() {
            Some(uri) if classifier.resolves_entity(&uri) && uri.scheme() == "block" => {
                LinkClickAction::Navigate(uri.to_string())
            }
            _ => LinkClickAction::SeedCaret,
        },
        Some(EntityRef::Name { name }) => LinkClickAction::FollowDangling(name.clone()),
        None => LinkClickAction::SeedCaret,
    }
}

/// Build a `navigation.focus` intent for the main region targeting `block_id`.
pub fn nav_focus(block_id: String) -> OperationIntent {
    OperationIntent::new(
        EntityName::new("navigation"),
        "focus".to_string(),
        [
            ("region".to_string(), Value::String("main".to_string())),
            ("block_id".to_string(), Value::String(block_id)),
        ]
        .into_iter()
        .collect(),
    )
}

/// Convert a Unicode-scalar range `[start, end)` to a byte range within `text`.
/// Asserts the range fits within `text` (fail-loud: a mark offset past the end
/// of its own block's content is a corruption, not something to paper over).
fn scalar_range_to_bytes(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let mut char_to_byte: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(text.len());
    let total = char_to_byte.len() - 1;
    assert!(
        start <= total && end <= total,
        "link mark range {start}..{end} exceeds content length {total} chars"
    );
    char_to_byte[start]..char_to_byte[end]
}

/// Split `content` at `Link`-mark boundaries into non-overlapping, in-order
/// segments. Non-link marks are ignored (read-only link rendering only cares
/// about links; other styling is applied by the text builders). Returns a
/// single plain segment when there are no link marks.
pub fn link_content_segments(content: &str, marks: &[MarkSpan]) -> Vec<ContentSegment> {
    let mut link_spans: Vec<(std::ops::Range<usize>, &EntityRef)> = marks
        .iter()
        .filter_map(|ms| match &ms.mark {
            InlineMark::Link { target, .. } => {
                Some((scalar_range_to_bytes(content, ms.start, ms.end), target))
            }
            _ => None,
        })
        .collect();

    if link_spans.is_empty() {
        return vec![ContentSegment {
            text: content.to_string(),
            byte_range: 0..content.len(),
            link_target: None,
        }];
    }

    link_spans.sort_by_key(|(r, _)| (r.start, r.end));

    let mut segments = Vec::new();
    let mut pos = 0;
    for (range, target) in &link_spans {
        if range.start > pos {
            segments.push(ContentSegment {
                text: content[pos..range.start].to_string(),
                byte_range: pos..range.start,
                link_target: None,
            });
        }
        segments.push(ContentSegment {
            text: content[range.clone()].to_string(),
            byte_range: range.clone(),
            link_target: Some((*target).clone()),
        });
        pos = range.end;
    }
    if pos < content.len() {
        segments.push(ContentSegment {
            text: content[pos..].to_string(),
            byte_range: pos..content.len(),
            link_target: None,
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;

    use super::*;

    fn mark_link(start: usize, end: usize, target: EntityRef) -> MarkSpan {
        MarkSpan::new(
            start,
            end,
            InlineMark::Link {
                target,
                label: String::new(),
            },
        )
    }

    fn internal(id: &str) -> EntityRef {
        EntityRef::from_uri(&EntityUri::parse(id).unwrap())
    }

    #[test]
    fn no_links_yields_single_plain_segment() {
        let segs = link_content_segments("Hello world", &[]);
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].is_link());
        assert_eq!(segs[0].text, "Hello world");
    }

    #[test]
    fn non_link_marks_are_ignored() {
        let segs = link_content_segments("bold text", &[MarkSpan::new(0, 4, InlineMark::Bold)]);
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].is_link());
        assert_eq!(segs[0].text, "bold text");
    }

    #[test]
    fn single_link_splits_into_three() {
        let content = "before [[link]] after";
        let start = content.find("[[link]]").unwrap();
        let end = start + "[[link]]".len();
        let segs = link_content_segments(content, &[mark_link(start, end, internal("block:abc"))]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "before ");
        assert!(!segs[0].is_link());
        assert_eq!(segs[1].text, "[[link]]");
        assert_eq!(segs[1].link_target, Some(internal("block:abc")));
        assert_eq!(segs[2].text, " after");
        assert!(!segs[2].is_link());
    }

    #[test]
    fn multiple_links_kept_in_order() {
        let content = "A [[one]] B [[two]] C";
        let o = content.find("[[one]]").unwrap();
        let t = content.find("[[two]]").unwrap();
        let segs = link_content_segments(
            content,
            &[
                mark_link(t, t + "[[two]]".len(), internal("block:two")),
                mark_link(o, o + "[[one]]".len(), internal("block:one")),
            ],
        );
        assert_eq!(
            segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec!["A ", "[[one]]", " B ", "[[two]]", " C"],
        );
        assert_eq!(segs[1].link_target, Some(internal("block:one")));
        assert_eq!(segs[3].link_target, Some(internal("block:two")));
    }

    #[test]
    fn link_at_start_and_end() {
        let content = "[[x]] mid [[y]]";
        let x = 0;
        let y = content.find("[[y]]").unwrap();
        let segs = link_content_segments(
            content,
            &[
                mark_link(x, "[[x]]".len(), internal("block:x")),
                mark_link(y, content.chars().count(), internal("block:y")),
            ],
        );
        assert_eq!(segs.len(), 3);
        assert!(segs[0].is_link());
        assert_eq!(segs[0].text, "[[x]]");
        assert!(!segs[1].is_link());
        assert_eq!(segs[1].text, " mid ");
        assert!(segs[2].is_link());
        assert_eq!(segs[2].text, "[[y]]");
    }

    #[test]
    fn multibyte_content_slices_on_char_boundaries() {
        // "ab\u{00FC}c": a=0 b=1 ü=2(2 bytes) c=3 → 4 scalars, 5 bytes.
        let content = "ab\u{00FC}c";
        let segs = link_content_segments(content, &[mark_link(2, 3, internal("block:u"))]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "ab");
        assert_eq!(segs[1].text, "\u{00FC}");
        assert!(segs[1].is_link());
        assert_eq!(segs[2].text, "c");
    }

    #[test]
    fn dangling_name_target_preserved() {
        let content = "see [[Some Page]]";
        let start = content.find("[[Some Page]]").unwrap();
        let segs = link_content_segments(
            content,
            &[mark_link(
                start,
                content.chars().count(),
                EntityRef::Name {
                    name: "Some Page".to_string(),
                },
            )],
        );
        assert_eq!(segs.len(), 2);
        assert_eq!(
            segs[1].link_target,
            Some(EntityRef::Name {
                name: "Some Page".to_string()
            })
        );
    }

    /// F3: an inline link mid-sentence must partition into ordered segments
    /// that tile the whole content contiguously (no gaps, no overlap). This is
    /// the data a single wrapping styled text consumes, replacing the old
    /// per-segment children that stacked onto separate lines.
    #[test]
    fn inline_link_partition_tiles_content_contiguously() {
        let content = "See the Target Page reference inline in this sentence";
        let start = content.find("Target Page").unwrap();
        let end = start + "Target Page".len();
        let segs = link_content_segments(
            content,
            &[mark_link(start, end, internal("block:target-page"))],
        );

        assert_eq!(segs.first().unwrap().byte_range.start, 0);
        assert_eq!(segs.last().unwrap().byte_range.end, content.len());
        for w in segs.windows(2) {
            assert_eq!(
                w[0].byte_range.end, w[1].byte_range.start,
                "segments must tile"
            );
        }
        let links: Vec<_> = segs.iter().filter(|s| s.is_link()).collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "Target Page");
        assert!(links[0].link_target.is_some());
    }

    #[test]
    fn byte_ranges_track_multibyte_content() {
        let content = "ab\u{00FC}c";
        let segs = link_content_segments(content, &[mark_link(2, 3, internal("block:u"))]);
        assert_eq!(segs[0].byte_range, 0..2);
        assert_eq!(segs[1].byte_range, 2..4);
        assert_eq!(segs[2].byte_range, 4..5);
    }

    #[test]
    fn link_at_offset_finds_the_link_under_the_offset() {
        let content = "before [[link]] after";
        let start = content.find("[[link]]").unwrap();
        let end = start + "[[link]]".len();
        let target = internal("block:abc-123");
        let segs = link_content_segments(content, &[mark_link(start, end, target.clone())]);

        assert_eq!(link_at_offset(&segs, start), Some(&target));
        assert_eq!(link_at_offset(&segs, end - 1), Some(&target));
    }

    #[test]
    fn link_at_offset_outside_a_link_returns_none() {
        let content = "before [[link]] after";
        let start = content.find("[[link]]").unwrap();
        let end = start + "[[link]]".len();
        let segs = link_content_segments(content, &[mark_link(start, end, internal("block:abc"))]);

        assert!(link_at_offset(&segs, 0).is_none());
        assert!(link_at_offset(&segs, end).is_none());
    }

    /// Bug 1 (dogfood 2026-07-22): a block whose ONLY marks are non-link
    /// (bold/underline) MUST render styled. A gate keyed on a Link mark returns
    /// false here and the block falls through to plain text — the exact escape,
    /// still live on the web arm until this predicate was shared.
    #[test]
    fn any_mark_kind_wants_styled_render() {
        assert!(wants_styled_render(
            "bold text",
            &[MarkSpan::new(0, 4, InlineMark::Bold)]
        ));
        assert!(wants_styled_render(
            "under",
            &[MarkSpan::new(0, 4, InlineMark::Underline)]
        ));
        assert!(wants_styled_render(
            "a link",
            &[mark_link(0, 4, internal("block:x"))]
        ));
    }

    #[test]
    fn no_marks_or_no_content_stays_plain() {
        assert!(!wants_styled_render("has content", &[]));
        assert!(!wants_styled_render(
            "",
            &[MarkSpan::new(0, 4, InlineMark::Bold)]
        ));
    }

    /// The routing table, one row per link kind. `External` going anywhere near
    /// `Navigate` is the bug that blanked the main panel (BugFunnel 2026-08-08,
    /// task #17): a URL is not an entity id.
    #[test]
    fn link_click_action_routes_each_kind_to_its_verb() {
        let classifier = LinkTargetClassifier::default();

        assert_eq!(
            link_click_action(
                Some(&EntityRef::External {
                    url: "https://example.com".into()
                }),
                &classifier
            ),
            LinkClickAction::OpenUrl("https://example.com".into()),
            "an external URL must go to the platform opener, never to navigation"
        );
        assert_eq!(
            link_click_action(
                Some(&EntityRef::Scheme {
                    raw: "block:abc-123".into()
                }),
                &classifier
            ),
            LinkClickAction::Navigate("block:abc-123".into())
        );
        assert_eq!(
            link_click_action(
                Some(&EntityRef::Scheme {
                    raw: "cc-session:0f3a".into()
                }),
                &classifier
            ),
            LinkClickAction::SeedCaret,
            "an unregistered scheme must not navigate (and must not mint a page)"
        );
        assert_eq!(
            link_click_action(
                Some(&EntityRef::Scheme {
                    raw: "tag:rust".into()
                }),
                &classifier
            ),
            LinkClickAction::SeedCaret,
            "`tag` is a REGISTERED scheme with no main-panel view — navigating to it would blank \
             the panel, so it must not navigate either"
        );
        assert_eq!(
            link_click_action(
                Some(&EntityRef::Name {
                    name: "Beta Page".into()
                }),
                &classifier
            ),
            LinkClickAction::FollowDangling("Beta Page".into())
        );
        assert_eq!(
            link_click_action(None, &classifier),
            LinkClickAction::SeedCaret
        );
    }

    /// `mailto:` is an external address too, and its scheme shape is exactly
    /// what would otherwise tempt the entity-scheme branch.
    #[test]
    fn link_click_action_opens_mailto_rather_than_navigating() {
        assert_eq!(
            link_click_action(
                Some(&EntityRef::External {
                    url: "mailto:a@b.c".into()
                }),
                &LinkTargetClassifier::default()
            ),
            LinkClickAction::OpenUrl("mailto:a@b.c".into())
        );
    }

    #[test]
    fn nav_focus_targets_the_main_region() {
        let intent = nav_focus("block:abc".to_string());
        assert_eq!(intent.op_name, "focus");
        assert_eq!(
            intent.params.get("region"),
            Some(&Value::String("main".to_string()))
        );
        assert_eq!(
            intent.params.get("block_id"),
            Some(&Value::String("block:abc".to_string()))
        );
    }

    /// The web-arm escape: a block whose only marks are Bold/Underline must
    /// come back with those attributes ON a run, not as one plain segment.
    #[test]
    fn styled_link_segments_carries_non_link_marks() {
        let content = "plain bold tail";
        let segs = styled_link_segments(content, &[MarkSpan::new(6, 10, InlineMark::Bold)]);

        let bold: Vec<_> = segs.iter().filter(|s| s.flags.bold).collect();
        assert_eq!(bold.len(), 1, "exactly one bold run: {segs:?}");
        assert_eq!(bold[0].text, "bold");
        assert!(segs.iter().all(|s| !s.is_link()));
    }

    /// Both partitions in one pass: a link inside a bold span breaks at both
    /// boundaries, and the runs still tile the content exactly.
    #[test]
    fn styled_link_segments_merges_link_and_style_boundaries() {
        let content = "a [[one]] b";
        let link_start = content.find("[[one]]").unwrap();
        let segs = styled_link_segments(
            content,
            &[
                MarkSpan::new(0, content.chars().count(), InlineMark::Bold),
                mark_link(
                    link_start,
                    link_start + "[[one]]".len(),
                    internal("block:one"),
                ),
            ],
        );

        assert_eq!(segs.first().unwrap().byte_range.start, 0);
        assert_eq!(segs.last().unwrap().byte_range.end, content.len());
        for w in segs.windows(2) {
            assert_eq!(w[0].byte_range.end, w[1].byte_range.start, "runs must tile");
        }
        assert!(segs.iter().all(|s| s.flags.bold), "bold covers everything");
        let links: Vec<_> = segs.iter().filter(|s| s.is_link()).collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "[[one]]");
    }

    #[test]
    fn marks_of_reads_the_entity_row_and_treats_empty_as_none() {
        let mut row = DataRow::new();
        assert!(marks_of(&row).is_empty());
        row.insert("marks".to_string(), Value::String("[]".to_string()));
        assert!(marks_of(&row).is_empty());
        row.insert("marks".to_string(), Value::String(String::new()));
        assert!(marks_of(&row).is_empty());
    }

    /// A row with `marks` but no `content` column: the pair is incomplete, so
    /// the marks cannot be checked against anything. Drop them and disclose
    /// rather than render spans nothing has validated.
    #[test]
    fn marks_of_drops_marks_when_the_row_carries_no_content() {
        let mut row = DataRow::new();
        row.insert(
            "id".to_string(),
            Value::String("block:no-content".to_string()),
        );
        row.insert(
            "marks".to_string(),
            Value::String(r#"[{"start":0,"end":2,"kind":"Bold"}]"#.to_string()),
        );
        assert!(
            marks_of(&row).is_empty(),
            "marks without content must be dropped, not rendered"
        );
    }

    /// The projection-path read boundary (D27.a). `marks_of` is where a
    /// `DataRow` becomes marks for BOTH frontends, and it is the only boundary
    /// on that path — the typed `Block::from_row` heal is a different consumer
    /// — so the range-vs-content heal has to happen here or nowhere.
    #[test]
    fn marks_of_heals_a_span_that_outlives_its_content() {
        let mut row = DataRow::new();
        row.insert("id".to_string(), Value::String("block:corrupt".to_string()));
        row.insert("content".to_string(), Value::String("abc".to_string()));
        row.insert(
            "marks".to_string(),
            Value::String(r#"[{"start":0,"end":99,"kind":"Bold"}]"#.to_string()),
        );
        assert_eq!(
            marks_of(&row),
            vec![MarkSpan::new(0, 3, InlineMark::Bold)],
            "an out-of-range span must be clamped to the content, not passed through"
        );
    }

    /// Unreadable marks must DEGRADE VISIBLY, not abort. An inverted span is a
    /// hard parse error since D27.a, and `execute_raw_sql` / `insert_data` let
    /// an agent write one, so `.expect()`ing here would let a stray MCP write
    /// panic the app. Plain text plus a loud ERROR is priority 2; a crash is
    /// worse than the corruption it reports.
    ///
    /// Asserted on BEHAVIOUR rather than on captured ERROR events: this unit
    /// harness installs no subscriber, so a capture assertion would pass
    /// vacuously (the `SpanCollector::global()` gotcha).
    #[test]
    fn marks_of_degrades_to_plain_text_when_the_marks_do_not_parse() {
        let mut row = DataRow::new();
        row.insert(
            "id".to_string(),
            Value::String("block:inverted".to_string()),
        );
        row.insert("content".to_string(), Value::String("hello".to_string()));
        row.insert(
            "marks".to_string(),
            Value::String(r#"[{"start":5,"end":2,"kind":"Bold"}]"#.to_string()),
        );
        assert!(
            marks_of(&row).is_empty(),
            "an inverted span must render the block plain, not abort the paint"
        );

        row.insert(
            "marks".to_string(),
            Value::String("{not json at all".to_string()),
        );
        assert!(
            marks_of(&row).is_empty(),
            "malformed JSON must render the block plain, not abort the paint"
        );
    }
}
