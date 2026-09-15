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
//! closed enum with **no order-key variant** and **no whole-bag variant**:
//! after parsing, neither is representable, not merely discarded. Parsing
//! happens once at the intent boundary
//! (`OperationDispatcher::execute_operation`,
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

use holon_pattern::schema::FieldIntent;

/// A block field that a `set_field` **intent** is allowed to write.
///
/// The named variants are the `FieldIntent::Writable` fields of
/// `holon_pattern::schema::BLOCK`; the lock that keeps the two from drifting is
/// `intent_writable_fields_are_all_arc_places` in
/// `holon-api/tests/descriptor_arcs_roundtrip.rs`.
///
/// Deliberately excludes:
/// - `sort_key` / `after_block_id` — order keys; minted by the ordering
///   authority only (Model.md invariant 3).
/// - `id`, `depth`, `created_at`, `updated_at`, `_change_origin`, `_expected_*`
///   — storage bookkeeping / derived fields; written by the storage layer
///   itself, never by intent.
/// - `properties` — the engine-owned overflow bag; an intent writes its KEYS
///   one at a time, never the bag itself ([`BlockWriteFieldError::WholeBag`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockWriteField {
    Content,
    ContentType,
    SourceLanguage,
    SourceName,
    Marks,
    Collapsed,
    WidgetOnly,
    Completed,
    BlockType,
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
    /// The `field` named the engine-owned overflow bag itself, not one of its
    /// keys.
    WholeBag(String),
    /// Empty field name.
    Empty,
}

impl fmt::Display for BlockWriteFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockWriteFieldError::OrderKey(field) => write!(
                f,
                "set_field(\"{field}\") rejected: intent must never carry an order key (Model.md \
                 invariant 3). Order keys are minted by the ordering authority; express the move \
                 positionally via move_block {{ id, parent_id, after_block_id }}."
            ),
            BlockWriteFieldError::StorageInternal(field) => write!(
                f,
                "set_field(\"{field}\") rejected: '{field}' is storage bookkeeping / derived \
                 state, written by the storage layer itself, never by intent."
            ),
            BlockWriteFieldError::WholeBag(field) => write!(
                f,
                "set_field(\"{field}\") rejected: the 'field' parameter names '{field}', the \
                 engine-owned property BAG. A whole-bag write hands its values over as ONE \
                 serialized string, so no per-property kind travels with it: the write replaces \
                 the bag and leaves its kinds describing values it no longer holds, and a \
                 DateTime or Json property written this way reads back as a String / Object. \
                 Write the properties ONE at a time instead — set_field {{ id, field: \
                 \"<property key>\", value: <the value> }} records that property's kind in the \
                 same statement; write one such op per property."
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
        // The refusals are read off the ONE schema declaration
        // (`holon_pattern::schema::BLOCK`), so a field's intent classification
        // lives beside its storage and cannot drift from it.
        match holon_pattern::schema::BLOCK.field(raw).map(|f| f.intent) {
            Some(FieldIntent::OrderKey) => {
                return Err(BlockWriteFieldError::OrderKey(raw.to_string()));
            }
            Some(FieldIntent::StorageInternal) => {
                return Err(BlockWriteFieldError::StorageInternal(raw.to_string()));
            }
            Some(FieldIntent::EngineOwnedBag) => {
                return Err(BlockWriteFieldError::WholeBag(raw.to_string()));
            }
            _ => {}
        }
        match raw {
            "content" => Ok(Self::Content),
            "content_type" => Ok(Self::ContentType),
            "source_language" => Ok(Self::SourceLanguage),
            "source_name" => Ok(Self::SourceName),
            "marks" => Ok(Self::Marks),
            "collapsed" => Ok(Self::Collapsed),
            "widget_only" => Ok(Self::WidgetOnly),
            "completed" => Ok(Self::Completed),
            "block_type" => Ok(Self::BlockType),
            "tags" => Ok(Self::Tags),
            "task_state" => Ok(Self::TaskState),
            "parent_id" => Ok(Self::ParentId),
            // An operation-control key that slipped past the specific arms above
            // (`_order_rekeys`, `_routing_*`) is an INSTRUCTION to the writer,
            // never a property. Accepted as a `Property`, a
            // `set_field(field="_order_rekeys", value=<object>)` would smuggle
            // the control key into the `properties` column in VALUE position,
            // where the key-position filter cannot see it and the Loro→SQL
            // projection later re-injects it as a live params key. Refuse it
            // here, at the same boundary that already refuses `sort_key`.
            other if crate::entity::is_operation_control_param(other) => {
                Err(BlockWriteFieldError::StorageInternal(other.to_string()))
            }
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
            Self::WidgetOnly => "widget_only",
            Self::Completed => "completed",
            Self::BlockType => "block_type",
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

    /// The refusal must be actionable: it names the parameter, the offending
    /// name, and the per-property route that replaces it (ruling D126.a).
    #[test]
    fn the_whole_properties_bag_is_unrepresentable() {
        let field = holon_pattern::schema::BLOCK
            .field("properties")
            .expect("the block schema declares the bag")
            .name;
        let err = BlockWriteField::parse(field)
            .expect_err("a whole-bag set_field carries no per-property kind and must be refused");
        assert_eq!(err, BlockWriteFieldError::WholeBag(field.to_string()));

        let msg = err.to_string();
        assert!(
            msg.contains("set_field(\"properties\")"),
            "the refusal must name the operation and the offending name as the caller spelled \
             them, got: {msg}"
        );
        assert!(
            msg.contains("'field' parameter"),
            "the refusal must name the offending PARAMETER, got: {msg}"
        );
        assert!(
            msg.contains("property key") && msg.contains("one"),
            "the refusal must name the per-PROPERTY route a caller switches to, got: {msg}"
        );
    }

    /// The order-rekey control key must not be writable as a property in VALUE
    /// position. `set_field(field="_order_rekeys", value=<object>)` would
    /// otherwise land the control key inside `properties` where the SQL
    /// provider's key-position filter cannot strip it, and the Loro→SQL
    /// projection would re-inject it as a live params key on the next flush.
    #[test]
    fn operation_control_keys_are_rejected() {
        // Reference the consts, not the literals — the routing/expected prefixes
        // are archlint-guarded string forms.
        for field in [
            crate::entity::ORDER_REKEYS_PARAM,
            crate::entity::ROUTING_DOC_URI_KEY,
            crate::entity::POSITION_AFTER_BLOCK_ID_PARAM,
        ] {
            assert!(
                BlockWriteField::parse(field).is_err(),
                "{field} is operation-control metadata and must be refused at the intent boundary"
            );
        }
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
