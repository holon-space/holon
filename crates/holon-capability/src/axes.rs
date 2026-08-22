//! The fidelity axes a capability profile declares.
//!
//! Increment 2b.1 carries axes 3 (`property_keys`) and 4 (`property_values`)
//! only. The other eight arrive in 2b.2; `deny_unknown_fields` means a yaml
//! naming one of them is a LOAD ERROR today rather than a silently ignored
//! section, so a profile can never claim more than the code checks.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

/// A property-key prefix the format OWNS: a key carrying it does not survive
/// as an ordinary property.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReservedPrefix(String);

impl ReservedPrefix {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An exact property key, used both for the format's reserved list and for
/// naming the offending key in a [`crate::Violation`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PropertyKey(String);

impl PropertyKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which key spellings the format can carry at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCharset {
    Any,
    /// A key containing whitespace is not a property at all.
    NoWhitespace,
    Identifier,
    KeywordNamespaced,
}

/// Whether the format preserves the authored spelling of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCase {
    Sensitive,
    FoldedUpper,
    FoldedLower,
}

/// What happens when two keys collide after `case` folding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    LastWins,
    FirstWins,
    Error,
    MultiValued,
}

/// Whether an undeclared key is an error (logseq-db) or simply carried (org).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaRequirement {
    /// Any key may be written without prior declaration.
    Open,
    /// A key the schema does not declare is refused.
    Declared,
}

/// Axis 3 — what the format can carry as a property KEY.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyKeysAxis {
    pub charset: KeyCharset,
    pub case: KeyCase,
    #[serde(default)]
    pub reserved_prefixes: Vec<ReservedPrefix>,
    #[serde(default)]
    pub reserved_keys: Vec<PropertyKey>,
    pub collision: Collision,
    pub schema_required: SchemaRequirement,
}

impl PropertyKeysAxis {
    /// Whether `key` carries a prefix the format ERASES.
    ///
    /// Distinct from [`Self::is_owned`] on purpose: a prefix reservation is a
    /// statement that the key does not come back, so its loss is honest and
    /// its SURVIVAL is the surprise.
    pub fn is_prefix_reserved(&self, key: &str) -> bool {
        self.reserved_prefixes
            .iter()
            .any(|p| key.starts_with(p.as_str()))
    }

    /// Whether `key` is one the format OWNS by exact spelling.
    ///
    /// An owned key is not an ordinary property and is not claimed to vanish
    /// — `ID` both survives and means something. What it round-trips THROUGH
    /// is the format's own machinery, so the ordinary-property law says
    /// nothing about it; axis 7 (`identity`) is what certifies it, and that
    /// axis arrives in 2b.2.
    pub fn is_owned(&self, key: &str) -> bool {
        self.reserved_keys.iter().any(|k| k.as_str() == key)
    }
}

/// The `Value` variants, as a value space a profile can name.
///
/// Mirrors `holon_pattern::Value` (`crates/holon-pattern/src/value.rs:21-37`).
/// A separate enum rather than `Value` itself because a profile names KINDS,
/// never inhabitants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Json,
    Array,
    Object,
    Null,
}

impl ValueKind {
    pub fn of(value: &holon_api::Value) -> Self {
        match value {
            holon_api::Value::String(_) => Self::String,
            holon_api::Value::Integer(_) => Self::Integer,
            holon_api::Value::Float(_) => Self::Float,
            holon_api::Value::Boolean(_) => Self::Boolean,
            holon_api::Value::DateTime(_) => Self::DateTime,
            holon_api::Value::Json(_) => Self::Json,
            holon_api::Value::Array(_) => Self::Array,
            holon_api::Value::Object(_) => Self::Object,
            holon_api::Value::Null => Self::Null,
        }
    }
}

/// Whether a particular inhabitant survives, vanishes, or is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Representability {
    Representable,
    Dropped,
    Error,
}

/// How the format carries more than one value under one key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MultiValue {
    None,
    Delimited {
        /// EVERY delimiter that splits, not one of them. org's edge fields
        /// split on a comma OR any whitespace
        /// (`crates/holon-org-format/src/parser.rs:1493`), and a single string
        /// cannot say that: declaring either value alone was true, so neither
        /// could be falsified by flipping it to the other.
        separators: BTreeSet<Separator>,
        semantics: MultiValueSemantics,
        scope: MultiValueScope,
    },
    NativeVector {
        semantics: MultiValueSemantics,
    },
}

/// One delimiter that splits a multi-valued field. Non-empty by construction —
/// an empty separator would split every character and is never a real claim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Separator(String);

