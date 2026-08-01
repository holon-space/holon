//! Link parsing for org-mode content
//!
//! Extracts `[[target][text]]` and bare `[[target]]` style links from org-mode
//! content. Classifies each link target and computes deterministic entity IDs
//! for creation intents.

use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::entity_uri::EntityUri;

/// Classification of a link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Already resolved: `[[block:uuid]]`, `[[cc-session:abc]]` — a
    /// scheme-shaped target whose scheme is a registered entity.
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
    /// Scheme-shaped target that names no entity right now: `[[Areas:Work]]`,
    /// `[[cc-sesion:abc]]` (typo), a link left behind by an uninstalled
    /// integration, or a registered scheme carrying a path no URI can hold
    /// (`[[tag:a b]]`). The scheme SHAPE is reserved, so this is never a
    /// page-creation intent — it is disclosed as an unresolved entity link and
    /// flips to [`Resolved`](Self::Resolved) once its scheme is registered AND
    /// it parses.
    UnknownScheme(String),
}

impl LinkTarget {
    /// Returns the target EntityUri if this is a resolved or creation-intent
    /// link.
    pub fn entity_id(&self) -> Option<&EntityUri> {
        match self {
            LinkTarget::Resolved(uri) => Some(uri),
            LinkTarget::CreationIntent { target_id, .. } => Some(target_id),
            LinkTarget::External(_) | LinkTarget::UnknownScheme(_) => None,
        }
    }
}

/// Entity schemes every classifier resolves, with or without a registry.
///
/// These are the schemes the core owns; a registry only ever ADDS to them, so
/// a classifier built without one still classifies core links correctly and
/// unit tests / the reference model stay IO-free.
pub const BUILT_IN_LINK_SCHEMES: &[&str] = &["block", "tag", "person"];

/// The scheme half of a scheme-shaped `[[…]]` target — `cc-session` from
/// `[[cc-session:abc]]`.
///
/// Minted ONLY by [`link_scheme_shape`], so holding one is proof the string
/// already passed the RFC 3986 shape check. That turns "callers always pass a
/// shape-validated scheme" from a comment into a compile-time fact, and stops a
/// whole target (or a page title) being handed to a scheme lookup by mistake.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkScheme<'a>(&'a str);

impl<'a> LinkScheme<'a> {
    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

impl fmt::Display for LinkScheme<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// The set of entity schemes a [`LinkTargetClassifier`] resolves beyond the
/// built-ins.
///
/// Implemented by the schema/profile registry, which is the ONE source of
/// truth for which entities exist (built-ins plus every entity a YAML sidecar
/// declares). Queried live, so installing or removing an integration moves its
/// links between `Resolved` and `UnknownScheme` without a restart — and never
/// across the page/entity boundary.
pub trait LinkSchemeRegistry: Send + Sync {
    fn is_registered_entity_scheme(&self, scheme: &LinkScheme<'_>) -> bool;
}

/// A fixed scheme set — the registry a test or a fixture supplies when it has
/// no real profile registry.
pub struct FixedLinkSchemes(BTreeSet<String>);

impl FixedLinkSchemes {
    pub fn new(schemes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(schemes.into_iter().map(Into::into).collect())
    }
}

impl LinkSchemeRegistry for FixedLinkSchemes {
    fn is_registered_entity_scheme(&self, scheme: &LinkScheme<'_>) -> bool {
        self.0.contains(scheme.as_str())
    }
}

/// Classifies a raw `[[…]]` target into a [`LinkTarget`].
///
/// Carries the registered-scheme set explicitly: every parse boundary holds
/// one and passes it down. A [`Default`] classifier knows only
/// [`BUILT_IN_LINK_SCHEMES`], which is what keeps pure unit tests and the
/// reference model free of registry IO.
#[derive(Clone, Default)]
pub struct LinkTargetClassifier {
    registry: Option<Arc<dyn LinkSchemeRegistry>>,
}

impl fmt::Debug for LinkTargetClassifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkTargetClassifier")
            .field("built_ins", &BUILT_IN_LINK_SCHEMES)
            .field("registry_attached", &self.registry.is_some())
            .finish()
    }
}

