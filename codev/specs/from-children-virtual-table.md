# Feature: `from children` Virtual Table in PRQL

## Goal

Replace the hard-coded `REGION` property handling in `backend_engine.rs` with a declarative, PRQL-native approach using virtual tables like `from children`.

## Current Problem

`backend_engine.rs:953-1002` has hard-coded region discovery:
- Queries blocks with `REGION` property
- Hard-coded display names ("main" → "Main Content")
- Inflexible layout structure

## Solution: PRQL Stdlib Injection

Inject a "stdlib" of virtual table definitions before compiling user PRQL queries:

```prql
# STDLIB (auto-injected before every user query)
let children = (from blocks | filter parent_id == $context_id)
let roots = (from blocks | filter parent_id == null)
let siblings = (from blocks | filter parent_id == $context_parent_id)
```

Then users write:
```prql
from children
select {id, content}
```

Which compiles to:
```sql
WITH children AS (
  SELECT * FROM blocks WHERE parent_id = $context_id
)
SELECT id, content FROM children
```

## Implementation Steps

### 1. Add QueryContext struct

File: `crates/holon/src/api/backend_engine.rs`

```rust
/// Context for query compilation - determines what virtual tables resolve to
pub struct QueryContext {
    /// Current block ID for `from children` resolution. None = root level (parent_id IS NULL)
    pub current_block_id: Option<String>,
    /// Parent of current block for `from siblings` resolution
    pub context_parent_id: Option<String>,
}

impl QueryContext {
    pub fn root() -> Self {
        Self { current_block_id: None, context_parent_id: None }
    }

    pub fn for_block(block_id: String, parent_id: Option<String>) -> Self {
        Self { current_block_id: Some(block_id), context_parent_id: parent_id }
    }
}
```

### 2. Create stdlib injection function

File: `crates/holon/src/api/backend_engine.rs`

```rust
/// PRQL stdlib defining virtual tables for hierarchical queries
const PRQL_STDLIB: &str = r#"
let children = (from blocks | filter parent_id == $context_id)
let roots = (from blocks | filter parent_id == null)
let siblings = (from blocks | filter parent_id == $context_parent_id)
"#;

/// Inject stdlib before user PRQL query
fn inject_stdlib(user_prql: &str) -> String {
    format!("{}\n{}", PRQL_STDLIB, user_prql)
}
```

### 3. Modify compile_query to accept context

File: `crates/holon/src/api/backend_engine.rs`

Change signature:
```rust
pub fn compile_query(&self, prql: String, context: Option<QueryContext>) -> Result<(String, RenderSpec)>
```

At the start of compile_query, inject stdlib:
```rust
let prql_with_stdlib = inject_stdlib(&prql);
// Use prql_with_stdlib for parsing instead of prql
```

### 4. Bind context parameters at execution time

File: `crates/holon/src/api/backend_engine.rs`

In `execute_query` and `watch_query`, bind context parameters:

```rust
fn bind_context_params(params: &mut HashMap<String, Value>, context: &QueryContext) {
    match &context.current_block_id {
        Some(id) => params.insert("context_id".to_string(), Value::String(id.clone())),
        None => params.insert("context_id".to_string(), Value::Null),
    };
    match &context.context_parent_id {
        Some(id) => params.insert("context_parent_id".to_string(), Value::String(id.clone())),
        None => params.insert("context_parent_id".to_string(), Value::Null),
    };
}
```

### 5. Update init_app_frame to use root layout block

File: `crates/holon/src/api/backend_engine.rs`

Replace `load_index_regions` with:

```rust
async fn load_root_layout_block(&self) -> Result<Block> {
    // Find first root block (parent_id IS NULL) ordered by sort_key
    let sql = r#"
        SELECT b.id, b.content, src.content as prql_source
        FROM blocks b
        LEFT JOIN blocks src ON src.parent_id = b.id
            AND src.content_type = 'source'
            AND src.source_language = 'prql'
        WHERE b.parent_id IS NULL
        ORDER BY b.sort_key
        LIMIT 1
    "#;
    // ... execute and return
}
```

Then in `init_app_frame`:
```rust
pub async fn init_app_frame(&self) -> Result<AppFrame> {
    let root_block = self.load_root_layout_block().await?;

    // Compile root block's PRQL with root context
    let context = QueryContext::root();
    let (sql, render_spec) = self.compile_query(root_block.prql_source, Some(context))?;

    // Execute to get layout children
    let layout_data = self.execute_query(sql, /* params with context */).await?;

    // Build regions from query results (each child becomes a region)
    // ...
}
```

## Example Document Structure

```org
* Holon App
:PROPERTIES:
:ID: holon-root
:END:
#+BEGIN_SRC holon_prql
from children
select {id, content, sort_key, width = s"COALESCE(json_extract(properties, '$.width'), 1.0)"}
sort sort_key
render (columns item_template:(panel width:width content:(render_entity this)))
#+END_SRC

** Navigation
:PROPERTIES:
:width: 0.25
:END:
#+BEGIN_SRC holon_prql
from documents
render (list item_template:(document_link this))
#+END_SRC

** Main Content
:PROPERTIES:
:width: 0.5
:END:
#+BEGIN_SRC holon_prql
from children
join cf = current_focus (cf.region == "main")
filter parent_id == cf.block_id
render (tree item_template:(render_entity this))
#+END_SRC
```

## Key Files to Modify

1. `crates/holon/src/api/backend_engine.rs` - Main changes (QueryContext, stdlib injection, init_app_frame)
2. `crates/holon-api/src/app_frame.rs` - May need to update AppFrame structure
3. `frontends/flutter/lib/ui/widgets/region_widget.dart` - May need updates if AppFrame changes

## PRQL Compilation Proof

Verified working PRQL:

```prql
let children = (from blocks | filter parent_id == $context_id)

from children
select {id, content, width = s"COALESCE(json_extract(properties, '$.width'), 1.0)"}
sort sort_key
```

Compiles to:
```sql
WITH children AS (
  SELECT * FROM blocks WHERE parent_id = $context_id
)
SELECT
  id,
  content,
  sort_key,
  COALESCE(json_extract(properties, '$.width'), 1.0) AS width
FROM children
ORDER BY sort_key
```

## Testing

1. Create test with root block containing `from children` query
2. Verify stdlib injection produces valid SQL
3. Verify context parameter binding works
4. Verify layout children are correctly queried
5. Verify Flutter receives proper region configs
