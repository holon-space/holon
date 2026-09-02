//! @c4 component
//! @c4 layer Core
//! Pattern: Shared Kernel
//! @c4 uses holon-expr "compiled Rhai expressions" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Shared value types, Operation descriptors, Change/CDC types, and entity
//! conversion traits. No frontend deps.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

pub mod action_dsl;
pub mod auth;
pub mod block;
pub mod block_mutation;
pub mod block_write_field;
pub mod capability;
pub mod change_set;
pub mod clock;
/// flutter_rust_bridge:ignore
pub mod computation;
pub mod computed;
pub mod content_canonical;
pub mod edge_field;
pub mod effect_id;
pub mod entity;
pub mod entity_profile;
pub mod entity_uri;
pub mod expr_parser;
pub mod filter;
mod hashmap_value_conversions;
pub mod history;
pub mod icon_name;
pub mod identity_minting;
pub mod identity_recognition;
pub mod inline_mark;
pub mod input_types;
/// flutter_rust_bridge:ignore
pub mod interp_value;
pub mod latency_e2e;
pub mod latency_slo;
pub mod link_candidate;
pub mod link_parser;
pub mod live_data;
pub mod operation_engine;
pub mod perspective;
pub mod predicate;
pub mod proposal;
pub mod provenance;
pub mod query_context;
pub mod query_engine;
pub mod reactive;
/// flutter_rust_bridge:ignore
pub mod render_dsl;
pub mod render_eval;
pub mod render_requirements;
pub mod render_types;
pub mod repository;
pub mod share_props;
pub mod spawner;
pub mod storage_error;
pub mod streaming;
pub mod template;
pub mod template_instantiation;
pub mod types;
pub mod ui_watcher;
pub mod vault_shape;
pub mod widget_meta;
pub mod widget_spec;
pub mod write_seq;

pub use entity_profile::EntityProfile;
pub use entity_profile::ProfileCache;
pub use entity_profile::ProfileResolving;
pub use entity_profile::VirtualChildConfig;
pub use history::HistoryEvent;
pub use history::HistoryFidelity;
pub use history::HistoryQuery;
pub use history::HistoryQueryArgs;
pub use history::HistoryStore;
pub use identity_minting::CarriedId;
pub use identity_minting::CreateId;
pub use identity_minting::IdentityInput;
pub use identity_minting::IdentityMinting;
pub use identity_minting::MintedId;
pub use identity_minting::ResolvedAddress;
pub use identity_recognition::Recognition;
pub use identity_recognition::recognize_derived_id;
pub use identity_recognition::sanitize_page_title;
pub use operation_engine::Delivery;
pub use operation_engine::OpOrigin;
pub use operation_engine::OpOutcome;
pub use operation_engine::OperationEngine;
pub use operation_engine::UndoOutcome;
pub use operation_engine::undo_step_dropped_detail;
pub use proposal::ACCEPT_PROPOSAL_OP;
pub use proposal::PROPOSAL_PROPERTY;
pub use proposal::PROPOSALS_ROOT_ID;
pub use proposal::PROPOSED_BY_PROPERTY;
pub use proposal::ProposalRecord;
pub use proposal::ProposalStatus;
pub use proposal::REJECT_PROPOSAL_OP;
pub use proposal::is_proposal_block;
pub use proposal::is_proposals_place;
pub use provenance::ENGINE_OWNED_PARAM_KEYS;
pub use provenance::PROVENANCE_PROPERTY;
pub use provenance::ProvenanceStamp;
pub use query_engine::OpenTab;
pub use query_engine::QueryEngine;
pub use query_engine::RegionTabs;
pub use storage_error::IDENTITY_COLLISION_MARKER;
pub use storage_error::IdentityCollision;
pub use storage_error::ParentNotFound;
pub use storage_error::ProjectionInvariantViolated;
pub use template::INSTANCE_OF_PROPERTY;
pub use template::INSTANTIATE_TEMPLATE_OP;
pub use template::TEMPLATE_MARKER_PROPERTY;
pub use template::TEMPLATE_VARS_PROPERTY;
pub use ui_watcher::UiWatcher;

