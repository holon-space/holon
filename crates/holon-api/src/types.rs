use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

use crate::Value;

// =============================================================================
// EntityName
// =============================================================================

/// Typed entity name — the canonical name for a domain entity (e.g., "block",
/// "document").
///
/// Two variants:
/// - `Named`: stores a **URI-scheme-safe** form (underscores replaced with
///   hyphens). Use `as_str()` for URI schemes / profile lookup / operation
///   dispatch. Use `table_name()` for SQL identifiers (converts hyphens back to
///   underscores).
/// - `Wildcard`: the `*` sentinel used by `OperationDispatcher` for broadcast
///   operations. `as_str()` returns `"*"`. `table_name()` panics — wildcards
///   have no SQL table.
///
/// Serializes as a plain string over FFI (Flutter sees `String`).
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq)]
/// @c4 code
pub enum EntityName {
    Named(String),
    Wildcard,
}

impl std::hash::Hash for EntityName {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl EntityName {
    pub fn new(s: impl Into<String>) -> Self {
        let raw = s.into();
        if raw == "*" {
            return Self::Wildcard;
        }
        let normalized = raw.replace('_', "-");
        debug_assert!(
            is_valid_uri_scheme(&normalized),
            "Invalid entity name after normalization: {normalized}"
        );
        Self::Named(normalized)
    }

    /// The canonical name, valid as a URI scheme. Returns `"*"` for Wildcard.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Named(s) => s,
            Self::Wildcard => "*",
        }
    }

    /// SQL-safe table name: converts hyphens back to underscores.
    /// Panics on Wildcard — wildcards have no SQL table.
    pub fn table_name(&self) -> String {
        match self {
            Self::Named(s) => s.replace('-', "_"),
            Self::Wildcard => panic!("Wildcard entity has no table name"),
        }
    }

    pub fn is_wildcard(&self) -> bool {
        matches!(self, Self::Wildcard)
    }
}

impl Serialize for EntityName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EntityName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

/// Check if a string is a valid URI scheme per RFC 3986:
/// `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`
fn is_valid_uri_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

impl fmt::Display for EntityName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for EntityName {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

impl AsRef<str> for EntityName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for EntityName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for EntityName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for EntityName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl From<&str> for EntityName {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for EntityName {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

// =============================================================================
// ContentType
// =============================================================================

/// Block content type discriminator.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Text,
    Source,
    Image,
}

impl ContentType {
    /// Sibling-ordering group for the canonical child list (ADR 0005).
    ///
    /// Non-heading "section content" (`Source`/`Image`) sorts **before**
    /// heading content (`Text`). This is not an aesthetic preference — it
    /// is a structural requirement of outline formats (org-mode, Markdown):
    /// a heading captures everything after it until the next heading, so
    /// any non-heading child must precede the parent's child headings or it
    /// would re-parent under the first heading on the next read. The domain
    /// therefore *defines* the ordered child list as "section content
    /// first, then headings", insertion/`after` order preserved within each
    /// group. Every renderer and the reference oracle order
    /// siblings through this one rule.
    pub fn sibling_order_group(self) -> u8 {
        match self {
            ContentType::Source | ContentType::Image => 0,
            ContentType::Text => 1,
        }
    }

    /// Finer sibling-ordering rank that the render→parse round trip actually
    /// realises. The renderer hoists section content (`Source`/`Image`) ahead
    /// of headings (`Text`) via [`Self::sibling_order_group`], but the org
    /// parser additionally re-emits **all `Source` blocks before all `Image`
    /// blocks** (the source loop precedes the image loop in
    /// `process_headlines`). So the post-round-trip kind order is `Source <
    /// Image < Text`.
    ///
    /// This is DISTINCT from [`Self::sibling_order_group`] on purpose: the
    /// renderer relies on the coarse grouping (both section-content kinds share
    /// group 0, insertion order preserved within the group), while a reference
    /// model that must reproduce the *stored* order after a round trip needs
    /// this finer rank as its primary sort key.
    pub fn parse_order_rank(self) -> u8 {
        match self {
            ContentType::Source => 0,
            ContentType::Image => 1,
            ContentType::Text => 2,
        }
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentType::Text => write!(f, "text"),
            ContentType::Source => write!(f, "source"),
            ContentType::Image => write!(f, "image"),
        }
    }
}

impl FromStr for ContentType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(ContentType::Text),
            "source" => Ok(ContentType::Source),
            "image" => Ok(ContentType::Image),
            other => {
                anyhow::bail!(
                    "Invalid content type: {other:?} (expected \"text\", \"source\", or \"image\")"
                )
            }
        }
    }
}