impl Separator {
    pub fn new(sep: impl Into<String>) -> Result<Self, String> {
        let sep = sep.into();
        if sep.is_empty() {
            return Err("a separator may not be empty".to_string());
        }
        Ok(Self(sep))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Separator {
    type Error = String;

    fn try_from(sep: String) -> Result<Self, Self::Error> {
        Self::new(sep)
    }
}

impl From<Separator> for String {
    fn from(sep: Separator) -> Self {
        sep.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiValueSemantics {
    /// Order is semantic and must round-trip.
    List,
    /// Order is not semantic; the format may reorder.
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiValueScope {
    /// Every property splits on the separator.
    AllProperties,
    /// Only the format's own edge fields split; an ordinary property
    /// containing the separator stays one value.
    EdgeFieldsOnly,
}

/// How a property NAMES another entity. Naming only — how MANY references a
/// property carries is `multi_value`'s question, and a value that answered
/// both let one axis certify the other's observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceValues {
    None,
    ByName,
    ById,
}

impl<'de> Deserialize<'de> for ReferenceValues {
    /// Hand-written for ONE reason: the retired `vector_of_refs` must fail with
    /// a message that says where the concept went. `unknown variant` would send
    /// the reader looking for a typo.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        match value.as_str() {
            "none" => Ok(Self::None),
            "by_name" => Ok(Self::ByName),
            "by_id" => Ok(Self::ById),
            "vector_of_refs" => Err(serde::de::Error::custom(
                "`vector_of_refs` states a CARDINALITY, which the `multi_value` axis governs; \
                 `reference_values` states NAMING only (none | by_id | by_name)",
            )),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["none", "by_id", "by_name"],
            )),
        }
    }
}

/// Axis 4 — what the format can carry as a property VALUE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyValuesAxis {
    /// The `Value` kinds that round-trip preserving BOTH kind and inhabitant.
    /// A kind that survives only by being re-typed (org's integer coming back
    /// as `String`) does NOT belong here — see the org profile's rationale.
    pub types: BTreeSet<ValueKind>,
    pub empty_string: Representability,
    pub null: Representability,
    pub multi_value: MultiValue,
    pub reference_values: ReferenceValues,
}

// =============================================================================
// Axis 1 — hosted_kinds
// =============================================================================

/// What SHAPE of entity a format can home at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedKind {
    /// A block in a tree — it has a parent and a position among siblings.
    Hierarchical,
    /// A typed row that belongs to no tree.
    FreeStanding,
}

// =============================================================================
// Axis 2 — content
// =============================================================================

/// How much of the content the format actually MODELS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentRepresentation {
    /// A string carried through without being parsed.
    OpaqueText,
    /// Parsed into marks and re-emitted.
    MarkedText,
    StructuredTree,
    None,
}

/// Inline marks, from a CLOSED vocabulary — a format cannot invent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InlineConstruct {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Verbatim,
    Code,
    Subscript,
    Superscript,
    LinkByName,
    LinkById,
    LinkExternal,
    Tag,
    EscapeSequence,
}

/// Block-level constructs, likewise closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockConstruct {
    Heading,
    Paragraph,
    SourceBlock,
    Quote,
    Table,
    List,
    Image,
    Logbook,
    PlanningTimestamp,
    TodoKeyword,
    Priority,
}

/// Axis 2 — what the format can carry as CONTENT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentAxis {
    pub representation: ContentRepresentation,
    #[serde(default)]
    pub inline_constructs: BTreeSet<InlineConstruct>,
    #[serde(default)]
    pub block_constructs: BTreeSet<BlockConstruct>,
}

