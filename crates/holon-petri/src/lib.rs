//! @c4 component
//! @c4 layer Engine
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-engine "Petri-net engine" "Rust"
//!
//! Materialization layer: Holon task blocks → Petri Net for WSJF ranking.
//!
//! Reads task blocks from the database and constructs a Petri Net where:
//! - Tokens represent entities (the user, referenced people/documents)
//! - Transitions represent tasks (with dependency ordering via completion tokens)
//! - The objective function scores tasks via prototypal inheritance with `=` computed properties
//!
//! Prototype blocks define both literal defaults and `=`-prefixed Rhai computed attributes.
//! Instance (task) blocks inherit from and override prototype properties.
//!
//! Content prefix parsing order (each strips its marker):
//! 1. `>` — sequential dependency on previous sibling
//! 2. `@[[Person]]:` — delegation to another person
//! 3. `?` — question producing a knowledge token
//!
//! The engine then ranks enabled transitions by WSJF (Δobj / duration).

use chrono::{DateTime, Utc};
use holon_api::block::Block;
use holon_api::types::{DependsOn, Priority, TaskState, Timestamp};
use holon_api::{CompiledExpr, EntityUri};
use holon_engine::arc::{AttrInit, CreateArc, InputArc, OutputArc};
use holon_engine::value::Value;
use holon_engine::{Marking, NetDef, PrecondSpec, TokenState, TransitionDef};
use rhai::{Dynamic, Engine as RhaiEngine, Scope};
use std::collections::{BTreeMap, HashMap, HashSet};

pub use holon_engine::engine::{Engine, RankedTransition};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failures materializing task blocks into a Petri net. These arise from
/// *external* stored data (org drawer properties, block content, prototype
/// blocks) reached via the live `rank_tasks` MCP tool, so they are returned as
/// errors — never `panic!` — so the tool surfaces them instead of aborting the
/// process (fail-loud, not fail-by-crash).
#[derive(Debug, thiserror::Error)]
pub enum PetriError {
    #[error("stored property '{name}' on block {block_id} is not numeric: {detail}")]
    NonNumericProperty {
        block_id: String,
        name: String,
        detail: String,
    },
    #[error("stored property '{name}' = {value} on block {block_id} is not an integer")]
    NonIntegerProperty {
        block_id: String,
        name: String,
        value: f64,
    },
    #[error("stored '{field}' on block {block_id} has unexpected type: {detail}")]
    UnexpectedPropertyType {
        block_id: String,
        field: String,
        detail: String,
    },
    #[error("invalid prototype property '{name}' on block {block_id}: {detail}")]
    InvalidPrototypeProperty {
        block_id: String,
        name: String,
        detail: String,
    },
    #[error("stored priority {value} on block {block_id} is invalid: {detail}")]
    InvalidPriority {
        block_id: String,
        value: i64,
        detail: String,
    },
    #[error("stored deadline {value:?} on block {block_id} is not a valid timestamp: {detail}")]
    InvalidDeadline {
        block_id: String,
        value: String,
        detail: String,
    },
    #[error("Rhai eval error for computed property '{name}': {detail}\n  expr: {expr}")]
    ComputedEval {
        name: String,
        detail: String,
        expr: String,
    },
    #[error("Rhai computed property '{name}' returned non-numeric: {detail}")]
    ComputedNonNumeric { name: String, detail: String },
    #[error("block ids {a:?} and {b:?} both sanitize to Rhai identifier fragment {frag:?}")]
    FragmentCollision { a: String, b: String, frag: String },
    #[error("failed to compile objective expression: {detail}")]
    ObjectiveCompile { detail: String },
    #[error(
        "stored duration {value} minutes on block {block_id} is out of range \
         (expected 1..={max} — ~10 years); chrono clock arithmetic overflows \
         far past this"
    )]
    DurationOutOfRange {
        block_id: String,
        value: i64,
        max: i64,
    },
}

/// Upper bound for task/prototype durations, ~10 years in minutes. Values
/// beyond this are certainly data-entry errors and (much further out)
/// overflow chrono's clock arithmetic in `Engine::fire`.
pub const MAX_DURATION_MINUTES: i64 = 10 * 365 * 24 * 60;

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct TaskToken {
    pub id: String,
    pub token_type: String,
    pub attributes: BTreeMap<String, Value>,
}

impl TokenState for TaskToken {
    fn id(&self) -> &str {
        &self.id
    }
    fn token_type(&self) -> &str {
        &self.token_type
    }
    fn get(&self, attr: &str) -> Option<&Value> {
        self.attributes.get(attr)
    }
    fn attrs(&self) -> &BTreeMap<String, Value> {
        &self.attributes
    }
}

// ---------------------------------------------------------------------------
// Transition
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct TaskTransition {
    pub id: String,
    /// The block id of the task this transition materializes. For delegated
    /// tasks the `{id}_delegate` sub-transition shares its parent task's block
    /// id — engine identity lives in `id`, API-facing block identity here.
    pub source_block_id: String,
    pub label: String,
    pub inputs: Vec<InputArc>,
    pub outputs: Vec<OutputArc>,
    pub creates: Vec<CreateArc>,
    pub duration: f64,
}

impl TransitionDef for TaskTransition {
    fn id(&self) -> &str {
        &self.id
    }
    fn inputs(&self) -> &[InputArc] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputArc] {
        &self.outputs
    }
    fn creates(&self) -> &[CreateArc] {
        &self.creates
    }
    fn duration_minutes(&self) -> f64 {
        self.duration
    }
}

// ---------------------------------------------------------------------------
// Net
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TaskNet {
    pub transitions: Vec<TaskTransition>,
    pub objective_expr: CompiledExpr,
}

impl PartialEq for TaskNet {
    fn eq(&self, other: &Self) -> bool {
        self.transitions == other.transitions && self.objective_expr == other.objective_expr
    }
}

impl NetDef for TaskNet {
    type Transition = TaskTransition;