impl From<ContentType> for Value {
    fn from(ct: ContentType) -> Self {
        Value::String(ct.to_string())
    }
}

impl TryFrom<Value> for ContentType {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => ContentType::from_str(&s)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() }),
            _ => Err("ContentType requires a string Value".into()),
        }
    }
}

// =============================================================================
// QueryLanguage
// =============================================================================

/// Query languages with special dispatch in the engine.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueryLanguage {
    #[serde(rename = "holon_prql")]
    HolonPrql,
    #[serde(rename = "holon_gql")]
    HolonGql,
    #[serde(rename = "holon_sql")]
    HolonSql,
}

impl QueryLanguage {
    pub const ALL: &[QueryLanguage] = &[
        QueryLanguage::HolonPrql,
        QueryLanguage::HolonGql,
        QueryLanguage::HolonSql,
    ];

    /// Returns a SQL `IN (...)` list of all query language string values, for
    /// use in SQL queries.
    pub fn sql_in_list() -> String {
        let items: Vec<_> = Self::ALL.iter().map(|q| format!("'{q}'")).collect();
        format!("({})", items.join(", "))
    }
}

impl fmt::Display for QueryLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryLanguage::HolonPrql => write!(f, "holon_prql"),
            QueryLanguage::HolonGql => write!(f, "holon_gql"),
            QueryLanguage::HolonSql => write!(f, "holon_sql"),
        }
    }
}

impl FromStr for QueryLanguage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "holon_prql" => Ok(QueryLanguage::HolonPrql),
            "holon_gql" => Ok(QueryLanguage::HolonGql),
            "holon_sql" => Ok(QueryLanguage::HolonSql),
            other => anyhow::bail!("Not a query language: {other:?}"),
        }
    }
}

// =============================================================================
// NavigationOp
// =============================================================================

/// Operations understood by the `navigation` entity provider. Single source of
/// truth for the navigation op-name vocabulary: backend dispatch
/// (`NavigationProvider` / `InMemoryNavigationProvider`) and the frontend
/// focus-mirror both parse `OperationIntent.op_name` into this enum, so adding
/// a navigation op becomes a compile error until every consumer handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavigationOp {
    /// Move focus to a block (or clear it when no `block_id` is supplied).
    Focus,
    /// Focus a block and pin it into navigation history.
    FocusPin,
    /// Soft-close a `navigation_history` row by id (sidebar X button).
    Close,
    /// Step backward in navigation history.
    GoBack,
    /// Step forward in navigation history.
    GoForward,
    /// Return to the navigation root (clears focus).
    GoHome,
    /// Set a region's cursor to an already-open `navigation_history` row
    /// (tab switch). Unlike `Focus` it inserts no row, closes no row, and does
    /// not reorder the open set — it moves ONLY the cursor, so the open tabs
    /// keep their stable insertion order and per-tab scroll survives.
    Activate,
    /// Open a block as an ADDITIONAL open tab in a region (modifier-click).
    /// Unlike `Focus` (replace-on-focus) it does NOT close the region's other
    /// open rows; if the block is already open it just activates that tab
    /// (no duplicate). The sole multi-open producer (ADR-0026 tab model, Q2).
    OpenTab,
}

impl NavigationOp {
    pub const ALL: &[NavigationOp] = &[
        NavigationOp::Focus,
        NavigationOp::FocusPin,
        NavigationOp::Close,
        NavigationOp::GoBack,
        NavigationOp::GoForward,
        NavigationOp::GoHome,
        NavigationOp::Activate,
        NavigationOp::OpenTab,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NavigationOp::Focus => "focus",
            NavigationOp::FocusPin => "focus_pin",
            NavigationOp::Close => "close",
            NavigationOp::GoBack => "go_back",
            NavigationOp::GoForward => "go_forward",
            NavigationOp::GoHome => "go_home",
            NavigationOp::Activate => "activate",
            NavigationOp::OpenTab => "open_tab",
        }
    }
}

impl fmt::Display for NavigationOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NavigationOp {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "focus" => Ok(NavigationOp::Focus),
            "focus_pin" => Ok(NavigationOp::FocusPin),
            "close" => Ok(NavigationOp::Close),
            "go_back" => Ok(NavigationOp::GoBack),
            "go_forward" => Ok(NavigationOp::GoForward),
            "go_home" => Ok(NavigationOp::GoHome),
            "activate" => Ok(NavigationOp::Activate),
            "open_tab" => Ok(NavigationOp::OpenTab),
            other => anyhow::bail!("Not a navigation op: {other:?}"),
        }
    }
}

// =============================================================================
// SourceLanguage
// =============================================================================

