//! The composed keystone's remote-list SUT component: the fixture peer plus
//! the REAL production round over the REAL mirror table.
//!
//! The peer is the only test double. The round is dispatched as the
//! `remote_list_sync` OPERATION through the production dispatcher — the same
//! route a frontend takes — so the operation's registration and its entity-name
//! routing are covered too, and the local writes travel back as the
//! operation's own follow-ups rather than being rebuilt here. The reconciler,
//! the sidecar declaration and the mirror read are production as well, so a
//! round that stops reconciling fails the mirror invariant instead of passing a
//! reimplementation.

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_connections::CompiledListSync;
use holon_connections::LocalRowReader;
use holon_connections::REMOTE_LIST_SYNC;
use holon_connections::RemoteListOperations;
use holon_connections::RemoteListPeer;
use holon_connections::RoundClock;
use holon_connections_testing::CacheMode;
use holon_connections_testing::ChangeDetection;
use holon_connections_testing::CommitGranularity;
use holon_connections_testing::FixtureListPeer;
use holon_connections_testing::FixtureProfile;
use holon_connections_testing::KeyShape;
use holon_connections_testing::ListMutation;
use holon_pbt_core::capabilities::RemoteListMutation;
use holon_pbt_core::capabilities::RemoteListRoundOutcome;
use holon_pbt_core::capabilities::SutRemoteListSync;

use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::remote_list_fixture;

/// The connection this fixture declares, and the name the operation is
/// registered under.
const CONNECTION: &str = "remote-list";

/// The round timestamp a sync mints command ids under. Fixed so two rounds are
/// reproducible from their inputs.
const NOW_MS: i64 = 1_000_000;

/// A clock pinned to [`NOW_MS`]: the production operation reads the wall clock
/// through this port, and a round whose command ids move with it would not be
/// reproducible from its inputs.
struct FixedRoundClock;

impl RoundClock for FixedRoundClock {
    fn now_ms(&self) -> i64 {
        NOW_MS
    }
}

/// The fixture peer the composed keystone owns: a content-keyed snapshot peer
/// with a cache-bust requirement — the harder of the two shapes the reconciler
/// declares for.
fn profile() -> FixtureProfile {
    FixtureProfile {
        key_shape: KeyShape::NaturalKey,
        change_detection: ChangeDetection::FullSnapshot,
        commit_granularity: CommitGranularity::PerRow,
        cache_mode: CacheMode::CachedNeedsBust,
    }
}

pub struct RemoteListSut {
    compiled: Arc<CompiledListSync>,
    peer: Arc<FixtureListPeer>,
    rows: Arc<holon_app::remote_list::SqlMirrorRows>,
    component: Arc<HeadlessFrontendComponent>,
}

