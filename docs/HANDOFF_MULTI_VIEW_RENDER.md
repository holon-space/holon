# Multi-View Render Implementation

## Goal

Enable queries to define multiple named render views that the frontend can switch between, supporting routing of data to different UI regions (sidebar, main, etc.) while preserving per-row CDC efficiency.

## Syntax

```prql
from blocks
derive {
  ui = (render (row (text this.content))),
  region = case [this.in_sidebar => "sidebar", true => "main"]
}
render (views
  sidebar:(list filter:(this.region == "sidebar") item_template:this.ui)
  main:(tree filter:(this.region == "main") parent_id:parent_id item_template:this.ui)
)
```

**Key points:**
- Data stays flat in SQL (no GROUP BY aggregation)
- `region` is a regular column computed by PRQL
- Each view has a `filter` expression that selects which rows belong to it
- Frontend filters rows client-side based on the filter expression
- Per-row CDC is preserved (no efficiency penalty)

**Alternative using `let` for readability:**
```prql
let sidebar_view = (list filter:(this.region == "sidebar") item_template:this.ui)
let main_view = (tree filter:(this.region == "main") item_template:this.ui)

from blocks
derive { ui = (render ...), region = case [...] }
render (views sidebar:sidebar_view main:main_view)
```

## PRQL Parsing

The syntax parses correctly at the PL (Pipeline Language) level. Example AST:

```yaml
FuncCall:
  name: render
  args:
    - FuncCall:
        name: views
        named_args:
          sidebar:
            FuncCall:
              name: list
              named_args:
                filter: Binary { left: this.region, op: Eq, right: "sidebar" }
                item_template: Ident [this, ui]
          main:
            FuncCall:
              name: tree
              named_args:
                filter: Binary { ... }
                item_template: ...
```

## Type Changes

### `/crates/holon-api/src/render_types.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderSpec {
    /// Named views for multi-view rendering. Empty for single-view queries.
    pub views: HashMap<String, ViewSpec>,

    /// Default view name (first view, or "default" for single-view)
    pub default_view: String,

    /// Existing fields (unchanged)
    pub nested_queries: Vec<String>,
    pub operations: HashMap<String, OperationWiring>,
    pub row_templates: Vec<RowTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSpec {
    /// Filter expression to select rows for this view (evaluated client-side)
    pub filter: Option<FilterExpr>,

    /// The collection render expression (list, tree, table, etc.)
    pub structure: RenderExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterExpr {
    /// Column equals literal: this.region == "sidebar"
    Eq { column: String, value: serde_json::Value },

    /// Column not equals
    Ne { column: String, value: serde_json::Value },

    /// Boolean AND of filters
    And(Vec<FilterExpr>),

    /// Boolean OR of filters
    Or(Vec<FilterExpr>),

    /// Always true (no filter)
    All,
}

impl RenderSpec {
    /// Backward compatibility: get the single/default view's structure
    pub fn root(&self) -> Option<&RenderExpr> {
        self.views.get(&self.default_view).map(|v| &v.structure)
    }
}
```

## Parser Changes

### `/crates/holon-prql-render/src/parser.rs`

Modify `extract_render_from_module()` to detect the `views` pattern:

```rust
pub enum ExtractedRender {
    /// Single view (backward compatible): render (list ...)
    Single(Expr),

    /// Multiple named views: render (views sidebar:(...) main:(...))
    Named(Vec<(String, Expr)>),
}

fn extract_render_from_expr(expr: &mut Expr) -> Option<ExtractedRender> {
    // ... existing pipeline extraction ...

    if let ExprKind::FuncCall(func_call) = &render_expr.kind {
        // Check if first arg is a `views` call
        if let Some(first_arg) = func_call.args.first() {
            if is_views_call(first_arg) {
                // Extract named views from the `views` call's named_args
                return Some(ExtractedRender::Named(extract_named_views(first_arg)));
            }
        }
        // Otherwise, it's a single view
        return Some(ExtractedRender::Single(render_expr));
    }
}

fn is_views_call(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::FuncCall(fc)
        if matches!(&fc.name.kind, ExprKind::Ident(id) if id.name == "views"))
}

fn extract_named_views(views_expr: &Expr) -> Vec<(String, Expr)> {
    if let ExprKind::FuncCall(fc) = &views_expr.kind {
        fc.named_args.iter()
            .map(|(name, expr)| (name.clone(), expr.clone()))
            .collect()
    } else {
        vec![]
    }
}
```

## Compiler Changes

### `/crates/holon-prql-render/src/compiler.rs`

Add handling for named views and filter expressions:

```rust
pub fn compile_render_spec(extracted: ExtractedRender) -> Result<RenderSpec> {
    match extracted {
        ExtractedRender::Single(expr) => {
            // Existing single-view compilation
            let structure = compile_render_expr(&expr)?;
            let mut views = HashMap::new();
            views.insert("default".to_string(), ViewSpec {
                filter: None,
                structure,
            });
            Ok(RenderSpec {
                views,
                default_view: "default".to_string(),
                ..Default::default()
            })
        }
        ExtractedRender::Named(named) => {
            let mut views = HashMap::new();
            let mut default_view = None;

            for (name, expr) in named {
                let view_spec = compile_view_spec(&expr)?;
                if default_view.is_none() {
                    default_view = Some(name.clone());
                }
                views.insert(name, view_spec);
            }

            Ok(RenderSpec {
                views,
                default_view: default_view.unwrap_or_else(|| "default".to_string()),
                ..Default::default()
            })
        }
    }
}

