//! Link parsing for org-mode content
//!
//! Extracts `[[target][text]]` and bare `[[target]]` style links from org-mode content.
//! Classifies each link target and computes deterministic entity IDs for creation intents.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::entity_uri::EntityUri;

/// Classification of a link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Already resolved: `[[doc:uuid]]` or `[[block:uuid]]`
    Resolved(EntityUri),
    /// Creation intent: `[[Projects/New thing]]` → computed deterministic ID
    CreationIntent {
        scheme: String,
        path: String,
        name: String,
        parent_path: Option<String>,
        target_id: EntityUri,
    },
    /// External URL: `[[https://...]]`
    External(String),
}

impl LinkTarget {
    /// Returns the target EntityUri if this is a resolved or creation-intent link.
    pub fn entity_id(&self) -> Option<&EntityUri> {
        match self {
            LinkTarget::Resolved(uri) => Some(uri),
            LinkTarget::CreationIntent { target_id, .. } => Some(target_id),
            LinkTarget::External(_) => None,
        }
    }
}

/// Represents a link found in org-mode content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Target URI or page name (e.g., "doc:uuid", "Projects/New thing")
    pub target: String,
    /// Display text (equals target for bare `[[target]]` links)
    pub text: String,
    /// Start position in content (byte offset)
    pub start: usize,
    /// End position in content (byte offset)
    pub end: usize,
    /// Classified target with deterministic ID
    pub classified: LinkTarget,
}

/// Matches `[[target][text]]` — described link with display text.
static DESCRIBED_LINK_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\[([^\]]+)\]\]").unwrap());

/// Matches bare `[[target]]` — no display text, no inner brackets.
static BARE_LINK_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]\[]+)\]\]").unwrap());

/// Normalize a path string for deterministic hashing.
/// Lowercase, trim whitespace, collapse internal whitespace runs to single space.
pub fn normalize_for_hash(input: &str) -> String {
    let trimmed = input.trim().to_lowercase();
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    result
}

/// Compute a deterministic EntityUri from a scheme and normalized path.
///
/// Uses blake3 to hash the normalized path, then formats as a UUID-style string
/// under the given scheme. Same input always produces the same output.
pub fn deterministic_entity_id(scheme: &str, normalized_path: &str) -> EntityUri {
    let hash = blake3::hash(normalized_path.as_bytes());
    let bytes = hash.as_bytes();
    // Format first 16 bytes as UUID-style: 8-4-4-4-12
    let uuid_str = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    );
    EntityUri::new(scheme, &uuid_str)
}

/// Infer entity scheme from the first path segment.
fn infer_scheme(first_segment: &str) -> Option<&'static str> {
    match first_segment.to_lowercase().as_str() {
        "person" => Some("person"),
        _ => None, // default to "doc"
    }
}

/// Classify a raw link target string.
pub fn classify_link(target: &str) -> LinkTarget {
    // External URLs
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        return LinkTarget::External(target.to_string());
    }

    // Already resolved: starts with known entity scheme followed by ':'
    if target.starts_with("doc:") || target.starts_with("block:") {
        let uri = EntityUri::from_raw(target);
        return LinkTarget::Resolved(uri);
    }

    // Creation intent: wiki-style link like "Projects/New thing" or "PageName"
    let segments: Vec<&str> = target.split('/').collect();
    let name = segments.last().unwrap().to_string();
    let parent_path = if segments.len() > 1 {
        Some(segments[..segments.len() - 1].join("/"))
    } else {
        None
    };

    let scheme = segments
        .first()
        .and_then(|s| infer_scheme(s))
        .unwrap_or("doc");

    let normalized = normalize_for_hash(target);
    let target_id = deterministic_entity_id(scheme, &normalized);

    LinkTarget::CreationIntent {
        scheme: scheme.to_string(),
        path: target.to_string(),
        name,
        parent_path,
        target_id,
    }
}

