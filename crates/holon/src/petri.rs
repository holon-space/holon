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
use holon_api::CompiledExpr;
use holon_api::block::Block;
use holon_api::types::{DependsOn, Priority, TaskState, Timestamp};
use holon_engine::arc::{CreateArc, InputArc, OutputArc};
use holon_engine::value::Value;
use holon_engine::{Marking, NetDef, TokenState, TransitionDef};
use rhai::{Dynamic, Engine as RhaiEngine, Scope};
use std::collections::{BTreeMap, HashMap, HashSet};

pub use holon_engine::engine::{Engine, RankedTransition};

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
    pub constraints: Vec<CompiledExpr>,
    pub discount_rate: f64,
}

impl PartialEq for TaskNet {
    fn eq(&self, other: &Self) -> bool {
        self.transitions == other.transitions
            && self.objective_expr == other.objective_expr
            && self.constraints == other.constraints
            && (self.discount_rate - other.discount_rate).abs() < f64::EPSILON
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

    fn constraints(&self) -> &[CompiledExpr] {
        &self.constraints
    }

    fn discount_rate(&self) -> f64 {
        self.discount_rate
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
    ("discount_rate", 0.05),
    ("deadline_buffer_days", 3.0),
    ("deadline_penalty", 200.0),
    ("mental_slots_capacity", 7.0),
    ("default_energy", 1.0),
    ("default_focus", 0.8),
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
) -> BTreeMap<String, f64> {
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
            .unwrap_or_else(|e| {
                panic!(
                    "Rhai eval error for '{name}': {e}\n  expr: {}",
                    compiled.source
                )
            });
        let val = if result.is_float() {
            result.as_float().unwrap()
        } else if result.is_int() {
            result.as_int().unwrap() as f64
        } else {
            panic!("Rhai expression '{name}' returned non-numeric: {result:?}");
        };
        scope.push(name.clone(), val);
        literals.insert(name.clone(), val);
    }

    literals
}

/// Topological sort of computed properties by dependency.
/// Scans each expression for references to other computed property names.
fn topo_sort_computed(computed: &BTreeMap<String, &CompiledExpr>) -> Vec<String> {
    let computed_names: HashSet<&str> = computed.keys().map(|s| s.as_str()).collect();
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();

    for (name, compiled) in computed {
        let mut name_deps = Vec::new();
        for other in &computed_names {
            if *other != name.as_str() && crate::util::expr_references(&compiled.source, other) {
                name_deps.push(*other);
            }
        }
        deps.insert(name.as_str(), name_deps);
    }

    crate::util::topo_sort_kahn(&computed_names, &deps)
}

/// Build prototype properties from a block's properties.
/// Parses each property into a PrototypeValue at the boundary — panics on invalid values.
pub fn block_to_prototype_props(
    engine: &RhaiEngine,
    block: &Block,
) -> BTreeMap<String, PrototypeValue> {
    use holon_api::Value as HValue;
    let mut props = BTreeMap::new();
    for (k, v) in &block.properties {
        if k == "prototype_for" {
            continue;
        }
        let pv = match v {
            HValue::Float(f) => PrototypeValue::Literal(*f),
            HValue::Integer(i) => PrototypeValue::Literal(*i as f64),
            HValue::String(s) => PrototypeValue::parse(engine, s)
                .unwrap_or_else(|e| panic!("invalid prototype property '{k}': {e}")),
            HValue::Boolean(b) => PrototypeValue::Literal(if *b { 1.0 } else { 0.0 }),
            _ => continue,
        };
        props.insert(k.clone(), pv);
    }
    props
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
pub struct SelfDescriptor {
    pub energy: f64,
    pub focus: f64,
    pub mental_slots_capacity: i64,
}

const DEFAULT_ENERGY: f64 = 1.0;
const DEFAULT_FOCUS: f64 = 0.8;
const DEFAULT_MENTAL_SLOTS_CAPACITY: i64 = 7;

impl SelfDescriptor {
    pub fn from_block(block: &Block) -> Self {
        use holon_api::Value as HValue;
        let props = &block.properties;

        let energy = props
            .get("energy")
            .map(|v| match v {
                HValue::Float(f) => *f,
                HValue::Integer(i) => *i as f64,
                _ => DEFAULT_ENERGY,
            })
            .unwrap_or(DEFAULT_ENERGY);

        let focus = props
            .get("focus")
            .map(|v| match v {
                HValue::Float(f) => *f,
                HValue::Integer(i) => *i as f64,
                _ => DEFAULT_FOCUS,
            })
            .unwrap_or(DEFAULT_FOCUS);

        let mental_slots_capacity = props
            .get("mental_slots_capacity")
            .map(|v| match v {
                HValue::Integer(i) => *i,
                _ => DEFAULT_MENTAL_SLOTS_CAPACITY,
            })
            .unwrap_or(DEFAULT_MENTAL_SLOTS_CAPACITY);

        Self {
            energy,
            focus,
            mental_slots_capacity,
        }
    }

    pub fn defaults() -> Self {
        Self {
            energy: DEFAULT_ENERGY,
            focus: DEFAULT_FOCUS,
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

    if content.starts_with("@[[") {
        if let Some(bracket_end) = content.find("]]") {
            let person = content[3..bracket_end].to_string();
            let after_bracket = &content[bracket_end + 2..];
            if after_bracket.starts_with(':') {
                executor = Executor::Delegated { person };
                content = after_bracket[1..].trim_start().to_string();
            }
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
    task_state: Option<TaskState>,
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
    fn from_block(block: &Block, position: usize) -> Option<Self> {
        use holon_api::Value as HValue;
        let props = &block.properties;

        let task_state = props.get("task_state").and_then(|v| match v {
            HValue::String(s) => Some(TaskState::from_keyword(s)),
            _ => None,
        });

        task_state.as_ref()?;

        let priority = props.get("priority").and_then(|v| match v {
            HValue::Integer(i) => Some(Priority::from_int(*i as i32).unwrap_or_else(|e| {
                panic!("stored priority {i} on block {} is invalid: {e}", block.id)
            })),
            _ => None,
        });

        let deadline = props.get("deadline").and_then(|v| match v {
            HValue::String(s) => Some(Timestamp::parse(s).unwrap_or_else(|e| {
                panic!(
                    "stored deadline property {s:?} on block {} is not a valid timestamp: {e}",
                    block.id
                )
            })),
            _ => None,
        });

        let depends_on = props
            .get("depends_on")
            .and_then(|v| match v {
                HValue::String(s) => Some(DependsOn::from_csv(s)),
                _ => None,
            })
            .unwrap_or_default();

        let duration_minutes = props.get("duration").and_then(|v| match v {
            HValue::Integer(i) => Some(*i),
            _ => None,
        });

        let is_completed = task_state.as_ref().map(|ts| ts.is_done()).unwrap_or(false);

        let (content, has_sequential_dep, executor, is_question) =
            parse_content_prefixes(&block.content);

        Some(TaskInfo {
            block_id: block.id.to_string(),
            parent_id: block.parent_id.to_string(),
            content,
            task_state,
            priority,
            deadline,
            depends_on,
            duration_minutes,
            is_completed,
            position,
            has_sequential_dep,
            executor,
            is_question,
        })
    }

    fn wiki_links(&self) -> Vec<String> {
        extract_wiki_links(&self.content)
    }
}

// ---------------------------------------------------------------------------
// Materialization
// ---------------------------------------------------------------------------

pub struct MentalSlotsInfo {
    pub occupied: usize,
    pub capacity: usize,
}

pub struct RankResult {
    pub ranked: Vec<RankedTask>,
    pub mental_slots: MentalSlotsInfo,
}

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
) -> (TaskNet, TaskMarking) {
    materialize_at(blocks, self_desc, prototype_props, Utc::now())
}

/// Like `materialize` but with an explicit `now` for testability.
pub fn materialize_at(
    blocks: &[Block],
    self_desc: &SelfDescriptor,
    prototype_props: &BTreeMap<String, PrototypeValue>,
    now: DateTime<Utc>,
) -> (TaskNet, TaskMarking) {
    let rhai_engine = RhaiEngine::new();

    let mut tasks: Vec<TaskInfo> = blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| TaskInfo::from_block(b, i))
        .collect();

    resolve_sequential_deps(&mut tasks);

    let active: Vec<&TaskInfo> = tasks.iter().filter(|t| !t.is_completed).collect();
    let completed: Vec<&TaskInfo> = tasks.iter().filter(|t| t.is_completed).collect();

    let default_duration = prototype_props
        .get("default_duration_minutes")
        .and_then(|v| v.as_literal())
        .unwrap_or(60.0);

    let discount_rate = prototype_props
        .get("discount_rate")
        .and_then(|v| v.as_literal())
        .unwrap_or(0.05);

    let mut tokens = vec![build_self_token(&active, self_desc)];
    tokens.extend(build_completion_tokens(&completed));
    tokens.extend(build_entity_tokens(&active));
    tokens.extend(build_delegate_tokens(&active));

    let max_position = active.len();

    let mut task_weights: BTreeMap<String, f64> = BTreeMap::new();
    for task in &active {
        let instance_props = task_to_instance_props_from_info(task);
        let context = build_context_props(task, now, max_position);
        let resolved = resolve_prototype(&rhai_engine, prototype_props, &instance_props, &context);
        let weight = resolved.get("task_weight").copied().unwrap_or(1.0);
        task_weights.insert(task.block_id.clone(), weight);
    }

    let transitions: Vec<TaskTransition> = active
        .iter()
        .flat_map(|t| build_task_transitions(t, default_duration, &task_weights))
        .collect();

    let objective_expr_src = build_objective_expr(&active, &task_weights);
    let objective_expr = CompiledExpr::compile(&rhai_engine, &objective_expr_src)
        .unwrap_or_else(|e| panic!("failed to compile objective expression: {e}"));

    let net = TaskNet {
        transitions,
        objective_expr,
        constraints: vec![],
        discount_rate,
    };
    let marking = TaskMarking { clock: now, tokens };

    (net, marking)
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
            if tasks[idx].has_sequential_dep {
                if let Some(ref prev_id) = prev_sibling_id {
                    if !tasks[idx].depends_on.contains(prev_id) {
                        tasks[idx].depends_on.push(prev_id.clone());
                    }
                }
            }
            prev_sibling_id = Some(tasks[idx].block_id.clone());
        }
    }
}

fn build_self_token(active_tasks: &[&TaskInfo], self_desc: &SelfDescriptor) -> TaskToken {
    let occupied = active_tasks
        .iter()
        .filter(|t| {
            t.task_state
                .as_ref()
                .map(|ts| ts.is_doing())
                .unwrap_or(false)
        })
        .count() as i64;

    TaskToken {
        id: "self".to_string(),
        token_type: "person".to_string(),
        attributes: {
            let mut a = BTreeMap::new();
            a.insert("status".to_string(), Value::String("active".to_string()));
            a.insert("energy".to_string(), Value::Float(self_desc.energy));
            a.insert("focus".to_string(), Value::Float(self_desc.focus));
            a.insert("mental_slots_occupied".to_string(), Value::Int(occupied));
            a.insert(
                "mental_slots_capacity".to_string(),
                Value::Int(self_desc.mental_slots_capacity),
            );
            a
        },
    }
}

fn build_completion_tokens(completed: &[&TaskInfo]) -> Vec<TaskToken> {
    completed
        .iter()
        .map(|t| TaskToken {
            id: format!("completed_{}", t.block_id),
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
        if let Executor::Delegated { ref person } = task.executor {
            if seen.insert(person.clone()) {
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
                    id_expr: format!("\"waiting_for_{}\"", task.block_id),
                    token_type: "waiting".to_string(),
                    attrs: {
                        let mut a = BTreeMap::new();
                        a.insert("source_task".to_string(), format!("\"{}\"", task.block_id));
                        a.insert("delegate".to_string(), format!("\"{person}\""));
                        a
                    },
                }],
                duration: 0.0,
            });

            let person_bind = format!("delegate_{}", person.replace(' ', "_"));
            let mut pcond = BTreeMap::new();
            pcond.insert("name".to_string(), person.clone());
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
            wcond.insert("source_task".to_string(), task.block_id.clone());
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
        let bind_name = link.replace('/', "_").replace(' ', "_");
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
        precond.insert("source_task".to_string(), dep_id.clone());
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
        id_expr: format!("\"completed_{}\"", task.block_id),
        token_type: "completion".to_string(),
        attrs: {
            let mut a = BTreeMap::new();
            a.insert("source_task".to_string(), format!("\"{}\"", task.block_id));
            a.insert("task_weight".to_string(), format!("{weight}"));
            a
        },
    });