impl RemoteListSut {
    /// Declare the connection's type through the production admission seat,
    /// compile its sidecar block and build the fixture peer over it.
    pub async fn boot(component: Arc<HeadlessFrontendComponent>) -> Arc<Self> {
        let type_def = remote_list_fixture::type_definition();
        let profiles = holon_capability::registry::shipped_profiles()
            .expect("the shipped capability profiles must parse");
        let registry = component
            .injector()
            .resolve::<holon_profiles::TypeRegistry>();
        holon_app::type_admission::declare_type_admitted(
            &profiles,
            &type_def,
            component.engine().db_handle(),
            &registry,
            &component.engine().get_dispatcher(),
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "remote-list fixture: declaring '{}' failed: {e}",
                remote_list_fixture::ENTITY
            )
        });
        let compiled =
            CompiledListSync::compile(CONNECTION, remote_list_fixture::list_sync_spec(), &type_def)
                .expect("the remote-list connection compiles");
        let rows = Arc::new(holon_app::remote_list::SqlMirrorRows::new(
            component.engine().db_handle().clone(),
            &compiled,
        ));
        // The peer persists its list independently of Holon's restarts, so a
        // reboot must not reset it. Seed the rebuilt peer from the mirror the
        // store kept — the two were in sync before the restart, so the mirror is
        // the peer's own list back again. (A first boot reads an empty mirror.)
        let seed = Self::peer_seed_from_mirror(&rows).await;
        let peer = Arc::new(FixtureListPeer::seeded(
            compiled.clone(),
            profile(),
            seed,
            0,
        ));
        let device_id = holon_app::remote_list::device_id().to_string();
        // Register the ONE generic operation a configured list connection owns,
        // over this fixture's transport. The keystone then dispatches a round
        // the way a frontend does, so an unregistered or mis-named
        // `remote_list_sync` is a keystone failure rather than a silent green.
        component
            .engine()
            .get_dispatcher()
            .register_provider(Arc::new(RemoteListOperations::new(
                CONNECTION,
                compiled.clone(),
                peer.clone() as Arc<dyn RemoteListPeer>,
                rows.clone() as Arc<dyn LocalRowReader>,
                Arc::new(FixedRoundClock),
                device_id,
            )))
            .unwrap_or_else(|e| {
                panic!("registering the '{CONNECTION}' remote-list operation failed: {e}")
            });
        Arc::new(Self {
            compiled,
            peer,
            rows,
            component,
        })
    }

    /// The peer's list, read back out of the surviving mirror table: the
    /// peer-authoritative columns only, in the peer's own value spellings.
    async fn peer_seed_from_mirror(
        rows: &holon_app::remote_list::SqlMirrorRows,
    ) -> Vec<holon_connections::RemoteRow> {
        let mirror = rows
            .load()
            .await
            .expect("reading the mirror to seed the peer");
        mirror
            .into_iter()
            .map(|row| {
                let mut columns = BTreeMap::new();
                for col in &remote_list_fixture::PROJECTION[1..] {
                    let Some(value) = row.columns.get(*col) else {
                        continue;
                    };
                    // A REAL merge column reads the peer's whole-number value
                    // back as a float; the peer serves it as an integer.
                    let value = match value {
                        Value::Float(f) => Value::Integer(*f as i64),
                        other => other.clone(),
                    };
                    columns.insert((*col).to_string(), value);
                }
                holon_connections::RemoteRow {
                    id: row.id,
                    columns,
                }
            })
            .collect()
    }

    /// The typed remote row a mutation's columns describe: the
    /// peer-authoritative columns under their declared types.
    fn typed_remote_row(&self, columns: &[(String, String)]) -> holon_connections::RemoteRow {
        let cell = |name: &str| {
            columns
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("a remote-list mutation lacks the '{name}' column"))
        };
        let label = cell("label");
        let bucket = cell("bucket");
        let id = remote_list_fixture::row_id(&label, &bucket);
        let mut cols = BTreeMap::new();
        cols.insert("id".into(), Value::String(id.clone()));
        cols.insert("label".into(), Value::String(label));
        cols.insert("bucket".into(), Value::String(bucket));
        cols.insert(
            "rank".into(),
            Value::Integer(
                cell("rank")
                    .parse()
                    .expect("the remote-list rank is a whole number"),
            ),
        );
        cols.insert(
            "done".into(),
            Value::Integer(
                cell("done")
                    .parse()
                    .expect("the remote-list done flag is 0 or 1"),
            ),
        );
        holon_connections::RemoteRow { id, columns: cols }
    }

    fn key_of(&self, columns: &[(String, String)]) -> holon_connections::RowKey {
        let map: BTreeMap<String, Value> = columns
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let doc = serde_json::to_value(&map).expect("mutation columns serialize");
        self.compiled
            .key_of(&doc)
            .expect("the mutation derives its key")
    }

    fn canonical_cell(value: Option<&Value>) -> String {
        match value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Integer(n)) => n.to_string(),
            // A REAL column reads a whole-number peer value back as a float; the
            // oracle stores the same number as its generator string, so whole
            // floats render as integers here to compare equal.
            Some(Value::Float(f)) if f.fract() == 0.0 => format!("{f:.0}"),
            Some(Value::Float(f)) => f.to_string(),
            Some(Value::Boolean(b)) => b.to_string(),
            Some(Value::Null) | None => String::new(),
            Some(other) => format!("{other:?}"),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutRemoteListSync for RemoteListSut {
    async fn remote_list_mutate(&self, mutation: RemoteListMutation) {
        match &mutation {
            RemoteListMutation::Add { columns } => {
                let row = self.typed_remote_row(columns);
                self.peer.mutate(&ListMutation::Add(row));
            }
            RemoteListMutation::Remove { columns } => {
                let key = self.key_of(columns);
                self.peer.mutate(&ListMutation::Remove { key });
            }
        }
    }

    async fn remote_list_sync_round(&self) -> RemoteListRoundOutcome {
        // The round's push count, read from the peer it pushed to — the remote
        // end of the round, not the round's self-report.
        let before = self.peer.commands_applied();
        self.component
            .engine()
            .get_dispatcher()
            .execute_operation_with_provenance(
                &EntityName::new(remote_list_fixture::ENTITY),
                REMOTE_LIST_SYNC,
                holon_api::StorageEntity::default(),
                holon::api::operation_dispatcher::AuthoredInput::Verbatim,
                // A peer exchange is machine-authored: it must not enter the
                // user's undo stack. The operation's own follow-ups are the
                // local writes, and the dispatcher runs them.
                OpOrigin::Sync,
            )
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "dispatching {}/{REMOTE_LIST_SYNC} failed: {e:#}",
                    remote_list_fixture::ENTITY
                )
            });
        RemoteListRoundOutcome {
            committed: self.peer.commands_applied() - before,
        }
    }

    async fn remote_list_mirror_rows(&self) -> Vec<Vec<String>> {
        let rows = self
            .rows
            .load()
            .await
            .expect("reading the remote-list mirror table");
        let mut out: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                remote_list_fixture::PROJECTION
                    .iter()
                    .map(|col| Self::canonical_cell(row.columns.get(*col)))
                    .collect()
            })
            .collect();
        out.sort();
        out
    }
}