    fn transitions(&self) -> Box<dyn Iterator<Item = &TaskTransition> + '_> {
        Box::new(self.transitions.iter())
    }

    fn transition(&self, id: &str) -> Option<&TaskTransition> {
        self.transitions.iter().find(|t| t.id == id)
    }

    fn objective_expr(&self) -> &CompiledExpr {
        &self.objective_expr
    }

    // The task adapter has no economic constraints and does not discount:
    // the generated objective never references `discount`, so a discount rate
    // would be inert. Both dimensions stay available on the generic `NetDef`
    // trait for YAML nets that do use them.
    fn constraints(&self) -> &[CompiledExpr] {
        &[]
    }

    fn discount_rate(&self) -> f64 {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Marking
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct TaskMarking {
    pub clock: DateTime<Utc>,
    pub tokens: Vec<TaskToken>,
}

impl Marking for TaskMarking {
    type Token = TaskToken;

    fn clock(&self) -> DateTime<Utc> {
        self.clock
    }

    fn set_clock(&mut self, t: DateTime<Utc>) {
        self.clock = t;
    }

    fn tokens_of_type(&self, token_type: &str) -> Vec<&TaskToken> {
        self.tokens
            .iter()
            .filter(|t| t.token_type == token_type)
            .collect()
    }

    fn tokens(&self) -> Box<dyn Iterator<Item = &TaskToken> + '_> {
        Box::new(self.tokens.iter())
    }

    fn token(&self, id: &str) -> Option<&TaskToken> {
        self.tokens.iter().find(|t| t.id == id)
    }

    fn set_attr(&mut self, token_id: &str, attr: &str, value: Value) {
        let token = self
            .tokens
            .iter_mut()
            .find(|t| t.id == token_id)
            .unwrap_or_else(|| panic!("token '{token_id}' not found"));
        token.attributes.insert(attr.to_string(), value);
    }

    fn create_token(&mut self, id: String, token_type: String, attrs: BTreeMap<String, Value>) {
        assert!(
            self.tokens.iter().all(|t| t.id != id),
            "token '{id}' already exists"
        );
        self.tokens.push(TaskToken {
            id,
            token_type,
            attributes: attrs,
        });
    }

    fn remove_token(&mut self, id: &str) {
        let len_before = self.tokens.len();
        self.tokens.retain(|t| t.id != id);
        assert!(
            self.tokens.len() < len_before,
            "token '{id}' not found for removal"
        );
    }
}

// ---------------------------------------------------------------------------
// Prototype system — replaces MaterializeConfig + scoring helpers
// ---------------------------------------------------------------------------

/// A prototype property value: either a literal number or a pre-compiled Rhai expression.
#[derive(Clone, Debug)]
pub enum PrototypeValue {
    Literal(f64),
    Computed(CompiledExpr),
}

impl PartialEq for PrototypeValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PrototypeValue::Literal(a), PrototypeValue::Literal(b)) => {
                (a - b).abs() < f64::EPSILON
            }
            (PrototypeValue::Computed(a), PrototypeValue::Computed(b)) => a == b,
            _ => false,
        }
    }
}

impl PrototypeValue {
    /// Parse a raw string into a PrototypeValue. `=`-prefixed strings become Computed
    /// (compiled immediately), otherwise the string must parse as f64.
    pub fn parse(engine: &RhaiEngine, raw: &str) -> Result<Self, String> {
        if let Some(expr) = raw.strip_prefix('=') {
            let compiled = CompiledExpr::compile(engine, expr)?;
            Ok(PrototypeValue::Computed(compiled))
        } else {
            raw.parse::<f64>()
                .map(PrototypeValue::Literal)
                .map_err(|_| format!("prototype value '{raw}' is neither a number nor a '='-prefixed Rhai expression"))
        }
    }

    /// Returns the literal f64 if this is a Literal, None if Computed.
    pub fn as_literal(&self) -> Option<f64> {
        match self {
            PrototypeValue::Literal(f) => Some(*f),
            PrototypeValue::Computed(_) => None,
        }
    }
}

/// Default task prototype. Literal values are inherited defaults.
pub const DEFAULT_TASK_PROTOTYPE: &[(&str, f64)] = &[
    ("default_duration_minutes", 60.0),
    ("deadline_buffer_days", 3.0),
    ("deadline_penalty", 200.0),
];

fn default_computed_props(engine: &RhaiEngine) -> Vec<(&'static str, PrototypeValue)> {
    vec![
        (
            "priority_weight",
            PrototypeValue::Computed(
                CompiledExpr::compile(
                    engine,
                    "switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0, _ => 1.0 }",
                )
                .expect("default priority_weight must compile"),
            ),
        ),
        (
            "urgency_weight",
            PrototypeValue::Computed(
                CompiledExpr::compile(
                    engine,
                    "if days_to_deadline > deadline_buffer_days { 0.0 } \
                     else if days_to_deadline <= 0.0 { deadline_penalty } \
                     else { deadline_penalty * (1.0 - days_to_deadline / deadline_buffer_days) }",
                )
                .expect("default urgency_weight must compile"),
            ),
        ),
        (
            "position_weight",
            PrototypeValue::Computed(
                CompiledExpr::compile(engine, "0.001 * (max_position - position)")
                    .expect("default position_weight must compile"),
            ),
        ),
        (
            "task_weight",
            PrototypeValue::Computed(
                CompiledExpr::compile(
                    engine,
                    "priority_weight * (1.0 + urgency_weight) + position_weight",
                )
                .expect("default task_weight must compile"),
            ),
        ),
    ]
}

