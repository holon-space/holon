//! Holon splits a headline into title and tags, and links a converted
//! headline's title, as Emacs org reads them. The expected readings are
//! `tests/emacs/headline_tags_org_9_7_11.txt`, written by `emacs -Q --batch`
//! (org 9.7.11) running `tests/emacs/headline_tags_org_9_7_11.el`.

use std::path::Path;

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Tags;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;
use holon_org_format::parser::linkable_title;
use holon_org_format::parser::split_headline_tags;

struct OrgReading {
    line: &'static str,
    raw: &'static str,
    tags: Vec<&'static str>,
    offset: usize,
    plain: bool,
    linked: &'static str,
    unlinked: &'static str,
}

impl OrgReading {
    /// Org holds the title as a link description, and Holon's link label
    /// rule (no surrounding whitespace) holds it too.
    fn links(&self) -> bool {
        !self.linked.is_empty() && self.raw.trim() == self.raw
    }

    /// Holon holds no empty tag.
    fn tags_holon_holds(&self) -> Vec<String> {
        self.tags
            .iter()
            .filter(|tag| !tag.is_empty())
            .map(|tag| tag.to_string())
            .collect()
    }
}

/// Rows, of all and of the plain ones, in the known-different set.
const KNOWN: usize = 69;
const KNOWN_PLAIN: usize = 68;

fn org_readings() -> Vec<OrgReading> {
    include_str!("emacs/headline_tags_org_9_7_11.txt")
        .lines()
        .map(|row| {
            let [line, raw, tags, offset, plain, linked, unlinked] = row
                .split('\x1f')
                .collect::<Vec<_>>()
                .try_into()
                .unwrap_or_else(|_| panic!("a fixture row has seven fields: {row:?}"));
            OrgReading {
                line,
                raw,
                tags: match tags {
                    "" => Vec::new(),
                    group => group[1..group.len() - 1].split(':').collect(),
                },
                offset: offset.parse().unwrap(),
                plain: plain == "1",
                linked,
                unlinked,
            }
        })
        .collect()
}

/// Holon's tag rule accepts `-` and org's `[[:alnum:]_@#%:]` does not, so a
/// headline ending in a colon group that holds a `-` differs from org until
/// the D21-D24 tag codec lands. Decided from the headline line alone: the
/// group runs from the first colon of the line's last blank-free word.
fn known_different_until_the_d21_d24_tag_codec(org: &OrgReading) -> bool {
    let line = org.line.trim_end_matches([' ', '\t']);
    let word = line.rsplit([' ', '\t']).next().unwrap();
    word.find(':').is_some_and(|colon| {
        let group = &word[colon..];
        group.ends_with(':')
            && group.contains('-')
            && group
                .chars()
                .all(|c| c == ':' || c == '-' || c.is_alphanumeric() || "_@#%".contains(c))
    })
}

/// `differs` holds, per row, the difference from org, if any. A row of the
/// known-different set must differ, every other row must agree.
fn report(readings: &[OrgReading], differs: impl Fn(&OrgReading) -> Option<String>, known: usize) {
    let mut known_different = 0;
    let wrong: Vec<String> = readings
        .iter()
        .filter_map(|org| {
            let difference = differs(org);
            if known_different_until_the_d21_d24_tag_codec(org) {
                known_different += 1;
                return match difference {
                    Some(_) => None,
                    None => Some(format!(
                        "{:?}: known different, but agrees with org",
                        org.line
                    )),
                };
            }
            difference
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} of {} rows differ from org:\n{}",
        wrong.len(),
        readings.len(),
        wrong.join("\n")
    );
    assert_eq!(
        known_different, known,
        "rows known different until the D21-D24 tag codec lands"
    );
}

#[test]
fn a_headline_splits_into_title_and_tags_as_org_reads_it() {
    let differs = |org: &OrgReading| {
        let holon = split_headline_tags(org.line);
        let expected = (org.raw.to_string(), org.tags_holon_holds());
        (holon != expected).then(|| format!("{:?}: org {expected:?}, holon {holon:?}", org.line))
    };
    report(&org_readings(), differs, KNOWN);
}

#[test]
fn a_typed_title_line_links_the_title_org_reads() {
    let differs = |org: &OrgReading| {
        let holon = linkable_title(org.line, &Tags::default());
        let expected = org.links().then(|| (org.offset, org.raw.to_string()));
        (holon != expected).then(|| format!("{:?}: org {expected:?}, holon {holon:?}", org.line))
    };
    report(&org_readings(), differs, KNOWN);
}

/// The headline a BlockToPage conversion writes: the stored title gets the
/// page link where org reads the whole title as the link's description, the
/// tags org read stay tags, and a headline org cannot read back is a loss.
#[test]
fn a_converted_headline_keeps_the_title_and_tags_org_reads() {
    let readings: Vec<OrgReading> = org_readings().into_iter().filter(|org| org.plain).collect();
    let page = EntityRef::from_uri(&EntityUri::from_raw("block:P"));
    let differs = |org: &OrgReading| {
        let drawer = ":PROPERTIES:\n:ID: h\n:END:\n";
        let file = format!("* {}\n{drawer}", org.line);
        let parsed = parse_org_file(
            Path::new("/vault/p.org"),
            &file,
            &EntityUri::no_parent(),
            Path::new("/vault"),
        )
        .unwrap_or_else(|e| panic!("parse of {file:?} failed: {e:#}"));
        let mut blocks: Vec<Block> = parsed.blocks;
        let h = blocks.iter_mut().find(|b| b.id.id() == "h").unwrap();
        h.marks = Some(
            linkable_title(&h.content, &h.tags)
                .map(|(start, label)| {
                    MarkSpan::new(
                        start,
                        start + label.chars().count(),
                        InlineMark::Link {
                            target: page.clone(),
                            label,
                        },
                    )
                })
                .into_iter()
                .collect(),
        );
        let written = OrgRenderer::render_document(
            &parsed.document,
            &blocks,
            Path::new("/vault/p.org"),
            &parsed.document.id,
        )
        .expect("org render");
        let holon = (written.text.as_str(), written.losses.is_empty());
        let converted = if org.links() {
            org.linked
        } else {
            org.unlinked
        };
        let expected = (converted != "!").then(|| format!("{converted}\n{drawer}"));
        let agrees = match &expected {
            Some(text) => holon == (text.as_str(), true),
            None => !holon.1,
        };
        (!agrees).then(|| {
            format!(
                "{:?}: org {expected:?}, holon {:?} losses {:?}",
                org.line, written.text, written.losses
            )
        })
    };
    assert!(readings.len() > 8000, "{} plain rows", readings.len());
    report(&readings, differs, KNOWN_PLAIN);
}
