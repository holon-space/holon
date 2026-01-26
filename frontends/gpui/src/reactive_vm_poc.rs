//! Proof-of-concept for the target reactive ViewModel architecture.
//!
//! Shared types and logic used by both the interactive demo
//! (`examples/reactive_vm_demo.rs`) and headless tests
//! (`tests/reactive_vm_test.rs`).
//!
//! Each node in the tree is a persistent GPUI `Entity` that owns its
//! reactive inputs (`Mutable<RenderExpr>`, `Mutable<Arc<DataRow>>`) and
//! self-interprets when any input changes. Changes push DOWN the tree —
//! no external tree walks, no reconciliation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures_signals::signal::{Mutable, SignalExt};
use futures_signals::signal_vec::MutableVec;
use gpui::prelude::*;
use gpui::*;

use holon_api::render_types::{Arg, RenderExpr};
use holon_api::Value;

pub type DataRow = holon_api::widget_spec::DataRow;

// ── Interpreter trait ─────────────────────────────────────────────────
//
// Validates the `BuilderServices`-like trait-object boundary from the
// target architecture.  Nodes call `interpreter.interpret(expr, row)`
// instead of a free function — this is the one and only entry point
// for interpretation in the reactive pipeline.

pub trait Interpreter: Send + Sync + 'static {
    fn interpret(&self, expr: &RenderExpr, row: &DataRow) -> String;
}

pub struct DefaultInterpreter;

impl Interpreter for DefaultInterpreter {
    fn interpret(&self, expr: &RenderExpr, row: &DataRow) -> String {
        interpret_expr(expr, row)
    }
}

pub fn default_interpreter() -> Arc<dyn Interpreter> {
    Arc::new(DefaultInterpreter)
}

// ── RenderExpr helpers ─────────────────────────────────────────────────

pub fn expr(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args,
    }
}

pub fn positional_arg(value: RenderExpr) -> Arg {
    Arg { name: None, value }
}

pub fn named_arg(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value,
    }
}

pub fn col_ref(name: &str) -> RenderExpr {
    RenderExpr::ColumnRef {
        name: name.to_string(),
    }
}

pub fn literal_str(s: &str) -> RenderExpr {
    RenderExpr::Literal {
        value: Value::String(s.to_string()),
    }
}

pub fn expr_kind(e: &RenderExpr) -> &str {
    match e {
        RenderExpr::FunctionCall { name, .. } => name.as_str(),
        RenderExpr::ColumnRef { .. } => "col",
        RenderExpr::Literal { .. } => "literal",
        _ => "other",
    }
}

// ── Mini interpreter ───────────────────────────────────────────────────

pub fn interpret_expr(render_expr: &RenderExpr, row: &DataRow) -> String {
    match render_expr {
        RenderExpr::FunctionCall { name, args } => match name.as_str() {
            "text" => args
                .first()
                .map(|a| interpret_expr(&a.value, row))
                .unwrap_or_default(),
            "bold" => {
                let inner = args
                    .first()
                    .map(|a| interpret_expr(&a.value, row))
                    .unwrap_or_default();
                format!("**{inner}**")
            }
            "badge" => {
                let inner = args
                    .first()
                    .map(|a| interpret_expr(&a.value, row))
                    .unwrap_or_default();
                format!("[{inner}]")
            }
            "icon" => {
                let name = args
                    .first()
                    .map(|a| interpret_expr(&a.value, row))
                    .unwrap_or("?".into());
                format!("⬡{name}")
            }
            "row" => args
                .iter()
                .map(|a| interpret_expr(&a.value, row))
                .collect::<Vec<_>>()
                .join("  "),
            "tree_item" => {
                let inner = args
                    .first()
                    .map(|a| interpret_expr(&a.value, row))
                    .unwrap_or_default();
                format!("├─ {inner}")
            }
            "table_row" => {
                let content = row
                    .get("content")
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                format!("│ {content} │")
            }
            "spacer" => "───".into(),
            other => format!("<{other}>"),
        },
        RenderExpr::ColumnRef { name } => row
            .get(name)
            .map(|v| v.to_display_string())
            .unwrap_or_default(),
        RenderExpr::Literal { value } => value.to_display_string(),
        _ => String::new(),
    }
}

// ── Sample data & templates ────────────────────────────────────────────

pub fn make_row(id: &str, content: &str, priority: &str) -> DataRow {
    let mut row = DataRow::new();
    row.insert("id".into(), Value::String(id.into()));
    row.insert("content".into(), Value::String(content.into()));
    row.insert("priority".into(), Value::String(priority.into()));
    row
}