/// Resolve prototypal inheritance: prototype → instance → context, then evaluate Computed expressions.
///
/// Returns all final attribute values as f64s.
pub fn resolve_prototype(
    engine: &RhaiEngine,
    prototype_props: &BTreeMap<String, PrototypeValue>,
    instance_props: &BTreeMap<String, PrototypeValue>,
    context_props: &BTreeMap<String, f64>,
) -> Result<BTreeMap<String, f64>, PetriError> {
    let mut merged: BTreeMap<String, PrototypeValue> = prototype_props.clone();
    for (k, v) in instance_props {
        merged.insert(k.clone(), v.clone());
    }

    let mut literals: BTreeMap<String, f64> = BTreeMap::new();
    let mut computed: BTreeMap<String, &CompiledExpr> = BTreeMap::new();

    for (k, v) in &merged {
        match v {
            PrototypeValue::Literal(f) => {
                literals.insert(k.clone(), *f);
            }
            PrototypeValue::Computed(compiled) => {
                computed.insert(k.clone(), compiled);
            }
        }
    }

    for (k, v) in context_props {
        literals.insert(k.clone(), *v);
    }

    let sorted = topo_sort_computed(&computed);

    let mut scope = Scope::new();
    for (k, v) in &literals {
        scope.push(k.clone(), *v);
    }

    for name in &sorted {
        let compiled = computed[name.as_str()];
        let result: Dynamic = engine
            .eval_ast_with_scope(&mut scope, &compiled.ast)
            .map_err(|e| PetriError::ComputedEval {
                name: name.clone(),
                detail: e.to_string(),
                expr: compiled.source.clone(),
            })?;
        let val = if result.is_float() {
            result.as_float().unwrap()
        } else if result.is_int() {
            result.as_int().unwrap() as f64
        } else {
            return Err(PetriError::ComputedNonNumeric {
                name: name.clone(),
                detail: format!("{result:?}"),
            });
        };
        scope.push(name.clone(), val);
        literals.insert(name.clone(), val);
    }

    Ok(literals)
}

/// Topological sort of computed properties by dependency.
/// Scans each expression for references to other computed property names.
fn topo_sort_computed(computed: &BTreeMap<String, &CompiledExpr>) -> Vec<String> {
    let computed_names: HashSet<&str> = computed.keys().map(|s| s.as_str()).collect();
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();

    for (name, compiled) in computed {
        let mut name_deps = Vec::new();
        for other in &computed_names {
            if *other != name.as_str() && holon_core::util::expr_references(&compiled.source, other)
            {
                name_deps.push(*other);
            }
        }
        deps.insert(name.as_str(), name_deps);
    }

    holon_core::util::topo_sort_kahn(&computed_names, &deps)
}

/// Build prototype properties from a block's properties.
/// Parses each property into a PrototypeValue at the boundary — panics on invalid values.
pub fn block_to_prototype_props(
    engine: &RhaiEngine,
    block: &Block,
) -> Result<BTreeMap<String, PrototypeValue>, PetriError> {
    use holon_api::Value as HValue;
    let mut props = BTreeMap::new();
    for (k, v) in &block.properties {
        if k == "prototype_for" {
            continue;
        }
        let pv = match v {
            HValue::Float(f) => PrototypeValue::Literal(*f),
            HValue::Integer(i) => PrototypeValue::Literal(*i as f64),
            HValue::String(s) => PrototypeValue::parse(engine, s).map_err(|detail| {
                PetriError::InvalidPrototypeProperty {
                    block_id: block.id.to_string(),
                    name: k.clone(),
                    detail,
                }
            })?,
            HValue::Boolean(b) => PrototypeValue::Literal(if *b { 1.0 } else { 0.0 }),
            _ => continue,
        };
        props.insert(k.clone(), pv);
    }
    Ok(props)
}

/// Build context properties for a task during materialization.
fn build_context_props(
    task: &TaskInfo,
    now: DateTime<Utc>,
    max_position: usize,
) -> BTreeMap<String, f64> {
    let mut ctx = BTreeMap::new();

    let priority = task.priority.map(|p| p.to_int() as f64).unwrap_or(0.0);
    ctx.insert("priority".to_string(), priority);

    ctx.insert("position".to_string(), task.position as f64);
    ctx.insert("max_position".to_string(), max_position as f64);

    let days_to_deadline = task
        .deadline
        .as_ref()
        .map(|ts| {
            let today = now.date_naive();
            (ts.date() - today).num_days() as f64
        })
        .unwrap_or(f64::MAX);
    ctx.insert("days_to_deadline".to_string(), days_to_deadline);

    ctx
}

/// Build the full default prototype: const literal defaults + compiled computed expressions.
pub fn default_prototype_props(engine: &RhaiEngine) -> BTreeMap<String, PrototypeValue> {
    let mut props: BTreeMap<String, PrototypeValue> = DEFAULT_TASK_PROTOTYPE
        .iter()
        .map(|(k, v)| (k.to_string(), PrototypeValue::Literal(*v)))
        .collect();
    for (k, v) in default_computed_props(engine) {
        props.insert(k.to_string(), v);
    }
    props
}

/// Describes the "self" person — from a persistent Person block or defaults.
///
/// A self block is identified by having an `is_self` property set to `true`.
#[derive(Debug)]
pub struct SelfDescriptor {
    pub mental_slots_capacity: i64,
}

const DEFAULT_MENTAL_SLOTS_CAPACITY: i64 = 7;

/// Parse a numeric property value. Org drawer properties arrive as strings,
/// so `String` is parsed; any other type is a stored-data bug — panic.
fn numeric_prop(block_id: &EntityUri, name: &str, v: &holon_api::Value) -> Result<f64, PetriError> {
    use holon_api::Value as HValue;
    match v {
        HValue::Float(f) => Ok(*f),
        HValue::Integer(i) => Ok(*i as f64),
        HValue::String(s) => s.parse().map_err(|e| PetriError::NonNumericProperty {
            block_id: block_id.to_string(),
            name: name.to_string(),
            detail: format!("{s:?}: {e}"),
        }),
        other => Err(PetriError::NonNumericProperty {
            block_id: block_id.to_string(),
            name: name.to_string(),
            detail: format!("non-numeric type {other:?}"),
        }),
    }
}

/// Parse an integer property value; fails loud on fractional or non-numeric values.
fn integer_prop(block_id: &EntityUri, name: &str, v: &holon_api::Value) -> Result<i64, PetriError> {
    let f = numeric_prop(block_id, name, v)?;
    if f.fract() != 0.0 {
        return Err(PetriError::NonIntegerProperty {
            block_id: block_id.to_string(),
            name: name.to_string(),
            value: f,
        });
    }
    Ok(f as i64)
}

