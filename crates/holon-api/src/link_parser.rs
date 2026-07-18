//! Link parsing for org-mode content
//!
//! Extracts `[[target][text]]` and bare `[[target]]` style links from org-mode
//! content. Classifies each link target and computes deterministic entity IDs
//! for creation intents.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::entity_uri::EntityUri;

/// Classification of a link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Already resolved: `[[block:uuid]]`
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
    /// Returns the target EntityUri if this is a resolved or creation-intent
    /// link.
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
    /// Target URI or page name (e.g., "block:uuid", "Projects/New thing")
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
/// Lowercase, trim whitespace, collapse internal whitespace runs to single
/// space.
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
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:\
         02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    );
    EntityUri::new(scheme, &uuid_str)
}

/// Deterministic identity of a `Page`-tagged block, derived purely from its
/// slash-joined path (root→leaf).
///
/// This is the ONE sanctioned way to mint the id of a NEW page. Every page
/// write path — the lazy link-create op, org-file name-chain ingest, and the
/// link parser's own target computation — routes through
/// [`PageId::for_path`] so page identity is a pure function of the normalized
/// path, never a random UUID. Same normalized path ⇒ same id ⇒ two peers that
/// each create the same page converge on ONE CRDT merge key
/// (inv-page-name-unique). Pages are always `block`-scheme, so the scheme is
/// not a caller-chosen parameter here (that closed the `EntityName::named`
/// scheme-bypass hole).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageId(EntityUri);

impl PageId {
    /// Split a page path into per-segment trimmed pieces — the ONE
    /// canonicalization every page-id computation shares. Trimming each
    /// segment is what makes `"Areas / Sub"` (spaced separators) and
    /// `"Areas/Sub"` mint the SAME id, so the link parser's optimistic id can
    /// never drift from the id the write paths assign.
    fn segments(path: &str) -> Vec<&str> {
        path.split('/').map(str::trim).collect()
    }

    /// Hash already-trimmed segments into a `block:<hash>` id.
    fn from_segments(segments: &[&str]) -> Self {
        let canonical = segments.join("/");
        PageId(deterministic_entity_id("block", &normalize_for_hash(&canonical)))
    }

    /// Mint the deterministic id for a NEW page whose full path (root→leaf,
    /// `/`-joined) is `path`. Idempotent modulo case/whitespace.
    ///
    /// Fail-loud: an empty segment (a leading/trailing `/` or a `//`) is a
    /// malformed page path. We refuse it rather than silently collapse
    /// `"a//b"` into `"a/b"`, which would fuse two distinct intents (or bury a
    /// typo) under one id.
    pub fn for_path(path: &str) -> Result<Self, String> {
        let segments = Self::segments(path);
        if segments.iter().any(|s| s.is_empty()) {
            return Err(format!(
                "page path {path:?} has an empty segment (leading/trailing or doubled '/'); a \
                 page path must be non-empty '/'-separated segments"
            ));
        }
        Ok(Self::from_segments(&segments))
    }

    /// Borrow the underlying `EntityUri`.
    pub fn as_entity_uri(&self) -> &EntityUri {
        &self.0
    }

    /// The `block:<hash>` string form.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Consume into the underlying `EntityUri`.
    pub fn into_entity_uri(self) -> EntityUri {
        self.0
    }
}

