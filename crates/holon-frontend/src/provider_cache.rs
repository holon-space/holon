//! Weak-ref cache for `ReactiveRowProvider`s produced by value functions.
//!
//! When a render pass evaluates `ops_of(col("uri"))` twice within one
//! outer iteration (e.g. once in the bar, once in a separate slash
//! popup), both call sites should share a single provider Arc. The
//! cache keys on `(fn_name, args_fingerprint)` and holds a `Weak` ref
//! so entries evaporate when the last streaming collection drops.
//!
//! ## Lifecycle constraint
//!
//! Providers stored here MUST be derivable functions of upstream
//! signals — NOT stateful accumulators — so that re-constructing a
//! provider (after a `Weak::upgrade()` miss) reproduces the same
//! observable behaviour. See `ReactiveRowProvider`'s doc comment for
//! the full rationale.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;

use holon_api::ReactiveRowProvider;
use holon_api::render_eval::ResolvedArgs;

/// Cache key — function name + fingerprint of resolved args.
///
/// The fingerprint is a structural 64-bit hash over
/// `{positional} | {named (sorted)} | {templates (sorted)}` — no
/// per-lookup `Debug`-string allocation. Map-valued nodes (`Value::Object`,
/// `RenderExpr::Object`) hash their entries key-sorted so iteration order
/// can't perturb the fingerprint; floats hash by `to_bits`.
#[derive(PartialEq, Eq, Hash, Debug)]
struct ProviderKey {
    name: String,
    fingerprint: u64,
}

/// Shared weak-ref cache. One per `ReactiveEngine`.
pub struct ProviderCache {
    entries: Mutex<HashMap<ProviderKey, Weak<dyn ReactiveRowProvider>>>,
}

impl ProviderCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Return a shared provider for `(name, args)`, constructing one
    /// via `construct` on a miss. Stale `Weak`s are replaced.
    pub fn get_or_create(
        &self,
        name: &str,
        args: &ResolvedArgs,
        construct: impl FnOnce() -> Arc<dyn ReactiveRowProvider>,
    ) -> Arc<dyn ReactiveRowProvider> {
        let key = ProviderKey {
            name: name.to_string(),
            fingerprint: fingerprint(args),
        };
        let mut entries = self.entries.lock().unwrap();
        if let Some(weak) = entries.get(&key) {
            if let Some(arc) = weak.upgrade() {
                return arc;
            }
        }
        let arc = construct();
        entries.insert(key, Arc::downgrade(&arc));
        // Keys embed block URIs, so over a long session the map accumulates
        // entries for providers that died and are never requested again —
        // sweep dead Weaks while we already hold the lock.
        entries.retain(|_, weak| weak.strong_count() > 0);
        arc
    }
}

impl Default for ProviderCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonicalise a `ResolvedArgs` into a deterministic 64-bit structural
/// fingerprint. Named / template entries are hashed key-sorted so map
/// iteration order does not affect the result.
fn fingerprint(args: &ResolvedArgs) -> u64 {
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();

    args.positional.len().hash(&mut h);
    for v in &args.positional {
        hash_value(v, &mut h);
    }

    let mut named: Vec<_> = args.named.iter().collect();
    named.sort_by(|a, b| a.0.cmp(b.0));
    named.len().hash(&mut h);
    for (k, v) in named {
        k.hash(&mut h);
        hash_value(v, &mut h);
    }

    let mut templates: Vec<_> = args.templates.iter().collect();
    templates.sort_by(|a, b| a.0.cmp(b.0));
    templates.len().hash(&mut h);
    for (k, e) in templates {
        k.hash(&mut h);
        hash_expr(e, &mut h);
    }

    // Note: `args.rows` intentionally excluded — provider Arcs
    // already share identity via the cache, so including them in the
    // key would just hash trait-object pointers.
    h.finish()
}