impl SelfDescriptor {
    pub fn from_block(block: &Block) -> Result<Self, PetriError> {
        let props = &block.properties;

        let mental_slots_capacity = match props.get("mental_slots_capacity") {
            Some(v) => integer_prop(&block.id, "mental_slots_capacity", v)?,
            None => DEFAULT_MENTAL_SLOTS_CAPACITY,
        };

        Ok(Self {
            mental_slots_capacity,
        })
    }

    pub fn defaults() -> Self {
        Self {
            mental_slots_capacity: DEFAULT_MENTAL_SLOTS_CAPACITY,
        }
    }

    /// Returns true if `block` is a self block (has `is_self` property set to true).
    pub fn is_self_block(block: &Block) -> bool {
        block
            .properties
            .get("is_self")
            .map(|v| matches!(v, holon_api::Value::Boolean(true)))
            .unwrap_or(false)
    }
}

/// Returns true if `block` is a prototype block (has `prototype_for` property).
pub fn is_prototype_block(block: &Block) -> bool {
    block.properties.contains_key("prototype_for")
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Executor {
    SelfExec,
    Delegated { person: String },
}

/// Parse content prefixes in order: `>`, `@[[Person]]:`, `?`.
/// Returns (cleaned_content, has_sequential_dep, executor, is_question).
pub fn parse_content_prefixes(raw: &str) -> (String, bool, Executor, bool) {
    let mut content = raw.trim().to_string();
    let mut has_sequential_dep = false;
    let mut executor = Executor::SelfExec;
    let mut is_question = false;

    if content.starts_with('>') {
        has_sequential_dep = true;
        content = content[1..].trim_start().to_string();
    }

    if content.starts_with("@[[")
        && let Some(bracket_end) = content.find("]]")
    {
        let person = content[3..bracket_end].to_string();
        let after_bracket = &content[bracket_end + 2..];
        if let Some(rest) = after_bracket.strip_prefix(':') {
            executor = Executor::Delegated { person };
            content = rest.trim_start().to_string();
        }
    }

    if content.starts_with('?') {
        is_question = true;
        content = content[1..].trim_start().to_string();
    }

    (content, has_sequential_dep, executor, is_question)
}

/// Extract `[[wiki links]]` from text content. Handles `[[target][display]]` syntax.
pub fn extract_wiki_links(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut remaining = content;
    while let Some(start) = remaining.find("[[") {
        remaining = &remaining[start + 2..];
        if let Some(end) = remaining.find("]]") {
            let link = remaining[..end].to_string();
            let target = link.split("][").next().unwrap_or(&link).to_string();
            if !target.is_empty() {
                links.push(target);
            }
            remaining = &remaining[end + 2..];
        } else {
            break;
        }
    }
    links
}

// ---------------------------------------------------------------------------
// TaskInfo
// ---------------------------------------------------------------------------

struct TaskInfo {
    block_id: String,
    parent_id: String,
    content: String,
    priority: Option<Priority>,
    deadline: Option<Timestamp>,
    depends_on: DependsOn,
    duration_minutes: Option<i64>,
    is_completed: bool,
    position: usize,
    has_sequential_dep: bool,
    executor: Executor,
    is_question: bool,
}

impl TaskInfo {
    fn from_block(block: &Block, position: usize) -> Result<Option<Self>, PetriError> {
        use holon_api::Value as HValue;
        let props = &block.properties;

        let task_state = match props.get("task_state") {
            Some(HValue::String(s)) => Some(TaskState::from_keyword(s)),
            Some(other) => {
                return Err(PetriError::UnexpectedPropertyType {
                    block_id: block.id.to_string(),
                    field: "task_state".to_string(),
                    detail: format!("{other:?}"),
                });
            }
            None => None,
        };

        // Not a task (no task_state) — not an error, just skipped.
        let Some(task_state) = task_state else {
            return Ok(None);
        };

        let priority = match props.get("priority") {
            Some(v) => {
                let i = integer_prop(&block.id, "priority", v)?;
                Some(
                    Priority::from_int(i as i32).map_err(|e| PetriError::InvalidPriority {
                        block_id: block.id.to_string(),
                        value: i,
                        detail: e.to_string(),
                    })?,
                )
            }
            None => None,
        };

        let deadline = match props.get("deadline") {
            Some(HValue::String(s)) => {
                Some(
                    Timestamp::parse(s).map_err(|e| PetriError::InvalidDeadline {
                        block_id: block.id.to_string(),
                        value: s.clone(),
                        detail: e.to_string(),
                    })?,
                )
            }
            Some(other) => {
                return Err(PetriError::UnexpectedPropertyType {
                    block_id: block.id.to_string(),
                    field: "deadline".to_string(),
                    detail: format!("{other:?}"),
                });
            }
            None => None,
        };

        let depends_on = match props.get("depends_on") {
            Some(HValue::String(s)) => DependsOn::from_csv(s),
            Some(other) => {
                return Err(PetriError::UnexpectedPropertyType {
                    block_id: block.id.to_string(),
                    field: "depends_on".to_string(),
                    detail: format!("{other:?}"),
                });
            }
            None => DependsOn::default(),
        };

        let duration_minutes = match props.get("duration") {
            Some(v) => {
                let d = integer_prop(&block.id, "duration", v)?;
                if d <= 0 || d > MAX_DURATION_MINUTES {
                    return Err(PetriError::DurationOutOfRange {
                        block_id: block.id.to_string(),
                        value: d,
                        max: MAX_DURATION_MINUTES,
                    });
                }
                Some(d)
            }
            None => None,
        };

        let is_completed = task_state.is_done();

        let (content, has_sequential_dep, executor, is_question) =
            parse_content_prefixes(&block.content);

        Ok(Some(TaskInfo {
            block_id: block.id.to_string(),
            parent_id: block.parent_id.to_string(),
            content,
            priority,
            deadline,
            depends_on,
            duration_minutes,
            is_completed,
            position,
            has_sequential_dep,
            executor,
            is_question,
        }))
    }

    fn wiki_links(&self) -> Vec<String> {
        extract_wiki_links(&self.content)
    }
}

// ---------------------------------------------------------------------------
// Materialization
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct MentalSlotsInfo {
    pub occupied: usize,
    pub capacity: usize,
}

#[derive(Debug)]
pub struct RankResult {
    pub ranked: Vec<RankedTask>,
    pub mental_slots: MentalSlotsInfo,
}

#[derive(Debug)]
pub struct RankedTask {
    pub block_id: String,
    pub label: String,
    pub delta_obj: f64,
    pub delta_per_minute: f64,
    pub duration_minutes: f64,
}

/// Materialize a list of blocks into a Petri Net and initial marking.
pub fn materialize(
    blocks: &[Block],
    self_desc: &SelfDescriptor,
    prototype_props: &BTreeMap<String, PrototypeValue>,
) -> Result<(TaskNet, TaskMarking), PetriError> {
    materialize_at(
        blocks,
        self_desc,
        prototype_props,
        DateTime::from_timestamp_millis(holon_api::clock::now_millis()).expect("now within range"),
    )
}

/// Like `materialize` but with an explicit `now` for testability.
pub fn materialize_at(
    blocks: &[Block],
    self_desc: &SelfDescriptor,
    prototype_props: &BTreeMap<String, PrototypeValue>,
    now: DateTime<Utc>,
) -> Result<(TaskNet, TaskMarking), PetriError> {
    let rhai_engine = holon_expr::bounded_engine();

    let mut tasks: Vec<TaskInfo> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        if let Some(t) = TaskInfo::from_block(b, i)? {
            tasks.push(t);
        }
    }

    resolve_sequential_deps(&mut tasks);

    let mut frag_to_id: BTreeMap<String, &str> = BTreeMap::new();
    for task in &tasks {
        let frag = rhai_ident_fragment(&task.block_id);
        if let Some(prev) = frag_to_id.insert(frag.clone(), &task.block_id)
            && prev != task.block_id
        {
            return Err(PetriError::FragmentCollision {
                a: prev.to_string(),
                b: task.block_id.clone(),
                frag,
            });
        }
    }

    let active: Vec<&TaskInfo> = tasks.iter().filter(|t| !t.is_completed).collect();
    let completed: Vec<&TaskInfo> = tasks.iter().filter(|t| t.is_completed).collect();

    let default_duration = prototype_props
        .get("default_duration_minutes")
        .and_then(|v| v.as_literal())
        .unwrap_or(60.0);
    if !default_duration.is_finite()
        || default_duration <= 0.0
        || default_duration > MAX_DURATION_MINUTES as f64
    {
        return Err(PetriError::InvalidPrototypeProperty {
            block_id: "<prototype>".to_string(),
            name: "default_duration_minutes".to_string(),
            detail: format!(
                "{default_duration} is out of range (expected finite, 1..={MAX_DURATION_MINUTES} minutes)"
            ),
        });
    }

    let mut tokens = vec![build_self_token(self_desc)];
    tokens.extend(build_completion_tokens(&completed));
    tokens.extend(build_entity_tokens(&active));
    tokens.extend(build_delegate_tokens(&active));

    let max_position = active.len();

    let mut task_weights: BTreeMap<String, f64> = BTreeMap::new();
    for task in &active {
        let instance_props = task_to_instance_props_from_info(task);
        let context = build_context_props(task, now, max_position);
        let resolved = resolve_prototype(&rhai_engine, prototype_props, &instance_props, &context)?;
        let weight = resolved.get("task_weight").copied().unwrap_or(1.0);
        task_weights.insert(task.block_id.clone(), weight);
    }

    let transitions: Vec<TaskTransition> = active
        .iter()
        .flat_map(|t| build_task_transitions(t, default_duration, &task_weights))
        .collect();

    let objective_expr_src = build_objective_expr(&active, &task_weights);
    let objective_expr = CompiledExpr::compile(&rhai_engine, &objective_expr_src)
        .map_err(|detail| PetriError::ObjectiveCompile { detail })?;

    let net = TaskNet {
        transitions,
        objective_expr,
    };
    let marking = TaskMarking { clock: now, tokens };

    Ok((net, marking))
}

