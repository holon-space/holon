//! Obsidian callout extraction.
//!
//! A callout is a blockquote whose first line opens with a `[!type]` marker:
//!
//! ```markdown
//! > [!note] Optional title
//! > body line
//! ```
//!
//! An optional `-` (collapsed) or `+` (expanded) immediately after the
//! `]` marks the callout collapsible. As with the other inline features the
//! raw blockquote stays verbatim in `block.content`; this module only lifts
//! the header metadata into the `callouts` sidecar property when the dialect
//! switch is on.

use serde::Serialize;

/// Parsed metadata from a single callout header line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Callout {
    /// Callout type, lowercased (`note`, `warning`, `tip`, …).
    pub kind: String,
    /// Title text after the marker, if any.
    pub title: Option<String>,
    /// `None` if not collapsible, `Some(true)` if collapsed by default
    /// (`[!type]-`), `Some(false)` if expanded by default (`[!type]+`).
    pub folded: Option<bool>,
}

/// Find every callout header in `text`, in document order. Body lines of a
/// callout (the continuation `>` lines) are skipped so a nested marker is not
/// double-counted.
pub fn extract_callouts(text: &str) -> Vec<Callout> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(callout) = parse_header(line) else {
            continue;
        };
        while lines.peek().is_some_and(|next| is_blockquote_line(next)) {
            lines.next();
        }
        out.push(callout);
    }
    out
}

fn parse_header(line: &str) -> Option<Callout> {
    let after = line.trim_start().strip_prefix('>')?.trim_start();
    let after = after.strip_prefix("[!")?;
    let close = after.find(']')?;
    let kind_raw = &after[..close];
    if kind_raw.is_empty()
        || !kind_raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    let rest = &after[close + 1..];
    let (folded, title_part) = match rest.as_bytes().first() {
        Some(b'-') => (Some(true), &rest[1..]),
        Some(b'+') => (Some(false), &rest[1..]),
        _ => (None, rest),
    };
    let title = title_part.trim();
    Some(Callout {
        kind: kind_raw.to_ascii_lowercase(),
        title: (!title.is_empty()).then(|| title.to_string()),
        folded,
    })
}

fn is_blockquote_line(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_callout_with_title() {
        let c = extract_callouts("> [!note] Heads up\n> body text\n");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, "note");
        assert_eq!(c[0].title.as_deref(), Some("Heads up"));
        assert_eq!(c[0].folded, None);
    }

    #[test]
    fn collapsed_and_expanded_markers() {
        let collapsed = extract_callouts("> [!warning]- secret\n> hidden\n");
        assert_eq!(collapsed[0].folded, Some(true));
        assert_eq!(collapsed[0].title.as_deref(), Some("secret"));

        let expanded = extract_callouts("> [!tip]+ shown\n");
        assert_eq!(expanded[0].folded, Some(false));
    }

    #[test]
    fn titleless_callout() {
        let c = extract_callouts("> [!info]\n> just body\n");
        assert_eq!(c[0].kind, "info");
        assert_eq!(c[0].title, None);
        assert_eq!(c[0].folded, None);
    }

    #[test]
    fn type_is_lowercased() {
        let c = extract_callouts("> [!NOTE] x\n");
        assert_eq!(c[0].kind, "note");
    }

    #[test]
    fn two_separated_callouts_both_found() {
        let c = extract_callouts("> [!a] one\n> body\n\nbetween\n\n> [!b] two\n");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].kind, "a");
        assert_eq!(c[1].kind, "b");
    }

    #[test]
    fn nested_marker_in_body_not_double_counted() {
        let c = extract_callouts("> [!outer] top\n> > [!inner] nested\n> more\n");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, "outer");
    }

    #[test]
    fn plain_blockquote_is_not_a_callout() {
        assert!(extract_callouts("> just a quote\n> more\n").is_empty());
    }
}