/// The `set_field` FIELD whose value is a block's full vault source rather than
/// one column: the engine parses it and writes `content` and `task_state`
/// together, clearing the task state when the source carries no keyword.
///
/// It lives here, not in the engine, because it is a CONTRACT between the
/// editable surface (which shows a source projection, `TODO milk`) and the
/// store (which re-derives both columns from it). `set_field("content")`
/// deliberately means something else: one column, and never a task-state
/// change.
pub const SOURCE_TEXT_FIELD: &str = "source_text";

/// Fixed root layout block ID — must match `:ID:` property on the root heading
/// in index.org. Stored with the `block:` EntityUri scheme prefix.
pub const ROOT_LAYOUT_BLOCK_ID: &str = "block:root-layout";

/// Returns ROOT_LAYOUT_BLOCK_ID as a typed EntityUri.
pub fn root_layout_block_uri() -> EntityUri {
    EntityUri::block("root-layout")
}

/// Fixed id of the `__default__` page that owns the bundled 3-column layout
/// (root-layout + sidebars). A real block id — deliberately NOT the
/// `sentinel:no_parent` marker (see `FrontendSession::default_doc_uri`). Stored
/// with the `block:` scheme prefix. Single source of truth so prod and tests
/// agree on what counts as the default document root.
pub const DEFAULT_DOC_BLOCK_ID: &str = "block:__default__";

/// Returns DEFAULT_DOC_BLOCK_ID as a typed EntityUri.
pub fn default_doc_block_uri() -> EntityUri {
    EntityUri::block("__default__")
}

/// True when `id` is a copy-on-write seed-origin document root — the bundled
/// default layout that ships in `assets/default/` and is re-seeded on every
/// boot. Such a doc is VIRTUAL: it lives only in Loro/SQL, is refreshed from
/// the current asset on boot, and is NEVER auto-materialized to a vault `.org`
/// file. The first user edit materializes the file (via the runtime page
/// write-back path), and from then on that file wins and suppresses re-seeding.
///
/// Anchored on `block:__default__` (the ruled anchor case): the layout
/// container is the only seed-origin root today. A general seed-provenance flag
/// would snowball through Block/SQL/Loro; this typed predicate is the single
/// source of truth callers consult instead of re-matching the id string.
pub fn is_seed_layout_doc(id: &EntityUri) -> bool {
    *id == default_doc_block_uri()
}