impl LinkTargetClassifier {
    /// A classifier that resolves the built-ins plus everything `registry`
    /// declares.
    pub fn with_registry(registry: Arc<dyn LinkSchemeRegistry>) -> Self {
        Self {
            registry: Some(registry),
        }
    }

    /// A classifier that resolves the built-ins plus a fixed extra set.
    pub fn with_schemes(schemes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::with_registry(Arc::new(FixedLinkSchemes::new(schemes)))
    }

    pub fn is_registered_entity_scheme(&self, scheme: &LinkScheme<'_>) -> bool {
        BUILT_IN_LINK_SCHEMES.contains(&scheme.as_str())
            || self
                .registry
                .as_ref()
                .is_some_and(|r| r.is_registered_entity_scheme(scheme))
    }

    /// Does this scheme-shaped target name an entity that exists RIGHT NOW?
    ///
    /// The read-time counterpart to persisting a bare `EntityRef::Scheme`: the
    /// mark records only that the target is scheme-shaped, so every consumer
    /// needing the registration answer asks here, against the live registry.
    pub fn resolves_entity(&self, uri: &EntityUri) -> bool {
        link_scheme_shape(uri.as_str()).is_some_and(|s| self.is_registered_entity_scheme(&s))
    }

    /// Classify a raw link target string.
    pub fn classify(&self, target: &str) -> LinkTarget {
        // External URLs — checked first so a web scheme is never a candidate
        // entity scheme.
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
        {
            return LinkTarget::External(target.to_string());
        }

        // The scheme SHAPE is reserved for entity links (org-mode's own typed
        // links work this way). A shaped target is never a page name, whether
        // or not its scheme happens to be registered right now.
        if let Some(scheme) = link_scheme_shape(target) {
            // `EntityUri::parse`, never `from_raw`: `from_raw` guesses whether
            // a colon-leading path means "bare synthetic id" and RE-MINTS what
            // it rejects as `block:<whole target>`. That guess has no business
            // here — the shape check above already proved this is a scheme, so
            // the only question left is whether the target is a legal URI. The
            // guess turned `[[tag::x]]` into `[[block:tag::x][tag::x]]` and
            // panicked outright on `[[tag:a b]]`.
            //
            // This matches `EntityRef::entity_uri()`, which is the single
            // discriminator every consumer asks — so a target it calls an
            // entity is exactly the one classified `Resolved` here.
            return match EntityUri::parse(target) {
                Ok(uri) if self.is_registered_entity_scheme(&scheme) => LinkTarget::Resolved(uri),
                // Scheme-shaped but unregistered, or not a legal URI at all:
                // either way it names no entity right now and its bytes are
                // the author's. `Resolved` would have to invent a URI.
                _ => LinkTarget::UnknownScheme(target.to_string()),
            };
        }

        // Creation intent: wiki-style link like "Projects/New thing" or
        // "PageName". Segments are trimmed through the SAME canonicalization
        // the write paths use, so `[[Areas / Sub]]` and `[[Areas/Sub]]` agree
        // on name/parent/id.
        let segments = PageId::segments(target);

        // A scheme-shaped SEGMENT (`Areas/cc-session:abc`) is reserved just as
        // a scheme-shaped whole target is. The writer refuses to mint such a
        // page, so classifying it as a creation intent would hand out an id for
        // an intent that can never be fulfilled — the classifier applies the
        // writer's own per-segment rule instead.
        if segments.iter().any(|s| link_scheme_shape(s).is_some()) {
            return LinkTarget::UnknownScheme(target.to_string());
        }

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
        // canonicalization the write paths mint with, so a `[[Areas / Sub]]`
        // link's target id is *exactly* the id `create_page_from_link` /
        // org name-chain ingest will assign the page. Non-page schemes (e.g.
        // `person/Alice`) keep the generic hash. An empty-segment (malformed)
        // target can never be written — `create_page_from_link` rejects it
        // loudly — so its optimistic id here is moot; we still derive it from
        // the trimmed segments rather than fabricate from the raw string.
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
}

/// The RFC 3986 scheme of `target`, if it is scheme-shaped.
///
/// Shape is `letter (letter | digit | '+' | '-' | '.')* ':'` with no space
/// after the colon — the no-space rule is what keeps ordinary titles like
/// `Ketosis: How to lose weight` on the page side without capitalization
/// heuristics. Returns the scheme WITHOUT the colon.
pub fn link_scheme_shape(target: &str) -> Option<LinkScheme<'_>> {
    let colon = target.find(':')?;
    if target[colon + 1..].starts_with(' ') {
        return None;
    }
    let scheme = &target[..colon];
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    chars
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        .then_some(LinkScheme(scheme))
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

