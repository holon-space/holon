use holon_api::BatchMetadata;
use holon_api::Change;
use holon_api::EntityUri;
use holon_api::WithMetadata;
use holon_macros::Entity;
use serde::Deserialize;
use serde::Serialize;

/// Changes wrapped with metadata for atomic sync token updates
pub type ChangesWithMetadata<T> = WithMetadata<Vec<Change<T>>, BatchMetadata>;

/// File - represents a file in the filesystem that maps to a logical Document
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "file", short_name = "file", graph_label = "file")]
pub struct File {
    #[primary_key]
    #[indexed]
    pub id: String,

    /// Filename (e.g. "index.org")
    pub name: String,

    /// Relative path of the containing folder, from the vault root — e.g.
    /// `Projects/DBG`, or `.` for a file sitting directly in the root.
    ///
    /// This is a plain path string, NOT an entity id and NOT a foreign key:
    /// nothing joins on it. Keep it a `String` — the deleted `Directory` entity
    /// wrapped the same relative path in an `EntityUri`, and because
    /// `EntityUri::from_raw` maps an unschemed string to `block:<s>`, a folder
    /// named `Agentic DPL` became `block:Agentic DPL` and panicked at boot (a
    /// space is not a legal RFC 3986 URI character). A path is not an entity
    /// id; giving this field a URI type would reintroduce that bug.
    #[indexed]
    pub parent_id: String,

    /// SHA256 for change detection
    pub content_hash: String,

    /// The document this file was recorded against, as
    /// [`File::document_id_text`] writes it; `None` until one is recorded.
    #[indexed]
    pub document_id: Option<String>,

    /// Overflow bag for properties with no column of their own; the engine's
    /// `_provenance` stamp lands here.
    #[jsonb]
    #[value_kind(overflow_properties)]
    pub properties: Option<String>,

    /// Per-key kind map for `properties`, holding an entry only where the JSON
    /// form is ambiguous. NULL means every key reads back at its JSON-evident
    /// kind.
    #[value_kind(overflow_property_kinds)]
    pub property_kinds: Option<String>,
}

impl File {
    pub fn new(
        id: String,
        name: String,
        parent_id: String,
        content_hash: String,
        document: Option<&EntityUri>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            id,
            name,
            parent_id,
            content_hash,
            document_id: document.map(Self::document_id_text).transpose()?,
            properties: None,
            property_kinds: None,
        })
    }

    /// `document` as the `document_id` column holds it: a `block:` id whose
    /// bare text is a plain block id is stored bare, any other id as its URI.
    pub fn document_id_text(document: &EntityUri) -> anyhow::Result<String> {
        let text = match document.as_str().strip_prefix("block:") {
            Some(bare) if is_bare_block_id(bare) => bare.to_string(),
            _ => document.as_str().to_string(),
        };
        let read_back = Self::parse_document_id(&text)?;
        anyhow::ensure!(
            &read_back == document,
            "document {document} cannot be recorded on a file row: its text {text:?} reads back \
             as {read_back}"
        );
        Ok(text)
    }

    /// The document a `document_id` column text names.
    pub fn parse_document_id(text: &str) -> anyhow::Result<EntityUri> {
        if is_bare_block_id(text) {
            return Ok(EntityUri::block(text));
        }
        EntityUri::schemed(text)
            .filter(|uri| !uri.id().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "file document id {text:?} names no document: it is neither a bare block id \
                     nor a URI with an id"
                )
            })
    }
}

/// A block id without its scheme, in the shape an org id line carries it: it
/// names no scheme, does not start with `:`, and `block:<id>` has that id.
fn is_bare_block_id(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with(':')
        && EntityUri::schemed(text).is_none()
        && EntityUri::parse(&format!("block:{text}")).is_ok_and(|uri| uri.id() == text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_reads_back_as_itself() {
        for raw in [
            "block:notes-doc",
            "block:a#b",
            "block:a?b",
            "block::split-1",
            "block:a:b",
            "block:123:45",
            "file:Notes/Sub.org",
            "file:a%20b.cook",
        ] {
            let document = EntityUri::parse(raw).unwrap();
            let text = File::document_id_text(&document).unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(
                File::parse_document_id(&text).unwrap(),
                document,
                "{raw} as {text:?}"
            );
        }
        let bare = File::document_id_text(&EntityUri::block("notes-doc")).unwrap();
        assert_eq!(bare, "notes-doc");
    }

    #[test]
    fn text_that_names_no_document_is_refused_by_name() {
        for text in ["", ":x", "a b", "café", "a%zz", "a#b", "block:", "x:"] {
            let err = File::parse_document_id(text).expect_err(text);
            assert!(err.to_string().contains(&format!("{text:?}")), "{err}");
        }
    }
}