// Re-export block types
// Re-export auth types
pub use arcs::ArcEmit;
pub use arcs::ArcParseError;
pub use arcs::ArcPlace;
pub use arcs::ArcRelation;
pub use arcs::TransitionArcs;
pub use auth::ProviderAuthStatus;
pub use block::Block;
pub use block::BlockContent;
pub use block::BlockMetadata;
pub use block::BlockResult;
pub use block::BlockWire;
pub use block::PAGE_TAG;
pub use block::ResultOutput;
pub use block::SnapshotBlock;
pub use block::SourceBlock;
pub use block::blocks_by_document;
// Re-export the intent ChangeSet vocabulary (block-sync rework, Phase 2)
pub use block_write_field::{BlockWriteField, BlockWriteFieldError, PropertyKey};
pub use change_set::ChangeOp;
pub use change_set::ChangeSet;
pub use change_set::Provenance;
pub use change_set::agrees_with_ops;
pub use change_set::source_op_names;
pub use clock::CalendarDate;
pub use clock::Clock;
pub use clock::Grain;
pub use clock::InjectedClock;
pub use clock::SystemClock;
pub use clock::TestClock;
// Re-export the block edge-field category
pub use edge_field::{BlockEdges, EdgeField, EdgeFieldUpdate};
// Re-export entity types (for Entity derive macro)
pub use entity::{
    ColumnValueKind, ComputedSpec, ComputedTier, DynamicEntity, FieldLifetime, FieldSchema,
    HomeProfileId, IntoEntity, InvalidHomeProfileId, POSITION_AFTER_BLOCK_ID_PARAM, ProfileVariant,
    ROUTING_DOC_URI_KEY, StorageEntity, TryFromEntity, TypeDefinition, TypeSource, WriteAuthority,
};
// Re-export entity URI type
pub use entity_uri::EntityUri;
// Re-export CompiledExpr from holon-expr for FieldLifetime::Computed
pub use holon_expr::CompiledExpr;
// The engine `ComputedSpec::parse` expects, re-exported so a caller declaring a
// computed field does not need a direct rhai dependency to supply one.
pub use holon_expr::bounded_engine;
// Re-export the free-function extractor + its unoptimized-compile engine: the
// profile boot path proves every entity-lookup a bundled computed field calls
// is registered on the engine.
pub use holon_expr::referenced_functions;
pub use holon_expr::unoptimized_engine;
// The guard AST, the transition-arc vocabulary and the dynamic `Value` live in
// the leaf crate `holon-pattern` (reachable from `holon-macros`, which parses
// both declaration surfaces at expansion time); these are their canonical paths.
pub use holon_pattern::AmbiguousKind;
pub use holon_pattern::PropertyKinds;
pub use holon_pattern::PropertyKindsError;
pub use holon_pattern::REMOVED_MARKER_KEY;
pub use holon_pattern::RemovedTag;
pub use holon_pattern::Value;
pub use holon_pattern::arcs;
pub use holon_pattern::kind_envelope;
pub use holon_pattern::marking;
pub use holon_pattern::pattern;
pub use holon_pattern::schema;
// Re-export inline-mark types (rich text)
pub use inline_mark::{
    DerivedLink, EntityRef, InlineMark, LinkKind, MarkClass, MarkSpan, SplitContentMarks,
    SplitSide, StyleFlags, StyledRun, canonicalize_marks, canonicalize_marks_against,
    derive_block_links, mark_style_flags, marks_from_json, marks_to_json, split_content_marks,
    style_fingerprint,
};
// Re-export input types
pub use input_types::{Key, KeyChord};
// Re-export interpreter-level value type (non-serializable — runtime only).
/// flutter_rust_bridge:ignore
pub use interp_value::{
    InterpValue, Occurrence, OccurrenceId, ReactiveRowProvider, RowKey, ptr_identity,
};
// CompletionStateInfo is defined in holon-core and re-exported here for frontend use
// The actual definition is in holon-core/src/traits.rs