    /// Reject a page name that is RFC 3986 scheme-shaped.
    ///
    /// The scheme shape is reserved for entity links, so a page can never be
    /// minted under one — otherwise installing an integration would collide a
    /// live page with a live entity scheme, and the classifier's guarantee
    /// that a target never crosses the page/entity boundary would be a lie.
    fn reject_scheme_shaped(segment: &str) -> Result<(), String> {
        match link_scheme_shape(segment) {
            Some(scheme) => Err(format!(
                "page name {segment:?} is scheme-shaped ({:?} followed by ':'); that shape is \
                 reserved for entity links like [[cc-session:abc]] — use '/' for hierarchy, or \
                 put a space after the colon for a title",
                scheme.as_str()
            )),
            None => Ok(()),
        }
    }

    /// Hash already-trimmed segments into a `block:<hash>` id.
    fn from_segments(segments: &[&str]) -> Self {
        let canonical = segments.join("/");
        PageId(deterministic_entity_id(
            "block",
            &normalize_for_hash(&canonical),
        ))
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
        for segment in &segments {
            Self::reject_scheme_shaped(segment)?;
        }
        Ok(Self::from_segments(&segments))
    }

    /// Mint the id for a NEW page placed directly under `destination_path`
    /// (root→leaf, `/`-joined; empty ⇒ the vault root) whose leaf title is
    /// `leaf`.
    ///
    /// Unlike [`for_path`](Self::for_path), `leaf` is treated as ONE segment:
    /// a `/` inside it is part of the title, never a path separator. This is
    /// the sanctioned entry point for "turn block into page", where the leaf
    /// comes from the origin block's CONTENT — a title, not a path — so a
    /// title like `"buy milk/eggs"` (or a stray menu-trigger `/`) must not
    /// spawn phantom hierarchy nor trip the empty-segment guard. The
    /// destination path itself is still validated fail-loud (a malformed
    /// picker/MCP `destination_path` is a real error).
    pub fn for_page_under(destination_path: &str, leaf: &str) -> Result<Self, String> {
        let leaf = leaf.trim();
        if leaf.is_empty() {
            return Err("page leaf title is empty; a page needs a non-empty title".to_string());
        }
        let mut segments: Vec<&str> = if destination_path.trim().is_empty() {
            Vec::new()
        } else {
            Self::segments(destination_path)
        };
        if segments.iter().any(|s| s.is_empty()) {
            return Err(format!(
                "page destination path {destination_path:?} has an empty segment \
                 (leading/trailing or doubled '/'); a page path must be non-empty \
                 '/'-separated segments"
            ));
        }
        segments.push(leaf);
        for segment in &segments {
            Self::reject_scheme_shaped(segment)?;
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

/// Extract all `[[target][text]]` and `[[target]]` links from org-mode content.
///
/// For bare `[[target]]` links, `text` is set equal to `target`.
/// Links are returned in order of appearance.
pub fn extract_links(content: &str, classifier: &LinkTargetClassifier) -> Vec<Link> {
    let mut described_ranges: Vec<(usize, usize)> = Vec::new();
    let mut links = Vec::new();

    for mat in DESCRIBED_LINK_REGEX.find_iter(content) {
        let captures = DESCRIBED_LINK_REGEX.captures(mat.as_str()).unwrap();
        let target = captures[1].to_string();
        let text = captures[2].to_string();
        described_ranges.push((mat.start(), mat.end()));
        let classified = classifier.classify(&target);
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
        let classified = classifier.classify(&target);
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
pub fn extract_link_targets(content: &str, classifier: &LinkTargetClassifier) -> HashSet<String> {
    extract_links(content, classifier)
        .iter()
        .map(|link| link.target.clone())
        .collect()
}

/// Replace links in content with plain text (keeping the display text).
pub fn strip_links(content: &str, classifier: &LinkTargetClassifier) -> String {
    let links = extract_links(content, classifier);
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
        let links = extract_links(content, &LinkTargetClassifier::default());

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
        let links = extract_links(content, &LinkTargetClassifier::default());

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
        let links = extract_links(content, &LinkTargetClassifier::default());

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
        let links = extract_links(content, &LinkTargetClassifier::default());

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "target");
        assert_eq!(links[0].text, "text");
    }

    #[test]
    fn test_extract_multiple_described_links() {
        let content = "First [[block:1][one]] and second [[block:2][two]].";
        let links = extract_links(content, &LinkTargetClassifier::default());

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "block:1");
        assert_eq!(links[1].target, "block:2");
    }

    #[test]
    fn test_extract_link_targets() {
        let content = "[[block:a][A]] and [[PageB]] and [[block:a][A again]].";
        let targets = extract_link_targets(content, &LinkTargetClassifier::default());

        assert_eq!(targets.len(), 2);
        assert!(targets.contains("block:a"));
        assert!(targets.contains("PageB"));
    }

    #[test]
    fn test_strip_links() {
        let content = "See [[block:123][this block]] and [[PageName]] for details.";
        let stripped = strip_links(content, &LinkTargetClassifier::default());

        assert_eq!(stripped, "See this block and PageName for details.");
    }

    #[test]
    fn test_no_links() {
        let content = "Plain text without any links.";
        let links = extract_links(content, &LinkTargetClassifier::default());
        assert!(links.is_empty());
    }

    #[test]
    fn test_positions_are_correct() {
        let content = "A [[Page]] B";
        let links = extract_links(content, &LinkTargetClassifier::default());

        assert_eq!(links.len(), 1);
        assert_eq!(&content[links[0].start..links[0].end], "[[Page]]");
    }

    // --- New tests for classification + deterministic IDs ---

    /// The `doc:` scheme is retired (H7, 2026-07-02) and unregistered, so a
    /// `doc:` target resolves to nothing. It is still scheme-SHAPED, so it is
    /// an unknown-scheme link rather than a page named `doc:existing-uuid`.
    #[test]
    fn test_doc_scheme_no_longer_resolved() {
        let target = LinkTargetClassifier::default().classify("doc:existing-uuid");
        assert_eq!(
            target,
            LinkTarget::UnknownScheme("doc:existing-uuid".into())
        );
    }

    // --- F1a: the three-state classifier ---

    #[test]
    fn scheme_shape_follows_rfc_3986() {
        let shape = |t: &'static str| link_scheme_shape(t).map(|s| s.as_str());
        assert_eq!(shape("cc-session:abc"), Some("cc-session"));
        assert_eq!(shape("Areas:Work"), Some("Areas"));
        assert_eq!(shape("a+b.c-d:x"), Some("a+b.c-d"));
        // No colon, colon-space, leading non-letter, and an illegal scheme char
        // are all NOT scheme-shaped.
        assert_eq!(shape("Areas/Work"), None);
        assert_eq!(shape("Ketosis: How to lose weight"), None);
        assert_eq!(shape("1up:x"), None);
        assert_eq!(shape("two words:x"), None);
    }

    #[test]
    fn registered_scheme_resolves_to_the_full_uri() {
        let classifier = LinkTargetClassifier::with_schemes(["cc-session"]);
        assert!(
            matches!(classifier.classify("cc-session:abc"), LinkTarget::Resolved(uri) if uri.as_str() == "cc-session:abc")
        );
        assert!(
            matches!(classifier.classify("tag:rust"), LinkTarget::Resolved(uri) if uri.as_str() == "tag:rust")
        );
    }

    /// The registered set only ever moves a target between `Resolved` and
    /// `UnknownScheme`. It can never move one across the page/entity boundary,
    /// which is what makes installing or removing an integration safe.
    #[test]
    fn unregistering_a_scheme_never_turns_its_links_into_pages() {
        let target = "cc-session:abc";
        assert!(matches!(
            LinkTargetClassifier::with_schemes(["cc-session"]).classify(target),
            LinkTarget::Resolved(_)
        ));
        assert_eq!(
            LinkTargetClassifier::default().classify(target),
            LinkTarget::UnknownScheme(target.into())
        );
    }

    #[test]
    fn unregistered_scheme_is_never_a_creation_intent() {
        for target in ["Areas:Work", "cc-sesion:abc", "ftp:example.com"] {
            assert_eq!(
                LinkTargetClassifier::default().classify(target),
                LinkTarget::UnknownScheme(target.into()),
                "{target} must reserve the scheme shape"
            );
        }
    }

    #[test]
    fn colon_space_title_stays_a_page() {
        let target = LinkTargetClassifier::default().classify("Ketosis: How to lose weight");
        assert!(
            matches!(&target, LinkTarget::CreationIntent { name, .. } if name == "Ketosis: How to lose weight"),
            "got {target:?}"
        );
    }

    #[test]
    fn page_creation_rejects_a_scheme_shaped_name() {
        let err = PageId::for_path("cc-session:abc").expect_err("must be rejected");
        assert!(err.contains("scheme-shaped"), "{err}");
        assert!(err.contains("use '/' for hierarchy"), "{err}");

        let err = PageId::for_page_under("Areas", "cc-session:abc").expect_err("must be rejected");
        assert!(err.contains("scheme-shaped"), "{err}");

        // A nested segment is guarded too, and colon-space titles still pass.
        assert!(PageId::for_path("Areas/doc:x/Leaf").is_err());
        PageId::for_path("Areas/Ketosis: How to lose weight").expect("colon-space title is a page");
    }

    /// Over the SCHEME-SHAPE rule the classifier and the writer agree exactly:
    /// nothing the classifier calls a creation intent is refused by
    /// `PageId::for_path` for its shape, and everything the writer refuses for
    /// its shape classifies as a disclosed unknown scheme.
    ///
    /// Scoped deliberately — the empty-segment rule is NOT yet aligned; see
    /// [`empty_segment_targets_still_diverge_from_the_writer`].
    #[test]
    fn creation_intents_match_the_writer_on_scheme_shape() {
        let classifier = LinkTargetClassifier::default();
        for target in [
            "cc-session:abc",
            "Areas/cc-session:abc",
            "Areas/doc:x/Leaf",
            "Projects/New thing",
            "Ketosis: How to lose weight",
            "Areas/Ketosis: How to lose weight",
        ] {
            match classifier.classify(target) {
                LinkTarget::CreationIntent { .. } => {
                    PageId::for_path(target).unwrap_or_else(|e| {
                        panic!(
                            "{target:?} classified as a creation intent but the writer refuses \
                             it ({e}) — an unfulfillable intent"
                        )
                    });
                }
                LinkTarget::UnknownScheme(_) => {
                    assert!(
                        PageId::for_path(target).is_err(),
                        "{target:?} classified as unknown-scheme but the writer would accept it \
                         as a page — the two rules disagree"
                    );
                }
                other => panic!("{target:?} unexpectedly classified as {other:?}"),
            }
        }
    }

    /// KNOWN DIVERGENCE, pinned so it cannot drift unnoticed.
    ///
    /// `PageId::for_path` refuses an empty/whitespace segment as a malformed
    /// path, but the classifier still calls these creation intents and mints an
    /// optimistic id for them — an intent the writer can never fulfil, exactly
    /// the class of bug the scheme-shape rule above was tightened to remove.
    ///
    /// Not fixed here because `LinkTarget` has no honest bucket for it:
    /// `UnknownScheme` means "scheme-shaped, scheme unclaimed" and these have
    /// no scheme at all, so a faithful fix needs a new variant that ripples
    /// through `EntityRef`, its serde tag, the Loro codec, the org renderer
    /// and the GPUI link styling. TODO: add that variant and fold these
    /// cases into the test above.
    ///
    /// The failure is at least DISCLOSED rather than silent: clicking such a
    /// link routes to `follow_dangling_link`, which calls the writer and fails
    /// loudly.
    #[test]
    fn empty_segment_targets_still_diverge_from_the_writer() {
        let classifier = LinkTargetClassifier::default();
        for target in ["a//b", " /b", "Areas/"] {
            assert!(
                PageId::for_path(target).is_err(),
                "{target:?} must be refused by the writer as a malformed path"
            );
            assert!(
                matches!(
                    classifier.classify(target),
                    LinkTarget::CreationIntent { .. }
                ),
                "{target:?} is expected to STILL classify as a creation intent — if this now                  fails the divergence was fixed: fold these cases into \
                 `creation_intents_match_the_writer_on_scheme_shape` and delete this test"
            );
        }
    }

    #[test]
    fn test_classify_resolved_block() {
        let target = LinkTargetClassifier::default().classify("block:some-id");
        assert!(matches!(target, LinkTarget::Resolved(uri) if uri.as_str() == "block:some-id"));
    }

    #[test]
    fn test_classify_external_https() {
        let target = LinkTargetClassifier::default().classify("https://example.com");
        assert!(matches!(target, LinkTarget::External(url) if url == "https://example.com"));
    }

    #[test]
    fn test_classify_external_mailto() {
        let target = LinkTargetClassifier::default().classify("mailto:test@example.com");
        assert!(matches!(target, LinkTarget::External(url) if url == "mailto:test@example.com"));
    }

    #[test]
    fn test_classify_creation_intent_simple() {
        let target = LinkTargetClassifier::default().classify("ProjectNotes");
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
        let target = LinkTargetClassifier::default().classify("Projects/New thing");
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
        let target = LinkTargetClassifier::default().classify("Person/Alice");
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
        let target1 = LinkTargetClassifier::default().classify("Projects/Thing");
        let target2 = LinkTargetClassifier::default().classify("projects/thing");

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
        let links = extract_links(content, &LinkTargetClassifier::default());
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
        let spaced = LinkTargetClassifier::default()
            .classify("Areas / Sub")
            .entity_id()
            .unwrap()
            .clone();
        let tight = LinkTargetClassifier::default()
            .classify("Areas/Sub")
            .entity_id()
            .unwrap()
            .clone();
        assert_eq!(spaced, tight, "spaced vs tight parser id must agree");

        let minted = PageId::for_path("Areas/Sub").unwrap().into_entity_uri();
        assert_eq!(spaced, minted, "parser id must equal writer-minted id");
        assert_eq!(
            PageId::for_path("Areas / Sub").unwrap().into_entity_uri(),
            minted,
            "for_path must be insensitive to separator spacing"
        );
        // name/parent are trimmed too.
        match LinkTargetClassifier::default().classify("Areas / Sub") {
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

    #[test]
    fn for_page_under_treats_leaf_as_a_single_segment() {
        // Bug (C): the "turn block into page" leaf comes from block CONTENT (a
        // title), so a `/` inside it is part of the title, never a separator —
        // it must NOT trip the empty-segment guard nor mint phantom hierarchy.
        assert!(
            PageId::for_page_under("", "Promote me to page/").is_ok(),
            "a trailing '/' in the title is one leaf segment, not a malformed path"
        );
        assert!(
            PageId::for_page_under("Home", "buy milk/eggs").is_ok(),
            "an embedded '/' in the title stays one leaf segment"
        );
        // Equivalence to for_path for slash-free content keeps ids born-equal
        // with the existing write paths.
        assert_eq!(
            PageId::for_page_under("Home", "Section").unwrap(),
            PageId::for_path("Home/Section").unwrap(),
        );
        assert_eq!(
            PageId::for_page_under("", "Loose note").unwrap(),
            PageId::for_path("Loose note").unwrap(),
        );
        // The DESTINATION path is still a real path — malformed destinations and
        // empty leaves fail loud.
        assert!(PageId::for_page_under("a//b", "leaf").is_err());
        assert!(PageId::for_page_under("Home", "   ").is_err());
    }
}