/// Infer entity scheme from the first path segment.
fn infer_scheme(first_segment: &str) -> Option<&'static str> {
    match first_segment.to_lowercase().as_str() {
        "person" => Some("person"),
        _ => None, // default to "block" (pages are blocks tagged `Page`)
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

    // Already resolved: starts with the entity scheme followed by ':'
    if target.starts_with("block:") {
        // ALLOW(entity_uri_from_raw): raw org-file wiki-link target
        let uri = EntityUri::from_raw(target);
        return LinkTarget::Resolved(uri);
    }

    // Creation intent: wiki-style link like "Projects/New thing" or "PageName".
    // Segments are trimmed through the SAME canonicalization the write paths
    // use, so `[[Areas / Sub]]` and `[[Areas/Sub]]` agree on name/parent/id.
    let segments = PageId::segments(target);
    let name = segments.last().unwrap().to_string();
    let parent_path = if segments.len() > 1 {
        Some(segments[..segments.len() - 1].join("/"))
    } else {
        None
    };

    let scheme = segments
        .first()
        .and_then(|s| infer_scheme(s))
        .unwrap_or("block");

    // Route the page (block-scheme) case through the SAME `PageId`
    // canonicalization the write paths mint with, so a `[[Areas / Sub]]` link's
    // target id is *exactly* the id `create_page_from_link` / org name-chain
    // ingest will assign the page. Non-page schemes (e.g. `person:`) keep the
    // generic hash. An empty-segment (malformed) target can never be written —
    // `create_page_from_link` rejects it loudly — so its optimistic id here is
    // moot; we still derive it from the trimmed segments rather than fabricate
    // from the raw string.
    let target_id = if scheme == "block" {
        PageId::from_segments(&segments).into_entity_uri()
    } else {
        deterministic_entity_id(scheme, &normalize_for_hash(target))
    };

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
        let content = "This is a [[block:uuid-123][link to block]] in text.";
        let links = extract_links(content);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "block:uuid-123");
        assert_eq!(links[0].text, "link to block");
        assert!(
            matches!(&links[0].classified, LinkTarget::Resolved(uri) if uri.as_str() == "block:uuid-123")
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
            matches!(&links[0].classified, LinkTarget::CreationIntent { scheme, name, .. } if scheme == "block" && name == "ProjectNotes")
        );
    }

    #[test]
    fn test_extract_mixed_links() {
        let content = "A [[PageOne]] then [[block:2][described]] then [[PageThree]].";
        let links = extract_links(content);

        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, "PageOne");
        assert_eq!(links[0].text, "PageOne");
        assert_eq!(links[1].target, "block:2");
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
        let content = "First [[block:1][one]] and second [[block:2][two]].";
        let links = extract_links(content);

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "block:1");
        assert_eq!(links[1].target, "block:2");
    }

    #[test]
    fn test_extract_link_targets() {
        let content = "[[block:a][A]] and [[PageB]] and [[block:a][A again]].";
        let targets = extract_link_targets(content);

        assert_eq!(targets.len(), 2);
        assert!(targets.contains("block:a"));
        assert!(targets.contains("PageB"));
    }

    #[test]
    fn test_strip_links() {
        let content = "See [[block:123][this block]] and [[PageName]] for details.";
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

    /// The `doc:` scheme is retired (H7, 2026-07-02): a `doc:`-prefixed target
    /// is no longer accepted as Resolved — it falls through to the
    /// name-hashing creation-intent path like any other page name.
    #[test]
    fn test_doc_scheme_no_longer_resolved() {
        let target = classify_link("doc:existing-uuid");
        assert!(
            matches!(&target, LinkTarget::CreationIntent { scheme, .. } if scheme == "block"),
            "doc: must not classify as Resolved anymore, got {target:?}"
        );
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
                assert_eq!(scheme, "block");
                assert_eq!(path, "ProjectNotes");
                assert_eq!(name, "ProjectNotes");
                assert!(parent_path.is_none());
                assert!(target_id.as_str().starts_with("block:"));
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
                assert_eq!(scheme, "block");
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
        let id1 = deterministic_entity_id("block", "projects/new thing");
        let id2 = deterministic_entity_id("block", "projects/new thing");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_deterministic_id_uuid_format() {
        let id = deterministic_entity_id("block", "test");
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

    #[test]
    fn spaced_separators_converge_with_writer() {
        // H2: a link typed with spaces around '/' must classify to the SAME id
        // as the tight form AND as the id the write paths mint via
        // `PageId::for_path` — the parser trims each segment.
        let spaced = classify_link("Areas / Sub").entity_id().unwrap().clone();
        let tight = classify_link("Areas/Sub").entity_id().unwrap().clone();
        assert_eq!(spaced, tight, "spaced vs tight parser id must agree");

        let minted = PageId::for_path("Areas/Sub").unwrap().into_entity_uri();
        assert_eq!(spaced, minted, "parser id must equal writer-minted id");
        assert_eq!(
            PageId::for_path("Areas / Sub").unwrap().into_entity_uri(),
            minted,
            "for_path must be insensitive to separator spacing"
        );
        // name/parent are trimmed too.
        match classify_link("Areas / Sub") {
            LinkTarget::CreationIntent {
                name, parent_path, ..
            } => {
                assert_eq!(name, "Sub");
                assert_eq!(parent_path.as_deref(), Some("Areas"));
            }
            other => panic!("expected CreationIntent, got {other:?}"),
        }
    }

    #[test]
    fn for_path_rejects_empty_segments() {
        for bad in ["a//b", "/b", "a/", "  /  b", ""] {
            assert!(
                PageId::for_path(bad).is_err(),
                "malformed page path {bad:?} must be rejected (fail loud)"
            );
        }
        assert!(PageId::for_path("a/b").is_ok());
        assert!(PageId::for_path("a").is_ok());
    }
}
