//! Closed intent vocabulary for block field writes ("parse, don't validate").
//!
//! Model.md invariant 3: **intent never carries an order key**. Fractional
//! order keys (`sort_key`) are minted exclusively by the ordering authority
//! (`OrderKeyMinting` / `BlockOrdering::place`); a frontend or MCP client
//! expresses a move positionally (`move_block { id, parent_id,
//! after_block_id }`), never by shipping a key value.
//!
//! [`BlockWriteField`] is the parsed form of the `field` parameter of a
//! `set_field` *intent* (frontend dispatch, MCP `execute_operation`). It is a
//! closed enum with **no order-key variant**: after parsing, an order key is
//! unrepresentable, not merely discarded. Parsing happens once at the intent
//! boundary (`OperationDispatcher::execute_operation`,
//! `LoroBlockOperations::execute_operation`); a disallowed field is a loud
//! `Err`, never a silent drop.
//!
//! Scope: this vocabulary types **intent**, not the internal storage seam.
//! The ordering authority and the outbound Loro→SQL projector write
//! storage-internal fields (`sort_key`, `depth`, `_expected_*` watermarks)
//! through their own sanctioned paths (`CrudOperations::set_field` /
//! `SqlOperationProvider` direct calls), which deliberately do not pass
//! through this parse.

use std::fmt;

/// A block field that a `set_field` **intent** is allowed to write.
///
/// Deliberately excludes:
/// - `sort_key` / `after_block_id` — order keys; minted by the ordering
///   authority only (Model.md invariant 3).
/// - `id`, `depth`, `created_at`, `updated_at`, `_change_origin`,
///   `_expected_*` — storage bookkeeping / derived fields; written by the
///   storage layer itself, never by intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockWriteField {
    Content,
    ContentType,
    SourceLanguage,
    SourceName,
    Marks,
    Collapsed,
    Completed,
    BlockType,
    Properties,
    Tags,
    TaskState,
    /// Routed to the structural authority (a tree move), not a raw column
    /// write — see `BlockCellRegistry::write_field("parent_id")`.
    ParentId,
    /// Any other user-defined property (e.g. `DEADLINE`, `PRIORITY`,
    /// `status`). The key is validated at construction: reserved and
    /// order-key names cannot be smuggled in through this variant.
    Property(PropertyKey),
}

/// A validated user-property key. The inner string is private: the only way
/// to obtain one is [`BlockWriteField::parse`], which rejects order keys and
/// storage-internal names — so holding a `PropertyKey` is proof the name is
/// a legal intent-writable property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyKey(String);

impl PropertyKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a raw field name was rejected at the intent boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockWriteFieldError {
    /// The field is an order key. Order is owned by the ordering authority;
    /// intent must express a move positionally (`move_block` with an
    /// `after_block_id` anchor), never carry a key value.
    OrderKey(String),
    /// The field is storage bookkeeping or derived state, written by the
    /// storage layer itself, never by intent.
    StorageInternal(String),
    /// Empty field name.
    Empty,
}

impl fmt::Display for BlockWriteFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockWriteFieldError::OrderKey(field) => write!(
                f,
                "set_field(\"{field}\") rejected: intent must never carry an order key \
                 (Model.md invariant 3). Order keys are minted by the ordering authority; \
                 express the move positionally via move_block {{ id, parent_id, after_block_id }}."
            ),
            BlockWriteFieldError::StorageInternal(field) => write!(
                f,
                "set_field(\"{field}\") rejected: '{field}' is storage bookkeeping / derived \
                 state, written by the storage layer itself, never by intent."
            ),
            BlockWriteFieldError::Empty => {
                write!(f, "set_field(\"\") rejected: empty field name")
            }
        }
    }
}

impl std::error::Error for BlockWriteFieldError {}