pub fn make_sample_data() -> Vec<DataRow> {
    vec![
        make_row("1", "Buy milk", "high"),
        make_row("2", "Write docs", "low"),
        make_row("3", "Fix bug #42", "medium"),
        make_row("4", "Review PR", "high"),
        make_row("5", "Deploy v2", "critical"),
    ]
}

pub fn tree_template() -> RenderExpr {
    expr(
        "tree_item",
        vec![positional_arg(expr(
            "row",
            vec![
                positional_arg(expr("bold", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        ))],
    )
}

pub fn table_template() -> RenderExpr {
    expr("table_row", vec![])
}

pub fn compact_template() -> RenderExpr {
    expr(
        "row",
        vec![positional_arg(expr(
            "text",
            vec![positional_arg(col_ref("content"))],
        ))],
    )
}

// ── ItemNode — self-interpreting collection item ───────────────────────

pub struct ItemNode {
    pub data: Mutable<Arc<DataRow>>,
    #[allow(dead_code)]
    pub template: Mutable<RenderExpr>,
    pub display: Mutable<String>,
    interpreter: Arc<dyn Interpreter>,
    _signal_task: Task<()>,
}

impl ItemNode {
    pub fn new(
        data: Arc<DataRow>,
        template: Mutable<RenderExpr>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_tmpl = template.get_cloned();
        let display = Mutable::new(interpreter.interpret(&initial_tmpl, &data));
        let data = Mutable::new(data);

        let _signal_task = {
            let data_signal = data.signal_cloned();
            let tmpl_signal = template.signal_cloned();
            let display_handle = display.clone();
            let interp = interpreter.clone();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                use futures_signals::map_ref;
                let combined = map_ref! {
                    let d = data_signal,
                    let t = tmpl_signal
                    => (d.clone(), t.clone())
                };
                let mut stream = combined.to_stream();
                stream.next().await;
                while let Some((d, t)) = stream.next().await {
                    match this.update(cx, |_, cx| cx.notify()) {
                        Ok(_) => display_handle.set(interp.interpret(&t, &d)),
                        Err(_) => break,
                    }
                }
            })
        };

        Self {
            data,
            template,
            display,
            interpreter,
            _signal_task,
        }
    }

    pub fn snapshot(&self) -> String {
        self.display.get_cloned()
    }
}

impl Render for ItemNode {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let current_data = self.data.get_cloned();
        let current_tmpl = self.template.get_cloned();
        let display = self.interpreter.interpret(&current_tmpl, &current_data);
        div()
            .pl_2()
            .py(px(1.0))
            .text_sm()
            .font_family("monospace")
            .child(display)
    }
}

// ── ReactiveNode — structural push-down node ──────────────────────────

pub struct ReactiveNode {
    pub expr: Mutable<RenderExpr>,
    pub data: Mutable<Arc<DataRow>>,
    pub display: Mutable<String>,
    pub children: Vec<Entity<ReactiveNode>>,
    pub render_count: Arc<AtomicUsize>,
    pub interpreter: Arc<dyn Interpreter>,
    pub expanded: Option<Mutable<bool>>,
    _tasks: Vec<Task<()>>,
}

impl ReactiveNode {
    pub fn new(
        initial_expr: RenderExpr,
        data: Arc<DataRow>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_shared(initial_expr, Mutable::new(data), interpreter, cx)
    }

    pub fn new_collection(
        item_template: RenderExpr,
        rows: Vec<Arc<DataRow>>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_collection_with_template(Mutable::new(item_template), rows, interpreter, cx)
    }

    pub fn new_collection_with_template(
        item_template: Mutable<RenderExpr>,
        rows: Vec<Arc<DataRow>>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let children: Vec<Entity<ReactiveNode>> = rows
            .iter()
            .map(|row| {
                let data = Mutable::new(row.clone());
                let tmpl = item_template.clone();
                let interp = interpreter.clone();
                cx.new(|cx| Self::new_leaf(tmpl, data, interp, cx))
            })
            .collect();

        let empty_row = Arc::new(DataRow::new());
        Self {
            expr: item_template,
            data: Mutable::new(empty_row),
            display: Mutable::new(String::new()),
            children,
            render_count: Arc::new(AtomicUsize::new(0)),
            interpreter,
            expanded: None,
            _tasks: vec![],
        }
    }