    if task.is_question {
        creates.push(CreateArc {
            id_expr: format!("\"knowledge_{}\"", task.block_id),
            token_type: "knowledge".to_string(),
            attrs: {
                let mut a = BTreeMap::new();
                a.insert("source_task".to_string(), format!("\"{}\"", task.block_id));
                a.insert("confidence".to_string(), "0.8".to_string());
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
                "(if is_def_var(\"completed_{bid}\") && completed_{bid}.source_task == \"{bid}\" {{ {weight:.6} }} else {{ 0.0 }})",
                bid = task.block_id
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
pub fn rank_tasks(blocks: &[Block]) -> RankResult {
    let rhai_engine = RhaiEngine::new();

    let prototype_block = blocks.iter().find(|b| is_prototype_block(b));
    let self_block = blocks.iter().find(|b| SelfDescriptor::is_self_block(b));

    let mut prototype_props = default_prototype_props(&rhai_engine);
    if let Some(pb) = prototype_block {
        let overrides = block_to_prototype_props(&rhai_engine, pb);
        for (k, v) in overrides {
            prototype_props.insert(k, v);
        }
    }

    let self_desc = self_block
        .map(SelfDescriptor::from_block)
        .unwrap_or_else(SelfDescriptor::defaults);

    let (net, marking) = materialize(blocks, &self_desc, &prototype_props);

    let engine = Engine::new();
    let enabled = engine.enabled(&net, &marking);
    let ranked = engine.rank(&net, &marking, &enabled);

    let ranked_tasks = ranked
        .into_iter()
        .map(|rt| {
            let transition = net.transition(&rt.binding.transition_id).unwrap();
            RankedTask {
                block_id: rt.binding.transition_id.clone(),
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

    RankResult {
        ranked: ranked_tasks,
        mental_slots: MentalSlotsInfo {
            occupied,
            capacity: self_desc.mental_slots_capacity as usize,
        },
    }
}