fn task_to_instance_props_from_info(task: &TaskInfo) -> BTreeMap<String, PrototypeValue> {
    let mut props = BTreeMap::new();
    if let Some(p) = task.priority {
        props.insert(
            "priority".to_string(),
            PrototypeValue::Literal(p.to_int() as f64),
        );
    }
    if let Some(dur) = task.duration_minutes {
        props.insert("duration".to_string(), PrototypeValue::Literal(dur as f64));
    }
    props
}

/// Resolve `>` sequential dependencies: within each sibling group (same parent_id),
/// a task with `has_sequential_dep` gets a dependency on the previous sibling.
fn resolve_sequential_deps(tasks: &mut [TaskInfo]) {
    let mut sibling_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, task) in tasks.iter().enumerate() {
        sibling_groups
            .entry(task.parent_id.clone())
            .or_default()
            .push(idx);
    }
    for group in sibling_groups.values() {
        let mut sorted = group.clone();
        sorted.sort_by_key(|&idx| tasks[idx].position);
        let mut prev_sibling_id: Option<String> = None;
        for &idx in &sorted {
            if tasks[idx].has_sequential_dep
                && let Some(ref prev_id) = prev_sibling_id
                && !tasks[idx].depends_on.contains(prev_id)
            {
                tasks[idx].depends_on.push(prev_id.clone());
            }
            prev_sibling_id = Some(tasks[idx].block_id.clone());
        }
    }
}

fn build_self_token(_self_desc: &SelfDescriptor) -> TaskToken {
    TaskToken {
        id: "self".to_string(),
        token_type: "person".to_string(),
        attributes: {
            let mut a = BTreeMap::new();
            a.insert("status".to_string(), Value::String("active".to_string()));
            a
        },
    }
}