// =============================================================================
// Axis 5 — ordering
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiblingOrder {
    /// Order IS position in the file; there is no key.
    FilePosition,
    FractionalIndex,
    ExplicitInteger,
    LinkedList,
    Unordered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderKeyDurability {
    /// The authored key comes back byte-identical.
    Authored,
    Carried,
    /// Read to recover the sequence, then RE-MINTED — a declared fidelity gap.
    CarriedButReminted,
    /// No key on disk; order is derived from something else.
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrentInsert {
    Stable,
    PositionalConflict,
    #[serde(rename = "n_a")]
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyOrder {
    /// The author's key order comes back.
    Preserved,
    /// Deterministic, but not the author's.
    Canonical,
    Unspecified,
}

/// Axis 5 — who owns ORDER, and whether it survives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderingAxis {
    pub sibling_order: SiblingOrder,
    pub order_key_durable: OrderKeyDurability,
    pub concurrent_insert: ConcurrentInsert,
    pub property_order: PropertyOrder,
}

// =============================================================================
// Axis 6 — hierarchy
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HierarchyShape {
    Flat,
    Tree,
    Forest,
    Dag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxDepth {
    Unbounded,
    Limit(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reparent {
    Free,
    Constrained,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cycles {
    Rejected,
    Representable,
}

/// A named structural rule the format enforces on reparenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintId {
    /// A `:Page:`-tagged heading may not sit under a non-page ancestor.
    PageTagRequiresPageAncestor,
    /// A page name containing `/` is refused rather than imported wrong.
    NoSlashInPageName,
    /// The id must form a valid URI path (`EntityUri::from_raw` parses via
    /// fluent_uri). MEASURED to be enforced by PANIC, not by `Err`.
    ValidUriPath,
}

/// Axis 6 — the SHAPE the format can hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HierarchyAxis {
    pub shape: HierarchyShape,
    pub max_depth: MaxDepth,
    pub reparent: Reparent,
    #[serde(default)]
    pub constraints: Vec<ConstraintId>,
    pub cycles: Cycles,
}

// =============================================================================
// Axis 7 — identity
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdSpace {
    Uuid,
    OpaqueString,
    PathDerived,
    NameDerived,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdOrigin {
    Authored,
    MintedOnWrite,
    DerivedFromPosition,
}

/// A rename the identity must SURVIVE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenameKind {
    FileRename,
    TitleRename,
    Move,
}

/// One place identity can live. Order in the profile is PRECEDENCE order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdCarrier {
    DrawerId,
    FileKeywordId,
    /// Identity derived from the file's PATH plus the vault root. Distinct from
    /// `NameChain`, which is LogSeq's page-name sense.
    PathDerived,
    NameChain,
    BlockUuid,
    BlockName,
}

/// Every carrier the vocabulary knows.
///
/// The carriers law ranges over THIS, not over what a profile declares:
/// ranging over the declared set makes a DELETION always satisfy the law, so a
/// profile could drop a real carrier and stay green — measured as silent flip
/// S8.
pub const ALL_ID_CARRIERS: &[IdCarrier] = &[
    IdCarrier::DrawerId,
    IdCarrier::FileKeywordId,
    IdCarrier::PathDerived,
    IdCarrier::NameChain,
    IdCarrier::BlockUuid,
    IdCarrier::BlockName,
];

/// What happens when two carriers name DIFFERENT identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierDisagreement {
    /// A loud parse error — never a silent pick.
    Error,
    /// The first carrier in precedence order wins, silently.
    PrecedenceWins,
}

/// Axis 7 — identity, and what it survives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityAxis {
    pub id_space: IdSpace,
    pub id_origin: IdOrigin,
    #[serde(default)]
    pub id_constraints: Vec<ConstraintId>,
    pub rename_stability: BTreeSet<RenameKind>,
    pub carriers: Vec<IdCarrier>,
    pub carrier_disagreement: CarrierDisagreement,
}

// =============================================================================
// Axis 8 — computed
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputedLive {
    Full,
    ScriptOnly,
    None,
}

/// What a format can DURABLY store as a computed result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComputedPersisted {
    FullAlgebra,
    TypedSubset { types: BTreeSet<ValueKind> },
    StringOnly,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionClosure {
    ComputationAlgebra,
    ComputationPlusScript,
    None,
}

/// Axis 8 — the computed tiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputedAxis {
    pub computed_live: ComputedLive,
    pub computed_persisted: ComputedPersisted,
    pub expression_closure: ExpressionClosure,
}

// =============================================================================
// Axis 9 — mutation
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteLeg {
    /// No code path writes this format. Every mutating action is un-offered.
    Absent,
    File,
    Api,
    InProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteUnit {
    Field,
    Entity,
    Container,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeGranularity {
    Character,
    Field,
    Entity,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSurface {
    None,
    PropertyBanner,
    Log,
    Ui,
}

/// Axis 9 — whether and how the format is WRITTEN.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationAxis {
    pub write_leg: WriteLeg,
    pub unit_of_write: WriteUnit,
    pub merge_granularity: MergeGranularity,
    pub conflict_surface: ConflictSurface,
}

// =============================================================================
// Axis 10 — assets
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attachments {
    None,
    InlineReference,
    ManagedStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryInline {
    None,
    DataUri,
    Native,
}

/// A permitted attachment file extension, lowercase and without the dot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Extension(String);

impl Extension {
    pub fn new(ext: impl Into<String>) -> Self {
        Self(ext.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Axis 10 — what the format can carry as an ATTACHMENT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetsAxis {
    pub attachments: Attachments,
    pub binary_inline: BinaryInline,
    #[serde(default)]
    pub extensions: BTreeSet<Extension>,
}
