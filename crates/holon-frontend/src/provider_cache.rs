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
use std::sync::{Arc, Mutex, Weak};

use holon_api::render_eval::ResolvedArgs;
use holon_api::ReactiveRowProvider;

/// Cache key — function name + fingerprint of resolved args.
///
/// The fingerprint is a `String` derived from
/// `{positional} | {named (sorted)} | {templates (sorted)}` using
/// `Debug` formatting. "Templates-by-debug-string" is acknowledged
/// technical debt — good enough for v1 cache hits, to be replaced by
/// a structural `RenderExpr` hash in a follow-up PR.
#[derive(PartialEq, Eq, Hash, Debug)]
struct ProviderKey {
    name: String,
    fingerprint: String,
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
        arc
    }
}

impl Default for ProviderCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonicalise a `ResolvedArgs` into a deterministic fingerprint
/// string. Named / template entries are sorted by key so map
/// iteration order does not affect the hash. `Debug` formatting
/// normalises numeric / string variants through `Value`'s own
/// `PartialEq`-aligned `Debug` impl.
fn fingerprint(args: &ResolvedArgs) -> String {
    let mut parts: Vec<String> = args.positional.iter().map(|v| format!("{v:?}")).collect();

    let mut named: Vec<_> = args.named.iter().collect();
    named.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in named {
        parts.push(format!("{k}={v:?}"));
    }

    let mut templates: Vec<_> = args.templates.iter().collect();
    templates.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in templates {
        parts.push(format!("tmpl:{k}={v:?}"));
    }

    // Note: `args.rows` intentionally excluded — provider Arcs
    // already share identity via the cache, so including them in the
    // key would just hash trait-object pointers.
    parts.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_signals::signal_vec::{MutableVec, SignalVec, SignalVecExt};
    use holon_api::widget_spec::DataRow;
    use holon_api::Value;
    use std::pin::Pin;

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
        ) -> Pin<Box<dyn SignalVec<Item = (String, Arc<DataRow>)> + Send>> {
            Box::pin(MutableVec::<(String, Arc<DataRow>)>::new().signal_vec_cloned())
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