/// Extract all `[[target][text]]` and `[[target]]` links from org-mode content.
///
/// For bare `[[target]]` links, `text` is set equal to `target`.
/// Links are returned in order of appearance.
pub fn extract_links(content: &str) -> Vec<Link> {
    let mut described_ranges: Vec<(usize, usize)> = Vec::new();
    let mut links = Vec::new();

    for mat in DESCRIBED_LINK_REGEX.find_iter(content) {
        let captures = DESCRIBED_LINK_REGEX.captures(mat.as_str()).unwrap();
        let target = captures[1].to_string();
        let text = captures[2].to_string();
        described_ranges.push((mat.start(), mat.end()));
        let classified = classify_link(&target);
        links.push(Link {
            target,
            text,
            start: mat.start(),
            end: mat.end(),
            classified,
        });
    }

    for mat in BARE_LINK_REGEX.find_iter(content) {
        let overlaps = described_ranges
            .iter()
            .any(|&(start, end)| mat.start() >= start && mat.end() <= end);
        if overlaps {
            continue;
        }
        let captures = BARE_LINK_REGEX.captures(mat.as_str()).unwrap();
        let target = captures[1].to_string();
        let classified = classify_link(&target);
        links.push(Link {
            target: target.clone(),
            text: target,
            start: mat.start(),
            end: mat.end(),
            classified,
        });
    }

    links.sort_by_key(|l| l.start);
    links
}

/// Extract unique link targets from content.
///
/// Returns a set of all unique target URIs found in links.
pub fn extract_link_targets(content: &str) -> HashSet<String> {
    extract_links(content)
        .iter()
        .map(|link| link.target.clone())
        .collect()
}