// Re-export link search candidate
pub use link_candidate::LinkCandidate;
pub use link_candidate::QuickOpenResults;
// Re-export the declared marking-delta vocabulary (ADR 0032 §4)
pub use marking::{
    AspectChange, DeltaViolation, ExistenceFlow, KindDelta, MarkingDelta, ObservedDelta, Placement,
    RowState, StructuralEvidence, StructuralFlow, TextFlow,
};
// Re-export the dual-evaluated Pattern guard AST (ADR 0024 Phase-2 spike)
pub use pattern::{
    BuiltinRef, CmpOp, CurrentSchema, FieldRef, Guard, GuardParseError, GuardResult, InMemoryWorld,
    OpGuard, Operand, PathPattern, PathSegment, Pattern, SchemaAbstraction, Subject, WorldBlock,
};
// Re-export predicate types
pub use predicate::Predicate;
// Re-export query context
pub use query_context::PathContext;
pub use query_context::QueryContext;
// Re-export reactive types
pub use reactive::{
    CdcAccumulator, MapDiff, OperatorStream, ReactiveStreamExt, UiEventResult, UiState,
    apply_map_diff, coalesce, combine_latest, materialize_map,
};
/// flutter_rust_bridge:ignore
pub use render_eval::ResolvedArgs;
/// flutter_rust_bridge:ignore
pub use render_eval::eval_binary_op;
/// flutter_rust_bridge:ignore
pub use render_eval::eval_to_value;
/// flutter_rust_bridge:ignore
pub use render_eval::is_template_arg;
/// flutter_rust_bridge:ignore
pub use render_eval::is_template_arg_for;
/// flutter_rust_bridge:ignore
pub use render_eval::resolve_args;
// Re-export render types
pub use render_types::{
    Arg, BinaryOperator, BoundaryBehavior, ClickModifiers, MenuExposure, NonMenuSurface, Operation,
    OperationDescriptor, OperationParam, OperationWiring, ParamMapping, PickerKind, RenderExpr,
    RenderProfile, RenderVariant, RenderableItem, RowProfile, RowTemplate, SurfaceSet, TargetScope,
    Trigger, TypeHint, ViewSpec, extract_widget_names,
};
// Re-export streaming types
pub use streaming::{
    Batch, BatchMapChange, BatchMapChangeWithMetadata, BatchMetadata, BatchTraceContext,
    BatchWithMetadata, BlockChange, CHANGE_ORIGIN_COLUMN, Change, ChangeOrigin,
    EnrichedChangeStream, MapChange, StreamPosition, SyncTokenUpdate, UiEvent, WatchHandle,
    WatcherCommand, WithMetadata,
};
// Re-export typed domain types
pub use types::{
    ContentType, DependsOn, EntityName, NavigationOp, Priority, QueryLanguage, Region,
    SourceLanguage, StateCategory, Tags, TaskState, Timestamp, UiInfo,
};
// Re-export widget meta types
// Note: StaticParam and WidgetMeta use &'static str which FRB wraps as unsized `str`.
// They are marked flutter_rust_bridge:ignore but still need pub use for Rust macro codegen.
pub use widget_meta::{StaticParam, WidgetCategory, WidgetMeta};
// Re-export widget spec types
pub use widget_spec::{
    DataRow, DataRowAccumulator, EnrichedRow, RowContentHash, RowIdentity, data_row_entity_uri,
    entity_uri_from_id_str,
};

/// flutter_rust_bridge:non_opaque
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Number {
    Int(i64),
    Float(f64),
}

// Generic over the key type so both `StorageEntity` (Arc<str> keys) and
// String-keyed DataRow/EnrichedRow maps can use the same id boundary.
pub fn row_id<K>(row: &HashMap<K, Value>) -> anyhow::Result<EntityUri>
where
    K: std::borrow::Borrow<str> + std::hash::Hash + Eq + std::fmt::Debug,
{
    uri_from_row(row, "id")
}

