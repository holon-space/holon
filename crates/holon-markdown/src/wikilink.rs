//! Obsidian `[[wikilink]]` extraction.
//!
//! Wikilinks are preserved verbatim in `block.content` for round-trip
//! fidelity. In addition, the bare names are extracted into a sidecar
//! `wikilinks` property (a JSON array of strings) so consumers can build a
//! graph of references without re-parsing markdown.
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
//! - `[[Note Name^block-id]]` — block reference
//! - `![[Note Name]]` — embed (treated like a regular reference for now;
//!   visual embedding is the renderer's concern, not the parser's)

/// A single wikilink occurrence parsed out of raw markdown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// Bare target name (everything before `#`, `^`, or `|`).
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

/// Just the unique target names, in first-occurrence order.
pub fn extract_wikilink_targets(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for link in extract_wikilinks(text) {
        if seen.insert(link.target.clone()) {
            out.push(link.target);
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
    let (target_part, block_ref) = match head.split_once('^') {
        Some((t, b)) => (t, Some(b.trim().to_string())),
        None => (head, None),
    };
    let (target, heading) = match target_part.split_once('#') {
        Some((t, h)) => (t.trim().to_string(), Some(h.trim().to_string())),
        None => (target_part.trim().to_string(), None),
    };
    if target.is_empty() && block_ref.is_none() && heading.is_none() {
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
    fn extracts_heading_and_block_ref() {
        let links = extract_wikilinks("[[Foo#Section]] [[Bar^abc123]]");
        assert_eq!(links[0].heading.as_deref(), Some("Section"));
        assert_eq!(links[1].block_ref.as_deref(), Some("abc123"));
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
    fn unique_targets() {
        let names = extract_wikilink_targets("[[Foo]] then [[Foo]] again, [[Bar]]");
        assert_eq!(names, vec!["Foo".to_string(), "Bar".to_string()]);
    }

    #[test]
    fn target_split_strips_whitespace() {
        let links = extract_wikilinks("[[ Foo Bar # Sub ]]");
        assert_eq!(links[0].target, "Foo Bar");
        assert_eq!(links[0].heading.as_deref(), Some("Sub"));
    }
}