/// Source block languages. Query languages + render + arbitrary others.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceLanguage {
    Query(QueryLanguage),
    Render,
    /// A reactive rule block (ADR 0024). Its content is a program (the effect
    /// DSL), never display content — the renderer routes it to the rule card.
    HolonRule,
    /// The retired `action` language. Parses to a *typed* deprecation sentinel
    /// (never folded into `Other`) so a legacy `action` block fails loud on the
    /// rule card — "legacy 'action' language; rename to holon_rule" — instead
    /// of silently becoming an inert unknown source. See ADR 0024 WP3.
    LegacyAction,
    Other(String),
}

impl SourceLanguage {
    pub fn as_query(&self) -> Option<QueryLanguage> {
        match self {
            SourceLanguage::Query(q) => Some(*q),
            _ => None,
        }
    }

    pub fn is_prql(&self) -> bool {
        matches!(self, SourceLanguage::Query(QueryLanguage::HolonPrql))
    }

    /// True for a rule head — the current `holon_rule` language or the retired
    /// `action` language. Both mark a block as program (routed to the rule
    /// card).
    pub fn is_rule(&self) -> bool {
        matches!(
            self,
            SourceLanguage::HolonRule | SourceLanguage::LegacyAction
        )
    }

    /// True only for the retired `action` language — a rule that must surface a
    /// loud deprecation status rather than execute silently.
    pub fn is_legacy(&self) -> bool {
        matches!(self, SourceLanguage::LegacyAction)
    }
}

impl fmt::Display for SourceLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceLanguage::Query(q) => write!(f, "{q}"),
            SourceLanguage::Render => write!(f, "render"),
            SourceLanguage::HolonRule => write!(f, "holon_rule"),
            SourceLanguage::LegacyAction => write!(f, "action"),
            SourceLanguage::Other(s) => write!(f, "{s}"),
        }
    }
}

impl FromStr for SourceLanguage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("render") {
            return Ok(SourceLanguage::Render);
        }
        if s.eq_ignore_ascii_case("holon_rule") {
            return Ok(SourceLanguage::HolonRule);
        }
        if s.eq_ignore_ascii_case("action") {
            return Ok(SourceLanguage::LegacyAction);
        }
        match QueryLanguage::from_str(s) {
            Ok(q) => Ok(SourceLanguage::Query(q)),
            Err(_) => Ok(SourceLanguage::Other(s.to_string())),
        }
    }
}

impl serde::Serialize for SourceLanguage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for SourceLanguage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        SourceLanguage::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl From<SourceLanguage> for Value {
    fn from(sl: SourceLanguage) -> Self {
        Value::String(sl.to_string())
    }
}

impl TryFrom<Value> for SourceLanguage {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => SourceLanguage::from_str(&s)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() }),
            _ => Err("SourceLanguage requires a string Value".into()),
        }
    }
}

// =============================================================================
// TaskState
// =============================================================================

/// Whether a task keyword represents an active or done state.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StateCategory {
    Active,
    Done,
}

impl StateCategory {
    /// Canonical stored spelling of the `task_state_category` sidecar
    /// property ("active" / "done"). One source of truth for every writer
    /// (org parse boundary, Loro `set_state`, SQL `cycle_task_state`) so the
    /// stored strings can never drift apart.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Done => "done",
        }
    }
}

/// Well-known done keywords (everything after `|` in org `#+TODO:` config).
const DEFAULT_DONE_KEYWORDS: &[&str] = &["DONE", "CANCELLED", "CLOSED"];

/// Task lifecycle state: a keyword (e.g. "TODO", "DOING", "DONE") paired with
/// its category (Active or Done). The category is determined at the parse
/// boundary — from org `#+TODO:` config, provider mapping, or default lists.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskState {
    pub keyword: String,
    pub category: StateCategory,
}

impl TaskState {
    pub fn new(keyword: impl Into<String>, category: StateCategory) -> Self {
        Self {
            keyword: keyword.into(),
            category,
        }
    }

    pub fn active(keyword: impl Into<String>) -> Self {
        Self::new(keyword, StateCategory::Active)
    }

    pub fn done(keyword: impl Into<String>) -> Self {
        Self::new(keyword, StateCategory::Done)
    }

    /// Categorize a keyword using the default done-keyword list.
    /// Unknown keywords default to Active.
    pub fn from_keyword(keyword: &str) -> Self {
        let category = if DEFAULT_DONE_KEYWORDS.contains(&keyword) {
            StateCategory::Done
        } else {
            StateCategory::Active
        };
        Self::new(keyword, category)
    }