/// Structural hash of a `Value`. Discriminant-tagged; `Object` entries
/// key-sorted; floats by `to_bits` (NaN payloads distinguish, which only
/// splits cache entries — never aliases them).
fn hash_value(v: &holon_api::Value, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;

    use holon_api::Value;
    std::mem::discriminant(v).hash(h);
    match v {
        Value::String(s) | Value::DateTime(s) | Value::Json(s) => s.hash(h),
        Value::Integer(i) => i.hash(h),
        Value::Float(f) => f.to_bits().hash(h),
        Value::Boolean(b) => b.hash(h),
        Value::Array(items) => {
            items.len().hash(h);
            for item in items {
                hash_value(item, h);
            }
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            entries.len().hash(h);
            for (k, val) in entries {
                k.hash(h);
                hash_value(val, h);
            }
        }
        Value::Null => {}
    }
}

/// Structural hash of a `RenderExpr` (the templates side of the key).
fn hash_expr(e: &holon_api::render_types::RenderExpr, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;

    use holon_api::render_types::RenderExpr;
    std::mem::discriminant(e).hash(h);
    match e {
        RenderExpr::FunctionCall { name, args } => {
            name.hash(h);
            args.len().hash(h);
            for arg in args {
                arg.name.hash(h);
                hash_expr(&arg.value, h);
            }
        }
        RenderExpr::LiveBlock { block_id } => block_id.hash(h),
        RenderExpr::ColumnRef { name } => name.hash(h),
        RenderExpr::Literal { value } => hash_value(value, h),
        RenderExpr::BinaryOp { op, left, right } => {
            std::mem::discriminant(op).hash(h);
            hash_expr(left, h);
            hash_expr(right, h);
        }
        RenderExpr::Array { items } => {
            items.len().hash(h);
            for item in items {
                hash_expr(item, h);
            }
        }
        RenderExpr::Object { fields } => {
            let mut entries: Vec<_> = fields.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            entries.len().hash(h);
            for (k, expr) in entries {
                k.hash(h);
                hash_expr(expr, h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use futures_signals::signal_vec::MutableVec;
    use futures_signals::signal_vec::SignalVec;
    use holon_api::Value;
    use holon_api::widget_spec::DataRow;

    use super::*;

    struct DummyProvider;
    impl ReactiveRowProvider for DummyProvider {
        fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
            Vec::new()
        }
        fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
            Box::pin(MutableVec::<Arc<DataRow>>::new().signal_vec_cloned())
        }
        fn keyed_rows_signal_vec(
            &self,
        ) -> Pin<Box<dyn SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> + Send>> {
            Box::pin(MutableVec::<(holon_api::RowKey, Arc<DataRow>)>::new().signal_vec_cloned())
        }
        fn cache_identity(&self) -> u64 {
            holon_api::ptr_identity(self)
        }
    }

    fn empty_args() -> ResolvedArgs {
        ResolvedArgs {
            positional: vec![],
            positional_exprs: vec![],
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
        }
    }

    #[test]
    fn cache_reuses_arc_for_same_key() {
        let cache = ProviderCache::new();
        let args = empty_args();
        let a = cache.get_or_create("test", &args, || Arc::new(DummyProvider));
        let b = cache.get_or_create("test", &args, || Arc::new(DummyProvider));
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn cache_constructs_fresh_after_all_drops() {
        let cache = ProviderCache::new();
        let args = empty_args();
        let id_1 = {
            let a = cache.get_or_create("test", &args, || Arc::new(DummyProvider));
            a.cache_identity()
        };
        let id_2 = {
            let a = cache.get_or_create("test", &args, || Arc::new(DummyProvider));
            a.cache_identity()
        };
        // Fresh Arc; different address — Weak failed to upgrade.
        assert_ne!(id_1, id_2);
    }

    #[test]
    fn fingerprint_order_independent_for_named_args() {
        let mut a = empty_args();
        a.named.insert("alpha".into(), Value::Integer(1));
        a.named.insert("beta".into(), Value::Integer(2));

        let mut b = empty_args();
        b.named.insert("beta".into(), Value::Integer(2));
        b.named.insert("alpha".into(), Value::Integer(1));

        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn fingerprint_sensitive_to_positional_order() {
        let mut a = empty_args();
        a.positional = vec![Value::Integer(1), Value::Integer(2)];

        let mut b = empty_args();
        b.positional = vec![Value::Integer(2), Value::Integer(1)];

        assert_ne!(fingerprint(&a), fingerprint(&b));
    }
}
