//! A read under the doc lock costs time linear in what it reads. A writer
//! waits for every reader to drain, so a reader whose cost grows with the
//! square of the store stalls every write behind it.
//!
//! The hold is the duration of the `read_hold` span the entity store opens
//! around its read lock. Every number names its build profile: the test
//! profile is about 11x slower than release, the ratio between sizes is
//! profile-independent.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityName;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_core::TypeWriteAuthority;
use holon_core::storage::types::StorageEntity;
use holon_core::type_write_rule::ScopeReplacement;
use holon_loro::loro_document::LoroDocument;
use holon_loro::loro_entity_operations::LoroTypeWriteAuthority;
use tracing::Subscriber;
use tracing::span;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

const TYPE: &str = "cookware";
const SMALL: usize = 500;
const GROWTH: usize = 4;
/// Linear growth gives 4x, quadratic 16x; the bound leaves room for noise.
const LINEAR_BOUND: f64 = 8.0;
const REPEATS: usize = 5;
const SCOPE_ROWS: usize = 5;
const PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

fn type_def() -> TypeDefinition {
    serde_json::from_value(serde_json::json!({
        "name": TYPE,
        "primary_key": "id",
        "fields": [
            {"name": "id", "sql_type": "TEXT", "primary_key": true},
            {"name": "recipe_id", "sql_type": "TEXT"},
            {"name": "name", "sql_type": "TEXT", "nullable": true},
        ],
    }))
    .expect("the test type parses")
}

fn scope(owner: &str, rows: usize) -> StorageEntity {
    let rows = (0..rows)
        .map(|n| {
            StorageEntity::from([
                (
                    "id".into(),
                    Value::String(format!("{}:{owner}-{n}", EntityName::new(TYPE).as_str())),
                ),
                ("recipe_id".into(), Value::String(owner.to_string())),
                ("name".into(), Value::Null),
            ])
        })
        .collect();
    ScopeReplacement::new("recipe_id".to_string(), owner.to_string(), rows)
        .expect("the scope holds its rows")
        .params()
}

/// The closed spans named `read_hold`, with their durations.
#[derive(Clone, Default)]
struct Holds(Arc<Mutex<Vec<(String, Duration)>>>);

struct Opened(Instant);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Holds {
    fn on_new_span(&self, _: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        ctx.span(id)
            .expect("a new span is registered")
            .extensions_mut()
            .insert(Opened(Instant::now()));
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("a closing span is registered");
        let opened = span
            .extensions()
            .get::<Opened>()
            .expect("every span records its start")
            .0;
        self.0
            .lock()
            .expect("the hold list is not poisoned")
            .push((span.name().to_string(), opened.elapsed()));
    }
}

#[derive(Clone, Copy, Debug)]
enum Read {
    ReplaceScope,
    ReadAll,
}

impl Read {
    fn span(self) -> &'static str {
        match self {
            Read::ReplaceScope => "loro.replace_scope.read_hold",
            Read::ReadAll => "loro.read_all.read_hold",
        }
    }
}

/// The fastest of `REPEATS` holds of `read` against a store of `n` entities.
fn hold(read: Read, n: usize) -> Duration {
    let holds = Holds::default();
    let subscriber = tracing_subscriber::registry().with(holds.clone());
    tracing::subscriber::with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let doc = Arc::new(LoroDocument::new("global".into()).unwrap());
            let authority = LoroTypeWriteAuthority::new(doc, None);
            let def = type_def();
            let ops = authority.for_type(&def).unwrap();
            let entity = EntityName::new(TYPE);
            ops.execute_operation(&entity, ScopeReplacement::OP, scope("seed", n))
                .await
                .unwrap();
            holds.0.lock().unwrap().clear();
            for _ in 0..REPEATS {
                match read {
                    Read::ReplaceScope => {
                        ops.execute_operation(
                            &entity,
                            ScopeReplacement::OP,
                            scope("probe", SCOPE_ROWS),
                        )
                        .await
                        .unwrap();
                    }
                    Read::ReadAll => {
                        let rows = authority.read_all(&def).await.unwrap();
                        assert_eq!(rows.len(), n, "read_all returns every live entity");
                    }
                }
            }
        })
    });
    let spans = holds.0.lock().unwrap();
    let mine: Vec<Duration> = spans
        .iter()
        .filter(|(name, _)| name == read.span())
        .map(|(_, d)| *d)
        .collect();
    assert_eq!(
        mine.len(),
        REPEATS,
        "the read must open its hold span once per call; saw {spans:?}"
    );
    mine.into_iter().min().unwrap()
}

fn assert_linear(read: Read) {
    let small = hold(read, SMALL);
    let large = hold(read, SMALL * GROWTH);
    let ratio = large.as_secs_f64() / small.as_secs_f64();
    println!(
        "{read:?} read-lock hold [{PROFILE}]: N={SMALL} {small:?}, N={} {large:?}, ratio {ratio:.1}x",
        SMALL * GROWTH
    );
    assert!(
        ratio < LINEAR_BOUND,
        "{read:?}: quadrupling the store multiplied the read-lock hold by {ratio:.1}x \
         ({small:?} -> {large:?}, {PROFILE}); a linear read stays under {LINEAR_BOUND}x"
    );
}

#[test]
fn replace_scope_holds_the_read_lock_in_time_linear_in_the_store() {
    assert_linear(Read::ReplaceScope);
}

#[test]
fn read_all_holds_the_read_lock_in_time_linear_in_the_store() {
    assert_linear(Read::ReadAll);
}