    /// Categorize a keyword using a caller-supplied list of done keywords
    /// (from org `#+TODO:` config or provider mapping).
    pub fn from_keyword_with_done_list(keyword: &str, done_keywords: &[String]) -> Self {
        let category = if done_keywords.iter().any(|dk| dk == keyword) {
            StateCategory::Done
        } else {
            StateCategory::Active
        };
        Self::new(keyword, category)
    }

    /// The `task_state_category` sidecar value for a BARE keyword write
    /// arriving at a storage boundary (widget click intent, `set_state`,
    /// `cycle_task_state`) — i.e. one with no org `#+TODO:` config in hand.
    /// Uses the default done-keyword list, exactly like [`Self::from_keyword`].
    pub fn category_str_for_keyword(keyword: &str) -> &'static str {
        Self::from_keyword(keyword).category.as_str()
    }

    pub fn is_done(&self) -> bool {
        self.category == StateCategory::Done
    }

    pub fn is_active(&self) -> bool {
        !self.is_done()
    }

    /// Whether this state is in the DOING (in-progress) family. Drives the
    /// half-filled progress glyph. `NOW` is LogSeq's in-progress keyword
    /// (its flow is LATER -> NOW -> DONE), so it maps to the same family as
    /// the native `DOING` (ForeignVaultCompat §4). The source keyword is
    /// preserved on `self.keyword`, so this family check never loses the
    /// original spelling on write-back.
    pub fn is_doing(&self) -> bool {
        matches!(self.keyword.as_str(), "DOING" | "NOW")
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.keyword)
    }
}

impl FromStr for TaskState {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_keyword(s))
    }
}

impl From<TaskState> for Value {
    fn from(ts: TaskState) -> Self {
        Value::String(ts.to_string())
    }
}

impl TryFrom<Value> for TaskState {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => Ok(TaskState::from_str(&s).unwrap()),
            _ => Err("TaskState requires a string Value".into()),
        }
    }
}

// =============================================================================
// Priority
// =============================================================================

/// Task priority — decoupled from org's A/B/C letter convention.
///
/// Stored as integer in SQL (High=3, Medium=2, Low=1).
/// Org serialization uses letters (A, B, C).
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    Low = 1,
    Medium = 2,
    High = 3,
}

impl Priority {
    pub fn to_int(self) -> i32 {
        self as i32
    }

    pub fn from_int(n: i32) -> anyhow::Result<Self> {
        match n {
            3 => Ok(Priority::High),
            2 => Ok(Priority::Medium),
            1 => Ok(Priority::Low),
            other => anyhow::bail!("Invalid priority integer: {other} (expected 1, 2, or 3)"),
        }
    }

    pub fn to_letter(self) -> &'static str {
        match self {
            Priority::High => "A",
            Priority::Medium => "B",
            Priority::Low => "C",
        }
    }

    pub fn from_letter(s: &str) -> anyhow::Result<Self> {
        match s.trim() {
            "A" => Ok(Priority::High),
            "B" => Ok(Priority::Medium),
            "C" => Ok(Priority::Low),
            other => anyhow::bail!(
                "Invalid priority letter: {other:?} (expected \"A\", \"B\", or \"C\")"
            ),
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_letter())
    }
}

impl FromStr for Priority {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Priority::from_letter(s)
    }
}

impl From<Priority> for Value {
    fn from(p: Priority) -> Self {
        Value::Integer(p.to_int() as i64)
    }
}

impl TryFrom<Value> for Priority {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Integer(i) => Priority::from_int(i as i32)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() }),
            Value::String(s) => Priority::from_letter(&s)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() }),
            _ => Err("Priority requires an integer or string Value".into()),
        }
    }
}

// =============================================================================
// Region
// =============================================================================

/// Navigation region — a named UI area that can hold focus state.
///
/// Currently: Main, LeftSidebar, RightSidebar. Closed enum — adding a new
/// region requires a code change (schema init, matview, CDC plumbing).
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    Main,
    LeftSidebar,
    RightSidebar,
}

impl Region {
    pub const ALL: &[Region] = &[Region::Main, Region::LeftSidebar, Region::RightSidebar];

    pub fn as_str(&self) -> &'static str {
        match self {
            Region::Main => "main",
            Region::LeftSidebar => "left_sidebar",
            Region::RightSidebar => "right_sidebar",
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Region {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "main" => Ok(Region::Main),
            "left_sidebar" => Ok(Region::LeftSidebar),
            "right_sidebar" => Ok(Region::RightSidebar),
            other => anyhow::bail!(
                "Invalid region: {other:?} (expected \"main\", \"left_sidebar\", or \
                 \"right_sidebar\")"
            ),
        }
    }
}