fn compile_view_spec(expr: &Expr) -> Result<ViewSpec> {
    // expr is e.g. (list filter:(...) item_template:(...))
    if let ExprKind::FuncCall(fc) = &expr.kind {
        let filter = fc.named_args.get("filter")
            .map(|f| compile_filter_expr(f))
            .transpose()?;

        // Remove filter from args before compiling structure
        let structure = compile_render_expr(expr)?;

        Ok(ViewSpec { filter, structure })
    } else {
        bail!("Expected function call for view spec")
    }
}

fn compile_filter_expr(expr: &Expr) -> Result<FilterExpr> {
    match &expr.kind {
        ExprKind::Binary(binary) if binary.op == BinOp::Eq => {
            let column = extract_column_name(&binary.left)?;
            let value = extract_literal_value(&binary.right)?;
            Ok(FilterExpr::Eq { column, value })
        }
        // Add more cases as needed (Ne, And, Or)
        _ => bail!("Unsupported filter expression"),
    }
}
```

## Frontend Changes

### `/frontends/flutter/lib/render/render_interpreter.dart`

Add view selection support:

```dart
class RenderInterpreter {
  /// Build widget for a specific named view
  Widget buildView(
    RenderSpec spec,
    String viewName,
    RenderContext context,
  ) {
    final viewSpec = spec.views[viewName] ?? spec.views[spec.defaultView];
    if (viewSpec == null) {
      return const Text('No view available');
    }

    // Apply filter to get rows for this view
    final filteredRows = applyFilter(context.allRows, viewSpec.filter);
    final filteredContext = context.copyWith(rows: filteredRows);

    return build(viewSpec.structure, filteredContext);
  }

  List<Map<String, dynamic>> applyFilter(
    List<Map<String, dynamic>> rows,
    FilterExpr? filter,
  ) {
    if (filter == null) return rows;

    return rows.where((row) => evaluateFilter(row, filter)).toList();
  }

  bool evaluateFilter(Map<String, dynamic> row, FilterExpr filter) {
    switch (filter) {
      case FilterExpr_Eq(:final column, :final value):
        return row[column] == value;
      case FilterExpr_Ne(:final column, :final value):
        return row[column] != value;
      case FilterExpr_And(:final filters):
        return filters.every((f) => evaluateFilter(row, f));
      case FilterExpr_Or(:final filters):
        return filters.any((f) => evaluateFilter(row, f));
      case FilterExpr_All():
        return true;
    }
  }
}
```

### New provider: `/frontends/flutter/lib/providers/view_selector_provider.dart`

```dart
@riverpod
class ViewSelector extends _$ViewSelector {
  @override
  String build(String queryId) {
    // Default to the spec's default view
    return ref.watch(renderSpecProvider(queryId)).defaultView;
  }

  void selectView(String viewName) {
    state = viewName;
  }

  List<String> get availableViews {
    return ref.read(renderSpecProvider(queryId)).views.keys.toList();
  }
}
```

### View switcher widget

```dart
class ViewSwitcher extends ConsumerWidget {
  final String queryId;

  Widget build(BuildContext context, WidgetRef ref) {
    final selector = ref.watch(viewSelectorProvider(queryId).notifier);
    final currentView = ref.watch(viewSelectorProvider(queryId));
    final views = selector.availableViews;

    if (views.length <= 1) return const SizedBox.shrink();

    return SegmentedButton<String>(
      segments: views.map((v) =>
        ButtonSegment(value: v, label: Text(v))
      ).toList(),
      selected: {currentView},
      onSelectionChanged: (s) => selector.selectView(s.first),
    );
  }
}
```

## Backward Compatibility

- Existing `render (list ...)` syntax continues to work
- Creates `RenderSpec { views: {"default": ViewSpec}, default_view: "default" }`
- `RenderSpec.root()` getter returns `views["default"].structure`
- Existing row templates (`derive { ui = (render ...) }`) are unaffected

## Key Files

| File | Changes |
|------|---------|
| `/crates/holon-api/src/render_types.rs` | Add `views`, `ViewSpec`, `FilterExpr` |
| `/crates/holon-prql-render/src/parser.rs` | Detect `views` pattern, extract named views |
| `/crates/holon-prql-render/src/compiler.rs` | Compile `ViewSpec` with filter |
| `/crates/holon-prql-render/src/lib.rs` | Update orchestration |
| `/frontends/flutter/lib/render/render_interpreter.dart` | View selection, filter evaluation |
| `/frontends/flutter/lib/providers/view_selector_provider.dart` | New file |

## Why This Approach

1. **Per-row CDC preserved**: Data stays flat, no SQL aggregation
2. **No data bloat**: Filters are metadata, not duplicated per row
3. **Turso IVM compatible**: Uses simple column filtering, fully supported
4. **Frontend control**: User can switch views, or display multiple simultaneously
5. **Query/render matching**: Views are co-located with query, validated together
6. **PRQL compatible**: Syntax parses correctly at PL level