    fn new_leaf(
        expr: Mutable<RenderExpr>,
        data: Mutable<Arc<DataRow>>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_expr = expr.get_cloned();
        let display = Mutable::new(interpreter.interpret(&initial_expr, &data.get_cloned()));

        let signal_task = Self::spawn_signal_task(&expr, &data, &display, &interpreter, cx);

        Self {
            expr,
            data,
            display,
            children: vec![],
            render_count: Arc::new(AtomicUsize::new(0)),
            interpreter,
            expanded: None,
            _tasks: vec![signal_task],
        }
    }

    pub fn new_expandable(
        initial_expr: RenderExpr,
        data: Arc<DataRow>,
        interpreter: Arc<dyn Interpreter>,
        initially_expanded: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let data_m = Mutable::new(data);
        let expr_m = Mutable::new(initial_expr.clone());
        let display = Mutable::new(interpreter.interpret(&initial_expr, &data_m.get_cloned()));
        let expanded = Mutable::new(initially_expanded);

        let signal_task = Self::spawn_signal_task(&expr_m, &data_m, &display, &interpreter, cx);

        let children = if initially_expanded {
            Self::build_children(&initial_expr, &data_m, &interpreter, cx)
        } else {
            vec![]
        };

        let expand_task = {
            let signal = expanded.signal();
            let expr_for_expand = expr_m.clone();
            let data_for_expand = data_m.clone();
            let interp_for_expand = interpreter.clone();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = signal.to_stream();
                stream.next().await;
                while let Some(is_expanded) = stream.next().await {
                    let result = this.update(cx, |node, cx| {
                        if is_expanded {
                            let current_expr = expr_for_expand.get_cloned();
                            node.children = Self::build_children(
                                &current_expr,
                                &data_for_expand,
                                &interp_for_expand,
                                cx,
                            );
                        } else {
                            node.children.clear();
                        }
                        cx.notify();
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
        };

        Self {
            expr: expr_m,
            data: data_m,
            display,
            children,
            render_count: Arc::new(AtomicUsize::new(0)),
            interpreter,
            expanded: Some(expanded),
            _tasks: vec![signal_task, expand_task],
        }
    }

    fn new_shared(
        initial_expr: RenderExpr,
        data: Mutable<Arc<DataRow>>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let expr_m = Mutable::new(initial_expr.clone());
        let display = Mutable::new(interpreter.interpret(&initial_expr, &data.get_cloned()));

        let signal_task = Self::spawn_signal_task(&expr_m, &data, &display, &interpreter, cx);

        let children = Self::build_children(&initial_expr, &data, &interpreter, cx);

        Self {
            expr: expr_m,
            data,
            display,
            children,
            render_count: Arc::new(AtomicUsize::new(0)),
            interpreter,
            expanded: None,
            _tasks: vec![signal_task],
        }
    }

    fn spawn_signal_task(
        expr_m: &Mutable<RenderExpr>,
        data: &Mutable<Arc<DataRow>>,
        display: &Mutable<String>,
        interpreter: &Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let data_signal = data.signal_cloned();
        let expr_signal = expr_m.signal_cloned();
        let display_handle = display.clone();
        let interp = interpreter.clone();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            use futures_signals::map_ref;
            let combined = map_ref! {
                let d = data_signal,
                let e = expr_signal
                => (d.clone(), e.clone())
            };
            let mut stream = combined.to_stream();
            stream.next().await;
            while let Some((d, e)) = stream.next().await {
                match this.update(cx, |_, cx| cx.notify()) {
                    Ok(_) => display_handle.set(interp.interpret(&e, &d)),
                    Err(_) => break,
                }
            }
        })
    }

    fn build_children(
        parent_expr: &RenderExpr,
        data: &Mutable<Arc<DataRow>>,
        interpreter: &Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<ReactiveNode>> {
        match parent_expr {
            RenderExpr::FunctionCall { args, .. } => args
                .iter()
                .map(|arg| {
                    let data_clone = data.clone();
                    let expr = arg.value.clone();
                    let interp = interpreter.clone();
                    cx.new(|cx| Self::new_shared(expr, data_clone, interp, cx))
                })
                .collect(),
            _ => vec![],
        }
    }

    fn create_child(
        expr: RenderExpr,
        data: &Mutable<Arc<DataRow>>,
        interpreter: &Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Entity<ReactiveNode> {
        let data_clone = data.clone();
        let interp = interpreter.clone();
        cx.new(|cx| Self::new_shared(expr, data_clone, interp, cx))
    }

    pub fn apply_expr(&mut self, new_expr: RenderExpr, cx: &mut Context<Self>) {
        let old_expr = self.expr.get_cloned();
        self.expr.set(new_expr.clone());

        let old_name = match &old_expr {
            RenderExpr::FunctionCall { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let new_name = match &new_expr {
            RenderExpr::FunctionCall { name, .. } => Some(name.as_str()),
            _ => None,
        };

        if old_name != new_name {
            self.children = Self::build_children(&new_expr, &self.data, &self.interpreter, cx);
            cx.notify();
            return;
        }

        let old_args = match &old_expr {
            RenderExpr::FunctionCall { args, .. } => args.as_slice(),
            _ => &[],
        };
        let new_args = match &new_expr {
            RenderExpr::FunctionCall { args, .. } => args.as_slice(),
            _ => &[],
        };

        let all_named = !old_args.is_empty()
            && !new_args.is_empty()
            && old_args.iter().all(|a| a.name.is_some())
            && new_args.iter().all(|a| a.name.is_some());

        if all_named {
            self.apply_named_args(old_args, new_args, cx);
        } else {
            self.apply_positional_args(old_args, new_args, cx);
        }
        cx.notify();
    }

    fn apply_positional_args(
        &mut self,
        old_args: &[Arg],
        new_args: &[Arg],
        cx: &mut Context<Self>,
    ) {
        let min_len = old_args.len().min(new_args.len());

        for i in 0..min_len {
            let old_kind = expr_kind(&old_args[i].value);
            let new_kind = expr_kind(&new_args[i].value);
            if old_kind == new_kind {
                self.children[i].update(cx, |child, cx| {
                    child.apply_expr(new_args[i].value.clone(), cx);
                });
            } else {
                self.children[i] = Self::create_child(
                    new_args[i].value.clone(),
                    &self.data,
                    &self.interpreter,
                    cx,
                );
            }
        }

        for arg in &new_args[min_len..] {
            self.children.push(Self::create_child(
                arg.value.clone(),
                &self.data,
                &self.interpreter,
                cx,
            ));
        }

        self.children.truncate(new_args.len());
    }

    fn apply_named_args(&mut self, old_args: &[Arg], new_args: &[Arg], cx: &mut Context<Self>) {
        use std::collections::HashMap;

        let old_by_name: HashMap<&str, usize> = old_args
            .iter()
            .enumerate()
            .map(|(i, a)| (a.name.as_deref().unwrap(), i))
            .collect();

        let mut new_children = Vec::with_capacity(new_args.len());

        for new_arg in new_args {
            let name = new_arg.name.as_deref().unwrap();
            if let Some(&old_idx) = old_by_name.get(name) {
                let old_kind = expr_kind(&old_args[old_idx].value);
                let new_kind = expr_kind(&new_arg.value);
                if old_kind == new_kind {
                    let mut child = self.children[old_idx].clone();
                    child.update(cx, |c, cx| c.apply_expr(new_arg.value.clone(), cx));
                    new_children.push(child);
                } else {
                    new_children.push(Self::create_child(
                        new_arg.value.clone(),
                        &self.data,
                        &self.interpreter,
                        cx,
                    ));
                }
            } else {
                new_children.push(Self::create_child(
                    new_arg.value.clone(),
                    &self.data,
                    &self.interpreter,
                    cx,
                ));
            }
        }

        self.children = new_children;
    }

    pub fn snapshot(&self) -> String {
        self.display.get_cloned()
    }

    pub fn tree_snapshot(&self, cx: &gpui::App) -> TreeSnapshot {
        let current_expr = self.expr.get_cloned();
        let current_data = self.data.get_cloned();
        TreeSnapshot {
            kind: expr_kind(&current_expr).to_string(),
            display: self.interpreter.interpret(&current_expr, &current_data),
            expanded: self.expanded.as_ref().map(|m| m.get()),
            children: self
                .children
                .iter()
                .map(|c| c.read(cx).tree_snapshot(cx))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    pub kind: String,
    pub display: String,
    pub expanded: Option<bool>,
    pub children: Vec<TreeSnapshot>,
}

impl TreeSnapshot {
    pub fn leaf_displays(&self) -> Vec<&str> {
        if self.children.is_empty() {
            vec![&self.display]
        } else {
            self.children
                .iter()
                .flat_map(|c| c.leaf_displays())
                .collect()
        }
    }
}

// ── ReactiveCollection — VecDiff-driven persistent collection ────────
//
// Models a collection of persistent child nodes backed by MutableVec.
// Supports insert, remove, update, and move operations — simulating
// CDC VecDiff events from production's ReactiveView.
//
// Each child is a ReactiveNode with its own Mutable<Arc<DataRow>>.
// UpdateAt just sets the child's data Mutable (cheap, no rebuild).
// InsertAt creates a new entity. RemoveAt drops the entity.

pub struct ReactiveCollection {
    pub template: Mutable<RenderExpr>,
    pub items: MutableVec<Entity<ReactiveNode>>,
    pub interpreter: Arc<dyn Interpreter>,
}

impl ReactiveCollection {
    pub fn new(
        template: RenderExpr,
        rows: Vec<Arc<DataRow>>,
        interpreter: Arc<dyn Interpreter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let template = Mutable::new(template);
        let items: Vec<Entity<ReactiveNode>> = rows
            .into_iter()
            .map(|row| {
                let tmpl = template.clone();
                let data = Mutable::new(row);
                let interp = interpreter.clone();
                cx.new(|cx| ReactiveNode::new_leaf(tmpl, data, interp, cx))
            })
            .collect();
        Self {
            template,
            items: MutableVec::new_with_values(items),
            interpreter,
        }
    }

    pub fn insert(&self, index: usize, row: Arc<DataRow>, cx: &mut Context<Self>) {
        let tmpl = self.template.clone();
        let data = Mutable::new(row);
        let interp = self.interpreter.clone();
        let entity = cx.new(|cx| ReactiveNode::new_leaf(tmpl, data, interp, cx));
        self.items.lock_mut().insert_cloned(index, entity);
        cx.notify();
    }

    pub fn remove(&self, index: usize, cx: &mut Context<Self>) {
        self.items.lock_mut().remove(index);
        cx.notify();
    }

    pub fn update_data(&self, index: usize, row: Arc<DataRow>, cx: &App) {
        let items = self.items.lock_ref();
        items[index].read(cx).data.set(row);
    }

    pub fn move_item(&self, from: usize, to: usize, cx: &mut Context<Self>) {
        let mut items = self.items.lock_mut();
        let entity = items.remove(from);
        items.insert_cloned(to, entity);
        cx.notify();
    }

    pub fn len(&self) -> usize {
        self.items.lock_ref().len()
    }

    pub fn snapshot(&self, cx: &App) -> Vec<TreeSnapshot> {
        self.items
            .lock_ref()
            .iter()
            .map(|e| e.read(cx).tree_snapshot(cx))
            .collect()
    }

    pub fn child_displays(&self, cx: &App) -> Vec<String> {
        self.items
            .lock_ref()
            .iter()
            .map(|e| {
                let node = e.read(cx);
                node.interpreter
                    .interpret(&node.expr.get_cloned(), &node.data.get_cloned())
            })
            .collect()
    }
}

impl Render for ReactiveCollection {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut el = div().pl_4();
        for item in self.items.lock_ref().iter() {
            el = el.child(item.clone().into_any_element());
        }
        el
    }
}

impl Render for ReactiveNode {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.render_count.fetch_add(1, Ordering::Relaxed);
        let current_expr = self.expr.get_cloned();
        let current_data = self.data.get_cloned();
        let kind = expr_kind(&current_expr);
        let has_children = !self.children.is_empty();

        let mut el = div().pl_4().py(px(1.0));

        if let Some(ref expanded_m) = self.expanded {
            let is_expanded = expanded_m.get();
            let chevron = if is_expanded { "▼" } else { "▶" };
            let display = self.interpreter.interpret(&current_expr, &current_data);
            let toggle = expanded_m.clone();
            el = el.child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .id("expand-toggle")
                            .cursor_pointer()
                            .text_xs()
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                toggle.set(!toggle.get());
                            })
                            .child(chevron.to_string()),
                    )
                    .child(div().text_sm().font_family("monospace").child(display)),
            );
            if is_expanded {
                for child in &self.children {
                    el = el.child(child.clone().into_any_element());
                }
            }
        } else if has_children {
            el = el.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x8888aa))
                    .child(format!("{kind}()")),
            );
            for child in &self.children {
                el = el.child(child.clone().into_any_element());
            }
        } else {
            let display = self.interpreter.interpret(&current_expr, &current_data);
            el = el.child(div().text_sm().font_family("monospace").child(display));
        }
        el
    }
}
