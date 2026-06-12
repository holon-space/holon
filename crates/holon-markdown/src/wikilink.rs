//! Obsidian `[[wikilink]]` extraction.
//!
//! Wikilinks are preserved verbatim in `block.content` for round-trip
//! fidelity. In addition, the bare names are extracted into sidecar
//! properties (JSON arrays of strings) so consumers can build a graph of
//! references without re-parsing markdown. [`classify_wikilinks`] splits each
//! occurrence into three orthogonal kinds so the parser can gate each behind
//! its own dialect switch:
//!
//! - **cross-file links** — `[[Note]]` → `ClassifiedWikilinks::links`
//! - **embeds** — `![[Note]]` → `ClassifiedWikilinks::embeds`
//! - **self-links** — `[[#Heading]]`, `[[#^block]]` → `ClassifiedWikilinks::self_links`
//!
//! The format adapter intentionally does **not** resolve names to
//! `EntityUri`. Resolution requires the vault-level filename index — the
//! adapter is single-file scope. A higher layer (e.g. the sync controller
//! or a dedicated link resolver) maps `"Note Name"` →
//! `EntityUri::file("relative/path.md")` once it has the directory listing.
//!
//! Supported forms:
//! - `[[Note Name]]`
//! - `[[Note Name|Display]]` — alias text
//! - `[[Note Name#Heading]]` — heading reference
//! - `[[Note Name#^block-id]]` — canonical block reference
//! - `[[Note Name^block-id]]` — legacy block reference (no `#`)
//! - `[[#Heading]]`, `[[#^block-id]]` — self (same-file) references
//! - `![[Note Name]]` — embed

use std::collections::HashSet;

/// A single wikilink occurrence parsed out of raw markdown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// Bare target name (everything before `#`, `^`, or `|`). Empty for a
    /// same-file self-link such as `[[#Heading]]`.
    pub target: String,
    /// `#heading` fragment, without the leading `#`.
    pub heading: Option<String>,
    /// `^block-id` fragment, without the leading `^`.
    pub block_ref: Option<String>,
    /// Pipe-aliased display text, if any.
    pub display: Option<String>,
    /// `true` for `![[...]]` embeds.
    pub embed: bool,
}

impl Wikilink {
    /// A self-link references a location within the same file (`[[#Heading]]`
    /// or `[[#^block]]`); its `target` is empty.
    pub fn is_self_link(&self) -> bool {
        self.target.is_empty()
    }

    /// Canonical same-file reference fragment for a self-link (`#Heading` or
    /// `#^block`). `None` for links that name a target.
    pub fn self_reference(&self) -> Option<String> {
        if !self.is_self_link() {
            return None;
        }
        match (&self.block_ref, &self.heading) {
            (Some(b), _) => Some(format!("#^{b}")),
            (None, Some(h)) => Some(format!("#{h}")),
            (None, None) => None,
        }
    }
}

/// Wikilink occurrences grouped by kind, each deduped in first-occurrence
/// order. The parser writes each non-empty group behind its own dialect
/// switch, so the groups stay independent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClassifiedWikilinks {
    /// Plain cross-file link targets (`[[Note]]`).
    pub links: Vec<String>,
    /// Embed targets (`![[Note]]`).
    pub embeds: Vec<String>,
    /// Same-file reference fragments (`#Heading`, `#^block`).
    pub self_links: Vec<String>,
}