impl From<Region> for Value {
    fn from(r: Region) -> Self {
        Value::String(r.as_str().to_string())
    }
}

impl TryFrom<Value> for Region {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => Region::from_str(&s)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() }),
            _ => Err("Region requires a string Value".into()),
        }
    }
}

// =============================================================================
// Timestamp
// =============================================================================

/// Parsed timestamp with org-mode angle-bracket format: `<2026-02-21 Fri>` or
/// `<2026-02-21 Fri 10:00>`.
///
/// Named generically — org is just the first SerDe format, not a core concept.
///
/// flutter_rust_bridge:ignore
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    raw: String,
    date: chrono::NaiveDate,
}

impl Timestamp {
    /// Parse an org-mode timestamp string like `<2026-02-21 Fri>` or just
    /// `2026-02-21`.
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        let stripped = raw.trim().trim_start_matches('<').trim_end_matches('>');
        // Take only the date part (first 10 chars or up to first space)
        let date_str = stripped.split_whitespace().next().unwrap_or(stripped);
        let date = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("Failed to parse timestamp {raw:?}: {e}"))?;
        Ok(Timestamp {
            raw: raw.trim().to_string(),
            date,
        })
    }

    pub fn date(&self) -> chrono::NaiveDate {
        self.date
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

impl FromStr for Timestamp {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Timestamp::parse(s)
    }
}

// =============================================================================
// Tags
// =============================================================================

/// Parsed tag set. Tags are an unordered, duplicate-free set: the order they
/// appear in is semantically irrelevant. Backed by a `BTreeSet` so order-
/// independence and de-duplication hold *by construction* — every consumer
/// (diffing, comparison, rendering) gets canonical behaviour for free instead
/// of having to remember to normalize. Iteration yields a deterministic
/// (lexicographically sorted) order. Stored as comma-separated string in SQL,
/// displayed as `:tag1:tag2:` in org-mode. Parsed eagerly at entry boundaries.
///
/// flutter_rust_bridge:ignore
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tags(std::collections::BTreeSet<String>);

impl Tags {
    /// Parse from a comma-separated string (the SQL/properties storage format).
    pub fn from_csv(s: &str) -> Self {
        s.split(',').map(|t| t.to_string()).collect()
    }

    /// Build from an iterator of individual tag strings (e.g. from orgize).
    pub fn from_tag_iter(iter: impl IntoIterator<Item = String>) -> Self {
        iter.into_iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, tag: &str) -> bool {
        self.0.contains(tag)
    }

    /// Insert a tag (trimmed; empty is ignored). Returns whether it was new.
    pub fn insert(&mut self, tag: impl Into<String>) -> bool {
        let tag = tag.into().trim().to_string();
        if tag.is_empty() {
            return false;
        }
        self.0.insert(tag)
    }

    /// Remove a tag. Returns whether it was present.
    pub fn remove(&mut self, tag: &str) -> bool {
        self.0.remove(tag)
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.iter()
    }

    /// Owned `Vec` in canonical (sorted) order — for the few APIs that still
    /// require a `Vec` (e.g. `Value::Array` construction).
    pub fn to_vec(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }

    /// Comma-separated string for SQL/properties storage.
    pub fn to_csv(&self) -> String {
        self.0.iter().cloned().collect::<Vec<_>>().join(",")
    }

    /// Org-mode tag format: `:tag1:tag2:`
    pub fn to_org(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        format!(":{}:", self.0.iter().cloned().collect::<Vec<_>>().join(":"))
    }

    /// The underlying `BTreeSet` (clone) for order-independent comparison.
    pub fn to_set(&self) -> std::collections::BTreeSet<String> {
        self.0.clone()
    }
}

impl fmt::Display for Tags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_csv())
    }
}

impl FromStr for Tags {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Tags::from_csv(s))
    }
}

impl From<Tags> for Value {
    fn from(tags: Tags) -> Self {
        Value::String(tags.to_csv())
    }
}

impl From<Vec<String>> for Tags {
    fn from(v: Vec<String>) -> Self {
        v.into_iter().collect()
    }
}

/// Normalizing collector: trims each tag and drops empties, so a `Tags` can
/// never hold an untrimmed or empty tag (parse-don't-validate invariant).
impl FromIterator<String> for Tags {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Tags(
            iter.into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
        )
    }
}