/// Deterministically map a block id to a valid Rhai identifier fragment.
///
/// Real block ids are EntityUris like `block:9f8e-…` whose `:` and `-` are not
/// valid in Rhai identifiers; token ids built from them become Rhai scope
/// variables referenced by the objective expression, so they must be
/// identifier-safe. `materialize_at` asserts the mapping stays injective.
pub fn rhai_ident_fragment(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Encode an arbitrary string as a Rhai string literal (surrounding quotes plus
/// escaping), so user-derived text embedded in a generated Rhai expression can
/// never break out of the literal or inject code. Used where a value must be
/// referenced inside a compiled expression (e.g. the objective) rather than
/// carried as a typed token attribute.
fn rhai_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn build_completion_tokens(completed: &[&TaskInfo]) -> Vec<TaskToken> {
    completed
        .iter()
        .map(|t| TaskToken {
            id: format!("completed_{}", rhai_ident_fragment(&t.block_id)),
            token_type: "completion".to_string(),
            attributes: {
                let mut a = BTreeMap::new();
                a.insert("source_task".to_string(), Value::String(t.block_id.clone()));
                a
            },
        })
        .collect()
}

fn build_entity_tokens(active: &[&TaskInfo]) -> Vec<TaskToken> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    for task in active {
        for link in task.wiki_links() {
            if seen.insert(link.clone()) {
                let entity_type = if link.starts_with("People/") {
                    "person"
                } else {
                    "document"
                };
                tokens.push(TaskToken {
                    id: link,
                    token_type: entity_type.to_string(),
                    attributes: {
                        let mut a = BTreeMap::new();
                        a.insert("status".to_string(), Value::String("active".to_string()));
                        a
                    },
                });
            }
        }
    }
    tokens
}

fn build_delegate_tokens(active: &[&TaskInfo]) -> Vec<TaskToken> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    for task in active {
        if let Executor::Delegated { ref person } = task.executor
            && seen.insert(person.clone())
        {
            tokens.push(TaskToken {
                id: format!("person_{person}"),
                token_type: "person".to_string(),
                attributes: {
                    let mut a = BTreeMap::new();
                    a.insert("status".to_string(), Value::String("active".to_string()));
                    a.insert("name".to_string(), Value::String(person.clone()));
                    a
                },
            });
        }
    }
    tokens
}

/// Build transitions for a single task. Returns 1 transition for self-executed tasks,
/// 2 for delegated tasks (delegate sub-transition + main transition).
fn build_task_transitions(
    task: &TaskInfo,
    default_duration: f64,
    task_weights: &BTreeMap<String, f64>,
) -> Vec<TaskTransition> {
    let mut transitions = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut creates = Vec::new();

    match &task.executor {
        Executor::SelfExec => {
            inputs.push(InputArc {
                bind: "self".to_string(),
                token_type: "person".to_string(),
                precond: BTreeMap::new(),
                consume: false,
            });
            outputs.push(OutputArc {
                from: "self".to_string(),
                postcond: BTreeMap::new(),
            });
        }
        Executor::Delegated { person } => {
            transitions.push(TaskTransition {
                id: format!("{}_delegate", task.block_id),
                source_block_id: task.block_id.clone(),
                label: format!("Delegate to {person}"),
                inputs: vec![InputArc {
                    bind: "self".to_string(),
                    token_type: "person".to_string(),
                    precond: BTreeMap::new(),
                    consume: false,
                }],
                outputs: vec![OutputArc {
                    from: "self".to_string(),
                    postcond: BTreeMap::new(),
                }],
                creates: vec![CreateArc {
                    id_expr: format!("\"waiting_for_{}\"", rhai_ident_fragment(&task.block_id))
                        .parse()
                        .expect("waiting_for id_expr is a Rhai string literal and must compile"),
                    token_type: "waiting".to_string(),
                    attrs: {
                        let mut a = BTreeMap::new();
                        a.insert(
                            "source_task".to_string(),
                            AttrInit::Literal(Value::String(task.block_id.clone())),
                        );
                        a.insert(
                            "delegate".to_string(),
                            AttrInit::Literal(Value::String(person.clone())),
                        );
                        a
                    },
                }],
                duration: 0.0,
            });

            let person_bind = format!("delegate_{}", person.replace(' ', "_"));
            let mut pcond = BTreeMap::new();
            pcond.insert("name".to_string(), PrecondSpec::Exact(person.clone()));
            inputs.push(InputArc {
                bind: person_bind.clone(),
                token_type: "person".to_string(),
                precond: pcond,
                consume: false,
            });
            outputs.push(OutputArc {
                from: person_bind,
                postcond: BTreeMap::new(),
            });

            let wait_bind = format!("wait_{}", task.block_id);
            let mut wcond = BTreeMap::new();
            wcond.insert(
                "source_task".to_string(),
                PrecondSpec::Exact(task.block_id.clone()),
            );
            inputs.push(InputArc {
                bind: wait_bind,
                token_type: "waiting".to_string(),
                precond: wcond,
                consume: true,
            });
        }
    }

    for link in task.wiki_links() {
        let entity_type = if link.starts_with("People/") {
            "person"
        } else {
            "document"
        };
        let bind_name = link.replace(['/', ' '], "_");
        inputs.push(InputArc {
            bind: bind_name.clone(),
            token_type: entity_type.to_string(),
            precond: BTreeMap::new(),
            consume: false,
        });
        outputs.push(OutputArc {
            from: bind_name,
            postcond: BTreeMap::new(),
        });
    }

    for (i, dep_id) in task.depends_on.iter().enumerate() {
        let bind_name = format!("dep_{i}");
        let mut precond = BTreeMap::new();
        precond.insert(
            "source_task".to_string(),
            PrecondSpec::Exact(dep_id.clone()),
        );
        inputs.push(InputArc {
            bind: bind_name.clone(),
            token_type: "completion".to_string(),
            precond,
            consume: false,
        });
        outputs.push(OutputArc {
            from: bind_name,
            postcond: BTreeMap::new(),
        });
    }

    let weight = task_weights.get(&task.block_id).copied().unwrap_or(1.0);

    creates.push(CreateArc {
        id_expr: format!("\"completed_{}\"", rhai_ident_fragment(&task.block_id))
            .parse()
            .expect("completed id_expr is a Rhai string literal and must compile"),
        token_type: "completion".to_string(),
        attrs: {
            let mut a = BTreeMap::new();
            a.insert(
                "source_task".to_string(),
                AttrInit::Literal(Value::String(task.block_id.clone())),
            );
            a.insert(
                "task_weight".to_string(),
                AttrInit::Literal(Value::Float(weight)),
            );
            a
        },
    });

    if task.is_question {
        creates.push(CreateArc {
            id_expr: format!("\"knowledge_{}\"", rhai_ident_fragment(&task.block_id))
                .parse()
                .expect("knowledge id_expr is a Rhai string literal and must compile"),
            token_type: "knowledge".to_string(),
            attrs: {
                let mut a = BTreeMap::new();
                a.insert(
                    "source_task".to_string(),
                    AttrInit::Literal(Value::String(task.block_id.clone())),
                );
                a.insert(
                    "confidence".to_string(),
                    AttrInit::Literal(Value::Float(0.8)),
                );
                a
            },
        });
    }

    let duration = task
        .duration_minutes
        .map(|m| m as f64)
        .unwrap_or(default_duration);

    transitions.push(TaskTransition {
        id: task.block_id.clone(),
        source_block_id: task.block_id.clone(),
        label: task.content.lines().next().unwrap_or("").to_string(),
        inputs,
        outputs,
        creates,
        duration,
    });

    transitions
}