/// Scan `text` for wikilinks. Order is preserved. Duplicates are kept —
/// callers that want unique targets should dedup themselves.
pub fn extract_wikilinks(text: &str) -> Vec<Wikilink> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let embed = i > 0 && bytes[i - 1] == b'!' && bytes[i] == b'[';
        if bytes[i] == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some(end) = find_close(&bytes[i + 2..]) {
                let inner = &text[i + 2..i + 2 + end];
                if let Some(link) = parse_inner(inner, embed) {
                    out.push(link);
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Split every wikilink occurrence into cross-file links, embeds, and
/// self-links. Each group is deduped, preserving first-occurrence order.
pub fn classify_wikilinks(text: &str) -> ClassifiedWikilinks {
    let mut out = ClassifiedWikilinks::default();
    let mut seen_links = HashSet::new();
    let mut seen_embeds = HashSet::new();
    let mut seen_self = HashSet::new();

    for link in extract_wikilinks(text) {
        if let Some(reference) = link.self_reference() {
            if seen_self.insert(reference.clone()) {
                out.self_links.push(reference);
            }
        } else if link.embed {
            if seen_embeds.insert(link.target.clone()) {
                out.embeds.push(link.target);
            }
        } else if seen_links.insert(link.target.clone()) {
            out.links.push(link.target);
        }
    }
    out
}

fn find_close(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            return Some(i);
        }
        if bytes[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

fn parse_inner(inner: &str, embed: bool) -> Option<Wikilink> {
    if inner.is_empty() {
        return None;
    }
    let (head, display) = match inner.split_once('|') {
        Some((h, d)) => (h, Some(d.to_string())),
        None => (inner, None),
    };
    // Split on `#` first so the canonical block ref `Note#^id` parses with the
    // heading slot empty. A `#` fragment beginning with `^` is a block ref;
    // otherwise it is a heading. With no `#`, a trailing `^id` is the legacy
    // block-ref form.
    let (target_part, heading, block_ref) = if let Some((t, frag)) = head.split_once('#') {
        match frag.trim().strip_prefix('^') {
            Some(b) => (t, None, Some(b.trim().to_string())),
            None => {
                let h = frag.trim();
                (t, (!h.is_empty()).then(|| h.to_string()), None)
            }
        }
    } else if let Some((t, b)) = head.split_once('^') {
        (t, None, Some(b.trim().to_string()))
    } else {
        (head, None, None)
    };
    let target = target_part.trim().to_string();
    if target.is_empty() && heading.is_none() && block_ref.is_none() {
        return None;
    }
    Some(Wikilink {
        target,
        heading,
        block_ref,
        display,
        embed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_wikilinks() {
        let links = extract_wikilinks("see [[Foo]] and [[Bar Baz]]");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Foo");
        assert_eq!(links[1].target, "Bar Baz");
    }

    #[test]
    fn extracts_aliased_link() {
        let links = extract_wikilinks("[[Foo|the foo]]");
        assert_eq!(links[0].target, "Foo");
        assert_eq!(links[0].display.as_deref(), Some("the foo"));
    }

    #[test]
    fn extracts_heading_and_legacy_block_ref() {
        let links = extract_wikilinks("[[Foo#Section]] [[Bar^abc123]]");
        assert_eq!(links[0].heading.as_deref(), Some("Section"));
        assert!(links[0].block_ref.is_none());
        assert_eq!(links[1].block_ref.as_deref(), Some("abc123"));
        assert!(links[1].heading.is_none());
    }

    #[test]
    fn canonical_block_ref_splits_hash_before_caret() {
        // `[[note#^id]]` — `#` introduces the fragment, `^` marks it a block
        // ref. The heading slot must stay empty (the historical bug filled it
        // with "").
        let links = extract_wikilinks("[[Note#^blk-1]]");
        assert_eq!(links[0].target, "Note");
        assert_eq!(links[0].block_ref.as_deref(), Some("blk-1"));
        assert!(links[0].heading.is_none());
    }

    #[test]
    fn self_links_have_empty_target() {
        let heading = &extract_wikilinks("[[#Section]]")[0];
        assert!(heading.is_self_link());
        assert_eq!(heading.self_reference().as_deref(), Some("#Section"));

        let block = &extract_wikilinks("[[#^xyz]]")[0];
        assert!(block.is_self_link());
        assert_eq!(block.self_reference().as_deref(), Some("#^xyz"));
    }

    #[test]
    fn extracts_embed() {
        let links = extract_wikilinks("![[Image.png]]");
        assert!(links[0].embed);
        assert_eq!(links[0].target, "Image.png");
    }

    #[test]
    fn ignores_unclosed_link() {
        let links = extract_wikilinks("incomplete [[Foo and more text");
        assert!(links.is_empty());
    }

    #[test]
    fn target_split_strips_whitespace() {
        let links = extract_wikilinks("[[ Foo Bar # Sub ]]");
        assert_eq!(links[0].target, "Foo Bar");
        assert_eq!(links[0].heading.as_deref(), Some("Sub"));
    }

    #[test]
    fn classify_separates_kinds_and_dedups() {
        let c = classify_wikilinks(
            "[[Foo]] [[Foo]] ![[Img.png]] [[#Sec]] [[Bar|alias]] ![[Img.png]] [[#^b1]]",
        );
        assert_eq!(c.links, vec!["Foo".to_string(), "Bar".to_string()]);
        assert_eq!(c.embeds, vec!["Img.png".to_string()]);
        assert_eq!(c.self_links, vec!["#Sec".to_string(), "#^b1".to_string()]);
    }

    #[test]
    fn classify_is_empty_for_plain_text() {
        let c = classify_wikilinks("no links at all");
        assert!(c.links.is_empty() && c.embeds.is_empty() && c.self_links.is_empty());
    }
}