impl BlockWriteField {
    /// Parse a raw `set_field` field name into the closed intent vocabulary.
    ///
    /// Returns `Err` (fail-loud) for order keys and storage-internal fields;
    /// unknown names parse as [`BlockWriteField::Property`].
    pub fn parse(raw: &str) -> Result<Self, BlockWriteFieldError> {
        if raw.is_empty() {
            return Err(BlockWriteFieldError::Empty);
        }
        if raw.starts_with("_expected_") {
            return Err(BlockWriteFieldError::StorageInternal(raw.to_string()));
        }
        match raw {
            "sort_key" | "after_block_id" => Err(BlockWriteFieldError::OrderKey(raw.to_string())),
            "id" | "depth" | "created_at" | "updated_at" | "_change_origin" => {
                Err(BlockWriteFieldError::StorageInternal(raw.to_string()))
            }
            "content" => Ok(Self::Content),
            "content_type" => Ok(Self::ContentType),
            "source_language" => Ok(Self::SourceLanguage),
            "source_name" => Ok(Self::SourceName),
            "marks" => Ok(Self::Marks),
            "collapsed" => Ok(Self::Collapsed),
            "completed" => Ok(Self::Completed),
            "block_type" => Ok(Self::BlockType),
            "properties" => Ok(Self::Properties),
            "tags" => Ok(Self::Tags),
            "task_state" => Ok(Self::TaskState),
            "parent_id" => Ok(Self::ParentId),
            other => Ok(Self::Property(PropertyKey(other.to_string()))),
        }
    }

    /// The raw field name this variant writes.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Content => "content",
            Self::ContentType => "content_type",
            Self::SourceLanguage => "source_language",
            Self::SourceName => "source_name",
            Self::Marks => "marks",
            Self::Collapsed => "collapsed",
            Self::Completed => "completed",
            Self::BlockType => "block_type",
            Self::Properties => "properties",
            Self::Tags => "tags",
            Self::TaskState => "task_state",
            Self::ParentId => "parent_id",
            Self::Property(key) => key.as_str(),
        }
    }
}

impl fmt::Display for BlockWriteField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_keys_are_unrepresentable() {
        assert_eq!(
            BlockWriteField::parse("sort_key"),
            Err(BlockWriteFieldError::OrderKey("sort_key".to_string()))
        );
        assert_eq!(
            BlockWriteField::parse("after_block_id"),
            Err(BlockWriteFieldError::OrderKey("after_block_id".to_string()))
        );
    }

    #[test]
    fn storage_internal_fields_are_rejected() {
        for field in ["id", "depth", "created_at", "updated_at", "_change_origin"] {
            assert_eq!(
                BlockWriteField::parse(field),
                Err(BlockWriteFieldError::StorageInternal(field.to_string())),
                "{field} must be rejected"
            );
        }
        assert_eq!(
            BlockWriteField::parse("_expected_content"),
            Err(BlockWriteFieldError::StorageInternal(
                "_expected_content".to_string()
            ))
        );
    }

    #[test]
    fn empty_field_is_rejected() {
        assert_eq!(BlockWriteField::parse(""), Err(BlockWriteFieldError::Empty));
    }

    #[test]
    fn known_fields_parse_to_their_variant() {
        assert_eq!(
            BlockWriteField::parse("content"),
            Ok(BlockWriteField::Content)
        );
        assert_eq!(BlockWriteField::parse("tags"), Ok(BlockWriteField::Tags));
        assert_eq!(
            BlockWriteField::parse("task_state"),
            Ok(BlockWriteField::TaskState)
        );
        assert_eq!(
            BlockWriteField::parse("parent_id"),
            Ok(BlockWriteField::ParentId)
        );
    }

    #[test]
    fn unknown_names_parse_as_validated_properties() {
        match BlockWriteField::parse("DEADLINE") {
            Ok(BlockWriteField::Property(key)) => assert_eq!(key.as_str(), "DEADLINE"),
            other => panic!("expected Property, got {other:?}"),
        }
    }

    #[test]
    fn as_str_round_trips() {
        for field in ["content", "marks", "task_state", "DEADLINE", "parent_id"] {
            assert_eq!(BlockWriteField::parse(field).unwrap().as_str(), field);
        }
    }
}