fn build_objective_expr(tasks: &[&TaskInfo], task_weights: &BTreeMap<String, f64>) -> String {
    if tasks.is_empty() {
        return "0.0".to_string();
    }

    let parts: Vec<String> = tasks
        .iter()
        .map(|task| {
            let weight = task_weights.get(&task.block_id).copied().unwrap_or(1.0);
            format!(
                "(if is_def_var(\"completed_{frag}\") && completed_{frag}.source_task == {bid} {{ {weight:.6} }} else {{ 0.0 }})",
                frag = rhai_ident_fragment(&task.block_id),
                bid = rhai_string_literal(&task.block_id)
            )
        })
        .collect();

    parts.join(" + ")
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// Rank tasks using the engine's WSJF algorithm.
///
/// Scans `blocks` for special blocks:
/// - A block with `prototype_for` → used as prototype properties
/// - A block with `is_self: true` → used as the self token source
/// - All other blocks with `task_state` → treated as tasks
pub fn rank_tasks(blocks: &[Block]) -> Result<RankResult, String> {
    let rhai_engine = holon_expr::bounded_engine();

    let prototype_block = blocks.iter().find(|b| is_prototype_block(b));
    let self_block = blocks.iter().find(|b| SelfDescriptor::is_self_block(b));

    let mut prototype_props = default_prototype_props(&rhai_engine);
    if let Some(pb) = prototype_block {
        let overrides = block_to_prototype_props(&rhai_engine, pb).map_err(|e| e.to_string())?;
        for (k, v) in overrides {
            prototype_props.insert(k, v);
        }
    }

    let self_desc = match self_block {
        Some(b) => SelfDescriptor::from_block(b).map_err(|e| e.to_string())?,
        None => SelfDescriptor::defaults(),
    };

    let (net, marking) =
        materialize(blocks, &self_desc, &prototype_props).map_err(|e| e.to_string())?;

    let engine = Engine::new();
    let enabled = engine.enabled(&net, &marking)?;
    let ranked = engine.rank(&net, &marking, &enabled)?;

    let ranked_tasks = ranked
        .into_iter()
        .map(|rt| {
            let transition = net.transition(&rt.binding.transition_id).unwrap();
            RankedTask {
                block_id: transition.source_block_id.clone(),
                label: transition.label.clone(),
                delta_obj: rt.delta_obj,
                delta_per_minute: rt.delta_per_minute,
                duration_minutes: transition.duration,
            }
        })
        .collect();

    let occupied = blocks
        .iter()
        .filter(|b| {
            b.properties
                .get("task_state")
                .and_then(|v| match v {
                    holon_api::Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .map(|s| s == "DOING")
                .unwrap_or(false)
        })
        .count();

    Ok(RankResult {
        ranked: ranked_tasks,
        mental_slots: MentalSlotsInfo {
            occupied,
            capacity: self_desc.mental_slots_capacity as usize,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_block(id: &EntityUri, content: &str, state: &str) -> Block {
        let mut b = Block::new_text(id.clone(), EntityUri::block("parent-1"), content);
        b.set_property("task_state", holon_api::Value::String(state.to_string()));
        b
    }

    /// Real block ids are EntityUris (`block:<uuid>`) whose `:`/`-` are invalid
    /// in Rhai identifiers — rank_tasks must still compile the objective and
    /// return real block ids, never delegate sub-transition ids.
    #[test]
    fn rank_tasks_with_real_entity_uri_ids() {
        let self_task = EntityUri::block_random();
        let delegated_task = EntityUri::block_random();
        let done_task = EntityUri::block_random();

        let blocks = vec![
            task_block(&self_task, "Write the report", "TODO"),
            task_block(&delegated_task, "@[[Alice]]: review doc", "TODO"),
            task_block(&done_task, "Old chore", "DONE"),
        ];

        let result = rank_tasks(&blocks).expect("rank_tasks must succeed on real ids");

        assert!(!result.ranked.is_empty(), "active tasks must be ranked");
        let real_ids = [self_task.to_string(), delegated_task.to_string()];
        for rt in &result.ranked {
            assert!(
                real_ids.contains(&rt.block_id),
                "ranked block_id {:?} is not a real block id",
                rt.block_id
            );
        }
        assert!(
            result.ranked.iter().any(|rt| rt.block_id == real_ids[1]),
            "delegated task must surface under its own block id"
        );
    }

    /// Pins the exact WSJF `rank()` output so the load-time compilation of
    /// postconditions / create-arc id-exprs (`PostcondExpr`) cannot silently
    /// change ranking semantics. Two self-executed TODO tasks, distinct
    /// priorities, no deadline: higher priority must rank first and each
    /// Δobjective must equal its completion-token weight
    /// (`priority_weight * (1 + urgency=0) + position_weight`), with
    /// Δper-minute = Δobjective / 60m default duration.
    #[test]
    fn rank_output_is_pinned_for_priority_ordering() {
        let high = EntityUri::block_random();
        let low = EntityUri::block_random();

        let mut hb = task_block(&high, "High priority task", "TODO");
        hb.set_property("priority", holon_api::Value::Integer(3));
        let mut lb = task_block(&low, "Low priority task", "TODO");
        lb.set_property("priority", holon_api::Value::Integer(1));

        let result = rank_tasks(&[hb, lb]).expect("rank_tasks must succeed");

        assert_eq!(result.ranked.len(), 2, "both active tasks must be ranked");
        assert_eq!(
            result.ranked[0].block_id,
            high.to_string(),
            "priority 3 must rank first"
        );
        assert_eq!(
            result.ranked[1].block_id,
            low.to_string(),
            "priority 1 must rank second"
        );

        // max_position = 2 active tasks; positions are block indices 0 and 1.
        let expect_high = 100.0 + 0.001 * (2.0 - 0.0);
        let expect_low = 15.0 + 0.001 * (2.0 - 1.0);
        let eps = 1e-6;
        assert!(
            (result.ranked[0].delta_obj - expect_high).abs() < eps,
            "high Δobj = {}, want {expect_high}",
            result.ranked[0].delta_obj
        );
        assert!(
            (result.ranked[1].delta_obj - expect_low).abs() < eps,
            "low Δobj = {}, want {expect_low}",
            result.ranked[1].delta_obj
        );
        assert!(
            (result.ranked[0].delta_per_minute - expect_high / 60.0).abs() < eps,
            "high Δ/min = {}",
            result.ranked[0].delta_per_minute
        );
        assert!(
            (result.ranked[1].delta_per_minute - expect_low / 60.0).abs() < eps,
            "low Δ/min = {}",
            result.ranked[1].delta_per_minute
        );
    }

    /// Org drawer properties arrive as strings — they must be parsed, and
    /// garbage must fail loud rather than silently defaulting.
    #[test]
    fn self_descriptor_parses_string_properties() {
        let mut b = Block::new_text(EntityUri::block_random(), EntityUri::block("p"), "Self");
        b.set_property("is_self", holon_api::Value::Boolean(true));
        b.set_property(
            "mental_slots_capacity",
            holon_api::Value::String("5".to_string()),
        );

        let desc = SelfDescriptor::from_block(&b).expect("valid self block parses");
        assert_eq!(desc.mental_slots_capacity, 5);
    }

    #[test]
    fn self_descriptor_errors_on_garbage_mental_slots_capacity() {
        let mut b = Block::new_text(EntityUri::block_random(), EntityUri::block("p"), "Self");
        b.set_property(
            "mental_slots_capacity",
            holon_api::Value::String("lots".to_string()),
        );
        let err = SelfDescriptor::from_block(&b)
            .expect_err("garbage mental_slots_capacity must fail loud, not silently default");
        assert!(
            err.to_string().contains("is not numeric"),
            "error must name the failure, got: {err}"
        );
    }

    /// A delegate name containing `"` and `\\` must be carried as typed token
    /// data (`AttrInit::Literal`), never spliced into Rhai source. Before the
    /// injection fix this produced invalid Rhai (`"Al"ice\\Bob"`) that failed
    /// at fire time, so `rank_tasks` returned `Err`.
    #[test]
    fn rank_tasks_tolerates_quotes_and_backslashes_in_names() {
        let self_task = EntityUri::block_random();
        let delegated = EntityUri::block_random();
        let blocks = vec![
            task_block(&self_task, "Write the report", "TODO"),
            task_block(&delegated, "@[[Al\"ice\\Bob]]: review doc", "TODO"),
        ];
        let result =
            rank_tasks(&blocks).expect("names with quotes/backslashes must not break ranking");
        assert!(!result.ranked.is_empty(), "active tasks must be ranked");
    }

    /// F3.1 regression: `duration: 200000000000000` used to reach
    /// `chrono::Duration::minutes` in `Engine::fire` and PANIC past the
    /// PetriError boundary, aborting the live `rank_tasks` MCP tool. It must
    /// be rejected at the parse boundary with a `PetriError`, not a panic.
    #[test]
    fn rank_tasks_errors_on_overflowing_duration() {
        let t = EntityUri::block_random();
        let mut b = task_block(&t, "Huge task", "TODO");
        b.set_property("duration", holon_api::Value::Integer(200_000_000_000_000));
        let err = rank_tasks(&[b]).expect_err("overflowing duration must be an Err, not a panic");
        assert!(
            err.contains("out of range"),
            "error must name the range violation, got: {err}"
        );
    }

    /// F3.2 regression: a stored `task_weight: "= while true {}"` used to
    /// hang `rank_tasks` forever (unbounded Rhai engine). The bounded engine
    /// must abort the eval with an error naming the operations limit.
    #[test]
    fn rank_tasks_errors_on_infinite_loop_task_weight() {
        let t = EntityUri::block_random();
        let b = task_block(&t, "Normal task", "TODO");
        let mut proto = Block::new_text(
            EntityUri::block_random(),
            EntityUri::block("p"),
            "Prototype",
        );
        proto.set_property(
            "prototype_for",
            holon_api::Value::String("task".to_string()),
        );
        proto.set_property(
            "task_weight",
            holon_api::Value::String("= while true {}".to_string()),
        );
        let err =
            rank_tasks(&[b, proto]).expect_err("unbounded Rhai loop must abort with Err, not hang");
        assert!(
            err.to_lowercase().contains("operations"),
            "error must name the operations limit, got: {err}"
        );
    }
}