/// Replace links in content with plain text (keeping the display text).
pub fn strip_links(content: &str) -> String {
    let links = extract_links(content);
    let mut result = content.to_string();

    // Replace in reverse order to maintain correct positions
    for link in links.iter().rev() {
        result.replace_range(link.start..link.end, &link.text);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_described_link() {
        let content = "This is a [[doc:uuid-123][link to block]] in text.";
        let links = extract_links(content);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "doc:uuid-123");
        assert_eq!(links[0].text, "link to block");
        assert!(
            matches!(&links[0].classified, LinkTarget::Resolved(uri) if uri.as_str() == "doc:uuid-123")
        );
    }

    #[test]
    fn test_extract_bare_link() {
        let content = "See [[ProjectNotes]] for details.";
        let links = extract_links(content);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "ProjectNotes");
        assert_eq!(links[0].text, "ProjectNotes");
        assert!(
            matches!(&links[0].classified, LinkTarget::CreationIntent { scheme, name, .. } if scheme == "doc" && name == "ProjectNotes")
        );
    }

    #[test]
    fn test_extract_mixed_links() {
        let content = "A [[PageOne]] then [[doc:2][described]] then [[PageThree]].";
        let links = extract_links(content);

        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, "PageOne");
        assert_eq!(links[0].text, "PageOne");
        assert_eq!(links[1].target, "doc:2");
        assert_eq!(links[1].text, "described");
        assert_eq!(links[2].target, "PageThree");
        assert_eq!(links[2].text, "PageThree");
    }

    #[test]
    fn test_bare_link_not_confused_with_described() {
        let content = "Only [[target][text]] here.";
        let links = extract_links(content);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "target");
        assert_eq!(links[0].text, "text");
    }

    #[test]
    fn test_extract_multiple_described_links() {
        let content = "First [[doc:1][one]] and second [[doc:2][two]].";
        let links = extract_links(content);

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "doc:1");
        assert_eq!(links[1].target, "doc:2");
    }

    #[test]
    fn test_extract_link_targets() {
        let content = "[[doc:a][A]] and [[PageB]] and [[doc:a][A again]].";
        let targets = extract_link_targets(content);

        assert_eq!(targets.len(), 2);
        assert!(targets.contains("doc:a"));
        assert!(targets.contains("PageB"));
    }

    #[test]
    fn test_strip_links() {
        let content = "See [[doc:123][this block]] and [[PageName]] for details.";
        let stripped = strip_links(content);

        assert_eq!(stripped, "See this block and PageName for details.");
    }

    #[test]
    fn test_no_links() {
        let content = "Plain text without any links.";
        let links = extract_links(content);
        assert!(links.is_empty());
    }

    #[test]
    fn test_positions_are_correct() {
        let content = "A [[Page]] B";
        let links = extract_links(content);

        assert_eq!(links.len(), 1);
        assert_eq!(&content[links[0].start..links[0].end], "[[Page]]");
    }

    // --- New tests for classification + deterministic IDs ---

    #[test]
    fn test_classify_resolved_doc() {
        let target = classify_link("doc:existing-uuid");
        assert!(matches!(target, LinkTarget::Resolved(uri) if uri.as_str() == "doc:existing-uuid"));
    }

    #[test]
    fn test_classify_resolved_block() {
        let target = classify_link("block:some-id");
        assert!(matches!(target, LinkTarget::Resolved(uri) if uri.as_str() == "block:some-id"));
    }

    #[test]
    fn test_classify_external_https() {
        let target = classify_link("https://example.com");
        assert!(matches!(target, LinkTarget::External(url) if url == "https://example.com"));
    }

    #[test]
    fn test_classify_external_mailto() {
        let target = classify_link("mailto:test@example.com");
        assert!(matches!(target, LinkTarget::External(url) if url == "mailto:test@example.com"));
    }

    #[test]
    fn test_classify_creation_intent_simple() {
        let target = classify_link("ProjectNotes");
        match &target {
            LinkTarget::CreationIntent {
                scheme,
                path,
                name,
                parent_path,
                target_id,
            } => {
                assert_eq!(scheme, "doc");
                assert_eq!(path, "ProjectNotes");
                assert_eq!(name, "ProjectNotes");
                assert!(parent_path.is_none());
                assert!(target_id.as_str().starts_with("doc:"));
            }
            _ => panic!("Expected CreationIntent, got {:?}", target),
        }
    }

    #[test]
    fn test_classify_creation_intent_with_path() {
        let target = classify_link("Projects/New thing");
        match &target {
            LinkTarget::CreationIntent {
                scheme,
                path,
                name,
                parent_path,
                ..
            } => {
                assert_eq!(scheme, "doc");
                assert_eq!(path, "Projects/New thing");
                assert_eq!(name, "New thing");
                assert_eq!(parent_path.as_deref(), Some("Projects"));
            }
            _ => panic!("Expected CreationIntent, got {:?}", target),
        }
    }

    #[test]
    fn test_classify_person_scheme() {
        let target = classify_link("Person/Alice");
        match &target {
            LinkTarget::CreationIntent { scheme, name, .. } => {
                assert_eq!(scheme, "person");
                assert_eq!(name, "Alice");
            }
            _ => panic!("Expected CreationIntent, got {:?}", target),
        }
    }

    #[test]
    fn test_deterministic_id_stability() {
        let id1 = deterministic_entity_id("doc", "projects/new thing");
        let id2 = deterministic_entity_id("doc", "projects/new thing");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_deterministic_id_uuid_format() {
        let id = deterministic_entity_id("doc", "test");
        let path = id.id();
        // UUID format: 8-4-4-4-12
        assert_eq!(path.len(), 36);
        assert_eq!(path.chars().nth(8), Some('-'));
        assert_eq!(path.chars().nth(13), Some('-'));
        assert_eq!(path.chars().nth(18), Some('-'));
        assert_eq!(path.chars().nth(23), Some('-'));
    }

    #[test]
    fn test_case_insensitive_convergence() {
        let target1 = classify_link("Projects/Thing");
        let target2 = classify_link("projects/thing");

        let id1 = target1.entity_id().unwrap();
        let id2 = target2.entity_id().unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_normalize_for_hash_whitespace() {
        assert_eq!(normalize_for_hash("  Hello   World  "), "hello world");
        assert_eq!(normalize_for_hash("A\t\tB"), "a b");
    }

    #[test]
    fn test_same_target_same_id_across_links() {
        let content = "See [[Projects/Test]] and also [[Projects/Test]].";
        let links = extract_links(content);
        assert_eq!(links.len(), 2);

        let id1 = links[0].classified.entity_id().unwrap();
        let id2 = links[1].classified.entity_id().unwrap();
        assert_eq!(id1, id2);
    }
}