impl IntoIterator for Tags {
    type Item = String;
    type IntoIter = std::collections::btree_set::IntoIter<String>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Tags {
    type Item = &'a String;
    type IntoIter = std::collections::btree_set::Iter<'a, String>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

// =============================================================================
// DependsOn
// =============================================================================

/// Parsed dependency list. Stored as comma-separated string in SQL/properties,
/// parsed eagerly at entry boundaries into a typed Vec.
///
/// No `Option` wrapper — empty is the zero value (like `Tags`).
///
/// flutter_rust_bridge:ignore
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DependsOn(Vec<String>);

impl DependsOn {
    /// Parse from a comma-separated string (the storage format).
    pub fn from_csv(s: &str) -> Self {
        let ids: Vec<String> = s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        DependsOn(ids)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn contains(&self, id: &str) -> bool {
        self.0.iter().any(|s| s == id)
    }

    pub fn push(&mut self, id: String) {
        self.0.push(id);
    }

    pub fn iter(&self) -> std::slice::Iter<'_, String> {
        self.0.iter()
    }

    /// Comma-separated string for storage.
    pub fn to_csv(&self) -> String {
        self.0.join(",")
    }
}

impl fmt::Display for DependsOn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_csv())
    }
}

impl FromStr for DependsOn {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(DependsOn::from_csv(s))
    }
}

impl From<DependsOn> for Value {
    fn from(d: DependsOn) -> Self {
        Value::String(d.to_csv())
    }
}

impl From<Vec<String>> for DependsOn {
    fn from(v: Vec<String>) -> Self {
        DependsOn(v)
    }
}

// =============================================================================
// UiInfo
// =============================================================================

/// Frontend capability descriptor — tells the backend what the frontend can
/// render.
///
/// Profiles are filtered against this: variants referencing widgets not in
/// `available_widgets` are dropped, so a TUI never receives `tree()` specs.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone)]
pub struct UiInfo {
    pub available_widgets: HashSet<String>,
    pub screen_size: Option<(u32, u32)>,
}

impl UiInfo {
    /// A UiInfo that accepts all widgets (no filtering).
    pub fn permissive() -> Self {
        Self {
            available_widgets: HashSet::new(),
            screen_size: None,
        }
    }

    /// Returns true if this UiInfo performs no filtering (empty
    /// available_widgets = all allowed).
    pub fn is_permissive(&self) -> bool {
        self.available_widgets.is_empty()
    }

    /// Check whether all widget names in `names` are supported.
    /// An empty `available_widgets` set means "all widgets allowed".
    pub fn supports_all(&self, names: &HashSet<String>) -> bool {
        if self.available_widgets.is_empty() {
            return true;
        }
        names.is_subset(&self.available_widgets)
    }
}