pub fn uri_from_row<K>(row: &HashMap<K, Value>, field: &str) -> anyhow::Result<EntityUri>
where
    K: std::borrow::Borrow<str> + std::hash::Hash + Eq + std::fmt::Debug,
{
    let uri_val = row
        .get(field)
        .ok_or_else(|| anyhow::anyhow!("No {field} found in {row:?}"))?;
    EntityUri::try_from(uri_val.clone()).map_err(|e| {
        // Include the raw value and the whole row in the error so the
        // caller can see exactly which row had the bad URI. Previous
        // error was just "Invalid URI: unexpected character at index N"
        // which gave no context for debugging.
        anyhow::anyhow!("invalid EntityUri for field {field:?} value={uri_val:?} row={row:?}: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_accessors() {
        let v = Value::Boolean(true);
        assert_eq!(v.as_bool(), Some(true));
        assert_eq!(v.as_i64(), None);

        let v = Value::Integer(42);
        assert_eq!(v.as_i64(), Some(42));
        assert_eq!(v.as_f64(), Some(42.0));

        let v = Value::String("hello".to_string());
        assert_eq!(v.as_string(), Some("hello"));

        let v = Value::Null;
        assert!(v.is_null());
    }

    /// Wire-format lockdown: `Value` is `#[serde(untagged)]`, so plain JSON
    /// primitives round-trip without discriminator tags. If this ever breaks
    /// — e.g. someone adds `#[serde(tag = "type")]` — every frontend that
    /// deserializes operation params or query rows starts failing. Lock it
    /// in with concrete JSON so the test fails at the tag site, not later
    /// in an unrelated frontend.
    #[test]
    fn value_serde_wire_format_is_untagged() {
        // Primitives: serialize as bare JSON, no wrapping object.
        assert_eq!(
            serde_json::to_string(&Value::String("hi".into())).unwrap(),
            r#""hi""#,
        );
        assert_eq!(serde_json::to_string(&Value::Integer(42)).unwrap(), "42",);
        assert_eq!(serde_json::to_string(&Value::Float(3.5)).unwrap(), "3.5",);
        assert_eq!(
            serde_json::to_string(&Value::Boolean(true)).unwrap(),
            "true",
        );
        assert_eq!(serde_json::to_string(&Value::Null).unwrap(), "null");

        // Bare primitives deserialize into the expected variant.
        assert_eq!(
            serde_json::from_str::<Value>(r#""hi""#).unwrap(),
            Value::String("hi".into()),
        );
        assert_eq!(
            serde_json::from_str::<Value>("42").unwrap(),
            Value::Integer(42),
        );
        assert_eq!(
            serde_json::from_str::<Value>("3.5").unwrap(),
            Value::Float(3.5),
        );
        assert_eq!(
            serde_json::from_str::<Value>("true").unwrap(),
            Value::Boolean(true),
        );
        assert_eq!(serde_json::from_str::<Value>("null").unwrap(), Value::Null);

        // Critically: there is NO `{"Text": {"value": "..."}}` or
        // `{"String": "..."}` tagged representation. Any frontend that
        // probes for such a shape is working from a wrong mental model.
        assert!(
            !serde_json::to_string(&Value::String("hi".into()))
                .unwrap()
                .contains("Text"),
        );
        assert!(
            !serde_json::to_string(&Value::String("hi".into()))
                .unwrap()
                .contains("String"),
        );
    }

    /// Operation params come in as a flat JSON object from frontends:
    /// `{"id": "block:foo", "content": "hello"}`. Each value must
    /// deserialize into the matching `Value` variant without any tagging.
    #[test]
    fn value_map_params_round_trip_untagged() {
        use std::collections::HashMap;
        let js_input = r#"{
            "id": "block:foo",
            "content": "hello world",
            "priority": 3,
            "done": true,
            "ratio": 0.5,
            "deleted": null
        }"#;
        let parsed: HashMap<String, Value> =
            serde_json::from_str(js_input).expect("parse params_json");
        assert_eq!(parsed["id"], Value::String("block:foo".into()));
        assert_eq!(parsed["content"], Value::String("hello world".into()));
        assert_eq!(parsed["priority"], Value::Integer(3));
        assert_eq!(parsed["done"], Value::Boolean(true));
        assert_eq!(parsed["ratio"], Value::Float(0.5));
        assert_eq!(parsed["deleted"], Value::Null);
    }

    #[test]
    fn test_value_from() {
        let v: Value = true.into();
        assert_eq!(v, Value::Boolean(true));

        let v: Value = 42i64.into();
        assert_eq!(v, Value::Integer(42));

        let v: Value = "test".into();
        assert_eq!(v, Value::String("test".to_string()));

        let v: Value = None::<i64>.into();
        assert_eq!(v, Value::Null);

        let v: Value = Some(42).into();
        assert_eq!(v, Value::Integer(42));
    }

    #[test]
    fn test_value_json() {
        let v = Value::Object(
            vec![
                ("name".to_string(), Value::String("test".to_string())),
                ("count".to_string(), Value::Integer(5)),
            ]
            .into_iter()
            .collect(),
        );

        let json = v.to_json_string();
        let parsed = Value::from_json_str(&json).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_value_array() {
        let arr = vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)];
        let v = Value::Array(arr.clone());
        assert_eq!(v.as_array(), Some(&arr));
    }

    /// Behavioral lockdown for the `Value` accessors that mutation testing
    /// found unguarded. Each assertion pins the exact returned payload so a
    /// replacement with `None`, `Some(Default)`, a stubbed literal, or a
    /// deleted match arm is caught.
    #[test]
    fn value_accessor_payloads_are_exact() {
        // as_json_value: real parse on the Json variant, None otherwise.
        let jv = Value::Json(r#"{"a":1}"#.to_string());
        assert_eq!(
            jv.as_json_value(),
            Some(serde_json::json!({"a": 1})),
            "must actually parse the Json string, not None/Default",
        );
        assert_eq!(Value::Integer(1).as_json_value(), None);

        // as_string_owned: clones the actual string, None otherwise.
        assert_eq!(
            Value::String("hello".to_string()).as_string_owned(),
            Some("hello".to_string()),
        );
        assert_eq!(Value::Integer(1).as_string_owned(), None);

        // as_datetime_string: returns the stored RFC3339 text verbatim.
        let dt_str = "2020-01-02T03:04:05+00:00";
        assert_eq!(
            Value::DateTime(dt_str.to_string()).as_datetime_string(),
            Some(dt_str),
        );
        assert_eq!(Value::Integer(1).as_datetime_string(), None);

        // as_datetime: parses to the corresponding chrono instant.
        let expected = chrono::DateTime::parse_from_rfc3339(dt_str)
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            Value::DateTime(dt_str.to_string()).as_datetime(),
            Some(expected),
        );
        assert_eq!(Value::Integer(1).as_datetime(), None);

        // as_object: returns the real map (non-empty), None otherwise.
        let mut m = std::collections::HashMap::new();
        m.insert("k".to_string(), Value::Integer(7));
        let obj = Value::Object(m.clone());
        assert_eq!(obj.as_object(), Some(&m));
        assert_eq!(obj.as_object().map(|o| o.len()), Some(1));
        assert_eq!(Value::Integer(1).as_object(), None);
    }

    /// Behavioral lockdown for the `TryFrom<Value>` conversions flagged by
    /// mutation testing. Pins both the success payload and the boolean
    /// integer-truthiness comparison.
    #[test]
    fn tryfrom_value_conversions_are_exact() {
        // bool: Boolean arm and the `!= 0` truthiness of the Integer arm.
        assert!(bool::try_from(Value::Boolean(true)).unwrap());
        assert!(!bool::try_from(Value::Boolean(false)).unwrap());
        assert!(!bool::try_from(Value::Integer(0)).unwrap());
        assert!(bool::try_from(Value::Integer(5)).unwrap());
        assert!(bool::try_from(Value::String("x".into())).is_err());

        // i32: real value, not Default.
        assert_eq!(i32::try_from(Value::Integer(42)).unwrap(), 42);

        // f64: real value, not Default.
        assert_eq!(f64::try_from(Value::Float(3.5)).unwrap(), 3.5);

        // Vec<T>: real contents, not Default (empty).
        let v: Vec<i64> =
            Vec::try_from(Value::Array(vec![Value::Integer(1), Value::Integer(2)])).unwrap();
        assert_eq!(v, vec![1, 2]);
    }
}

/// Structured error types for API operations.
///
/// These errors are designed to cross FFI boundaries (e.g., Rust to Dart)
/// and provide type-safe error handling in frontends.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum ApiError {
    #[error("Block not found: {id}")]
    BlockNotFound { id: String },

    #[error("Document not found: {doc_id}")]
    DocumentNotFound { doc_id: String },

    #[error("Cyclic move detected: cannot move block {id} to descendant {target_parent}")]
    CyclicMove { id: String, target_parent: String },

    #[error("Invalid operation: {message}")]
    InvalidOperation { message: String },

    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Internal error: {message}")]
    InternalError { message: String },
}