impl Default for UiInfo {
    fn default() -> Self {
        Self::permissive()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_round_trip() {
        assert_eq!(ContentType::Text.to_string(), "text");
        assert_eq!(ContentType::Source.to_string(), "source");
        assert_eq!("text".parse::<ContentType>().unwrap(), ContentType::Text);
        assert_eq!(
            "source".parse::<ContentType>().unwrap(),
            ContentType::Source
        );
        assert!("invalid".parse::<ContentType>().is_err());
    }

    #[test]
    fn content_type_value_round_trip() {
        let v: Value = ContentType::Source.into();
        assert_eq!(v, Value::String("source".into()));
        let ct: ContentType = v.try_into().unwrap();
        assert_eq!(ct, ContentType::Source);
    }

    #[test]
    fn source_language_round_trip() {
        assert_eq!(
            "holon_prql".parse::<SourceLanguage>().unwrap(),
            SourceLanguage::Query(QueryLanguage::HolonPrql)
        );
        assert_eq!(
            "render".parse::<SourceLanguage>().unwrap(),
            SourceLanguage::Render
        );
        assert_eq!(
            "python".parse::<SourceLanguage>().unwrap(),
            SourceLanguage::Other("python".into())
        );

        assert_eq!(
            SourceLanguage::Query(QueryLanguage::HolonSql).to_string(),
            "holon_sql"
        );
        assert_eq!(SourceLanguage::Render.to_string(), "render");
        assert_eq!(SourceLanguage::Other("rust".into()).to_string(), "rust");
    }

    #[test]
    fn source_language_rule_and_legacy() {
        // holon_rule parses to the typed rule variant and round-trips.
        assert_eq!(
            "holon_rule".parse::<SourceLanguage>().unwrap(),
            SourceLanguage::HolonRule
        );
        assert_eq!(SourceLanguage::HolonRule.to_string(), "holon_rule");
        assert!(SourceLanguage::HolonRule.is_rule());
        assert!(!SourceLanguage::HolonRule.is_legacy());

        // The retired `action` language parses to a typed deprecation sentinel —
        // NOT Other("action") — so it fails loud, never silently inert.
        let action = "action".parse::<SourceLanguage>().unwrap();
        assert_eq!(action, SourceLanguage::LegacyAction);
        assert_ne!(action, SourceLanguage::Other("action".into()));
        assert_eq!(action.to_string(), "action");
        assert!(action.is_rule());
        assert!(action.is_legacy());

        // Round-trip through serde string form.
        for lang in [SourceLanguage::HolonRule, SourceLanguage::LegacyAction] {
            let s = lang.to_string();
            assert_eq!(s.parse::<SourceLanguage>().unwrap(), lang);
        }
    }

    #[test]
    fn task_state_done_checks() {
        assert!(TaskState::done("DONE").is_done());
        assert!(TaskState::done("CANCELLED").is_done());
        assert!(TaskState::done("CLOSED").is_done());
        assert!(!TaskState::active("TODO").is_done());
        assert!(!TaskState::active("DOING").is_done());
        assert!(!TaskState::active("WAITING").is_done());
    }

    #[test]
    fn task_state_from_keyword_uses_defaults() {
        assert!(TaskState::from_keyword("DONE").is_done());
        assert!(TaskState::from_keyword("CANCELLED").is_done());
        assert!(TaskState::from_keyword("CLOSED").is_done());
        assert!(!TaskState::from_keyword("TODO").is_done());
        assert!(!TaskState::from_keyword("DOING").is_done());
        assert!(!TaskState::from_keyword("WAITING").is_done());
    }

    #[test]
    fn task_state_from_keyword_with_custom_done_list() {
        let done = vec!["DONE".into(), "WONTFIX".into()];
        assert!(TaskState::from_keyword_with_done_list("WONTFIX", &done).is_done());
        assert!(TaskState::from_keyword_with_done_list("DONE", &done).is_done());
        // CANCELLED is NOT in the custom list, so it's Active
        assert!(!TaskState::from_keyword_with_done_list("CANCELLED", &done).is_done());
    }

    #[test]
    fn task_state_round_trip() {
        assert_eq!(
            "TODO".parse::<TaskState>().unwrap(),
            TaskState::active("TODO")
        );
        assert_eq!(
            "DOING".parse::<TaskState>().unwrap(),
            TaskState::active("DOING")
        );
        assert_eq!(
            "DONE".parse::<TaskState>().unwrap(),
            TaskState::done("DONE")
        );
        assert_eq!(
            "CANCELLED".parse::<TaskState>().unwrap(),
            TaskState::done("CANCELLED")
        );
        assert_eq!(
            "WAITING".parse::<TaskState>().unwrap(),
            TaskState::active("WAITING")
        );
        assert_eq!(TaskState::active("TODO").to_string(), "TODO");
    }

    #[test]
    fn timestamp_parse_org_format() {
        let ts = Timestamp::parse("<2026-02-21 Fri>").unwrap();
        assert_eq!(
            ts.date(),
            chrono::NaiveDate::from_ymd_opt(2026, 2, 21).unwrap()
        );
        assert_eq!(ts.as_str(), "<2026-02-21 Fri>");
    }

    #[test]
    fn timestamp_parse_plain_date() {
        let ts = Timestamp::parse("2026-03-01").unwrap();
        assert_eq!(
            ts.date(),
            chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
        );
    }

    #[test]
    fn timestamp_parse_with_time() {
        let ts = Timestamp::parse("<2026-02-21 Fri 10:00>").unwrap();
        assert_eq!(
            ts.date(),
            chrono::NaiveDate::from_ymd_opt(2026, 2, 21).unwrap()
        );
    }

    #[test]
    fn timestamp_parse_invalid() {
        assert!(Timestamp::parse("not-a-date").is_err());
    }

    #[test]
    fn tags_from_csv() {
        // Tags are a set: input is trimmed, de-duplicated, and iterated in
        // canonical (sorted) order regardless of source order.
        let tags = Tags::from_csv("foo, bar ,baz, foo");
        assert_eq!(tags.to_vec(), vec!["bar", "baz", "foo"]);
        assert_eq!(tags.to_csv(), "bar,baz,foo");
        assert_eq!(tags.to_org(), ":bar:baz:foo:");
    }

    #[test]
    fn tags_empty() {
        let tags = Tags::from_csv("");
        assert!(tags.is_empty());
        assert_eq!(tags.to_org(), "");
        assert_eq!(tags.to_csv(), "");
    }

    #[test]
    fn tags_from_vec() {
        let tags = Tags::from(vec!["a".into(), "b".into()]);
        assert_eq!(tags.to_csv(), "a,b");
    }

    #[test]
    fn tags_from_str() {
        let tags: Tags = "x,y,z".parse().unwrap();
        assert_eq!(tags.to_vec(), vec!["x", "y", "z"]);
    }

    #[test]
    fn tags_to_set() {
        let tags = Tags::from_csv("b,a,c");
        let set = tags.to_set();
        let expected: std::collections::BTreeSet<String> =
            ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(set, expected);
    }

    #[test]
    fn priority_letter_round_trip() {
        assert_eq!(Priority::from_letter("A").unwrap(), Priority::High);
        assert_eq!(Priority::from_letter("B").unwrap(), Priority::Medium);
        assert_eq!(Priority::from_letter("C").unwrap(), Priority::Low);
        assert_eq!(Priority::High.to_letter(), "A");
        assert_eq!(Priority::Medium.to_letter(), "B");
        assert_eq!(Priority::Low.to_letter(), "C");
    }

    #[test]
    fn priority_int_round_trip() {
        assert_eq!(Priority::from_int(3).unwrap(), Priority::High);
        assert_eq!(Priority::from_int(2).unwrap(), Priority::Medium);
        assert_eq!(Priority::from_int(1).unwrap(), Priority::Low);
        assert_eq!(Priority::High.to_int(), 3);
        assert_eq!(Priority::Medium.to_int(), 2);
        assert_eq!(Priority::Low.to_int(), 1);
    }

    #[test]
    fn priority_rejects_invalid_letter() {
        assert!(Priority::from_letter("D").is_err());
        assert!(Priority::from_letter("").is_err());
    }

    #[test]
    fn priority_rejects_invalid_int() {
        assert!(Priority::from_int(0).is_err());
        assert!(Priority::from_int(4).is_err());
    }

    #[test]
    fn priority_value_round_trip() {
        let v: Value = Priority::High.into();
        assert_eq!(v, Value::Integer(3));
        let p: Priority = v.try_into().unwrap();
        assert_eq!(p, Priority::High);

        let v: Value = Value::String("B".into());
        let p: Priority = v.try_into().unwrap();
        assert_eq!(p, Priority::Medium);
    }

    #[test]
    fn priority_ordering() {
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
    }

    #[test]
    fn depends_on_from_csv() {
        let deps = DependsOn::from_csv("block-1, block-2 ,block-3");
        assert_eq!(deps.as_slice(), &["block-1", "block-2", "block-3"]);
        assert_eq!(deps.to_csv(), "block-1,block-2,block-3");
    }

    #[test]
    fn depends_on_empty() {
        let deps = DependsOn::from_csv("");
        assert!(deps.is_empty());
        assert_eq!(deps.to_csv(), "");
    }

    #[test]
    fn depends_on_contains() {
        let deps = DependsOn::from_csv("a,b,c");
        assert!(deps.contains("b"));
        assert!(!deps.contains("d"));
    }

    #[test]
    fn depends_on_push() {
        let mut deps = DependsOn::from_csv("a");
        deps.push("b".to_string());
        assert_eq!(deps.as_slice(), &["a", "b"]);
    }

    #[test]
    fn depends_on_from_vec() {
        let deps = DependsOn::from(vec!["x".into(), "y".into()]);
        assert_eq!(deps.to_csv(), "x,y");
    }

    #[test]
    fn depends_on_from_str() {
        let deps: DependsOn = "a,b,c".parse().unwrap();
        assert_eq!(deps.as_slice(), &["a", "b", "c"]);
    }

    #[test]
    fn region_round_trip() {
        assert_eq!(Region::Main.as_str(), "main");
        assert_eq!(Region::LeftSidebar.as_str(), "left_sidebar");
        assert_eq!(Region::RightSidebar.as_str(), "right_sidebar");
        assert_eq!("main".parse::<Region>().unwrap(), Region::Main);
        assert_eq!(
            "left_sidebar".parse::<Region>().unwrap(),
            Region::LeftSidebar
        );
        assert_eq!(
            "right_sidebar".parse::<Region>().unwrap(),
            Region::RightSidebar
        );
        assert!("center".parse::<Region>().is_err());
    }

    #[test]
    fn region_value_round_trip() {
        let v: Value = Region::Main.into();
        assert_eq!(v, Value::String("main".into()));
        let r: Region = v.try_into().unwrap();
        assert_eq!(r, Region::Main);
    }

    #[test]
    fn region_all() {
        assert_eq!(Region::ALL.len(), 3);
        assert!(Region::ALL.contains(&Region::Main));
        assert!(Region::ALL.contains(&Region::LeftSidebar));
        assert!(Region::ALL.contains(&Region::RightSidebar));
    }
}
