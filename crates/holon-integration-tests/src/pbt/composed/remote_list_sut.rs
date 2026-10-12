//! The composed keystone's remote-list SUT component: the shipped shopping
//! connection over the REAL production round and the REAL mirror table.
//!
//! The list app is the only test double (see
//! [`crate::pbt::remote_list_fixture`]); the peer the round calls is the
//! production `RestListPeer` running the sidecar's own mappings. The round is
//! dispatched as the `remote_list_sync` OPERATION through the production
//! dispatcher — the same route a frontend takes — so the operation's
//! registration and its entity-name routing are covered too, and the local
//! writes travel back as the operation's own follow-ups rather than being
//! rebuilt here.

use std::sync::Arc;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_app::remote_list::RestListPeer;
use holon_connections::CompiledListSync;
use holon_connections::LocalRowReader;
use holon_connections::REMOTE_LIST_SYNC;
use holon_connections::RemoteListOperations;
use holon_connections::RemoteListPeer;
use holon_connections::RoundClock;
use holon_core::OperationProvider;
use holon_pbt_core::capabilities::RemoteListMutation;
use holon_pbt_core::capabilities::RemoteListRoundOutcome;
use holon_pbt_core::capabilities::SutRemoteListSync;

use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::remote_list_fixture;
use crate::pbt::remote_list_fixture::AppEntry;
use crate::pbt::remote_list_fixture::FakeAppSurface;
use crate::pbt::remote_list_fixture::FakeListApp;

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

pub struct RemoteListSut {
    app: Arc<FakeListApp>,
    rows: Arc<holon_app::remote_list::AuthorityRows>,
    component: Arc<HeadlessFrontendComponent>,
}

impl RemoteListSut {
    /// Compile the shipped connection against the `shopping_item` type the
    /// boot registered, and register its round over the fake app.
    pub async fn boot(component: Arc<HeadlessFrontendComponent>) -> Arc<Self> {
        let type_def = component
            .injector()
            .resolve::<holon_profiles::TypeRegistry>()
            .get(remote_list_fixture::ENTITY)
            .unwrap_or_else(|| {
                panic!(
                    "the boot registered no '{}' type for the remote-list fixture",
                    remote_list_fixture::ENTITY
                )
            });
        let compiled = CompiledListSync::compile(
            remote_list_fixture::CONNECTION,
            remote_list_fixture::list_sync_spec(),
            &type_def,
        )
        .expect("the shipped shopping connection compiles");
        let rows = Arc::new(holon_app::remote_list::AuthorityRows::new(
            component
                .injector()
                .resolve_async::<dyn holon_core::TypeWriteAuthority>()
                .await,
            type_def,
        ));
        let app = component.remote_list_app();
        let peer = RestListPeer::new(
            Arc::new(FakeAppSurface::new(app.clone())),
            compiled.clone(),
            holon_app::remote_list::device_id(),
        );
        component
            .engine()
            .get_dispatcher()
            .register_provider(Arc::new(RemoteListOperations::new(
                remote_list_fixture::CONNECTION,
                compiled,
                Arc::new(peer) as Arc<dyn RemoteListPeer>,
                rows.clone() as Arc<dyn LocalRowReader>,
                Arc::new(FixedRoundClock),
                holon_app::remote_list::device_id(),
            )))
            .unwrap_or_else(|e| {
                panic!(
                    "registering the '{}' remote-list operation failed: {e}",
                    remote_list_fixture::CONNECTION
                )
            });
        Arc::new(Self {
            app,
            rows,
            component,
        })
    }

    fn cell(columns: &[(String, String)], name: &str) -> String {
        columns
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("a remote-list mutation lacks the '{name}' column"))
    }

    fn entry(columns: &[(String, String)]) -> AppEntry {
        AppEntry {
            name: Self::cell(columns, "name"),
            cat: Self::cell(columns, "cat"),
            count: Some(Self::cell(columns, "count")),
        }
    }

    fn id_of(columns: &[(String, String)]) -> String {
        remote_list_fixture::row_id(&Self::cell(columns, "name"), &Self::cell(columns, "cat"))
    }

    async fn execute(&self, op: &str, params: holon_api::StorageEntity, id: &str) {
        self.component
            .engine()
            .get_dispatcher()
            .execute_operation(&EntityName::new(remote_list_fixture::ENTITY), op, params)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{op} on {} '{id}' through the dispatcher failed: {e:#}",
                    remote_list_fixture::ENTITY
                )
            });
    }

    async fn edit_in_holon(&self, columns: &[(String, String)]) {
        let id = Self::id_of(columns);
        for (column, value) in columns {
            if remote_list_fixture::IDENTITY_COLUMNS.contains(&column.as_str()) {
                continue;
            }
            let mut params = holon_api::StorageEntity::default();
            params.insert("id".into(), Value::String(id.clone()));
            params.insert("field".into(), Value::String(column.clone()));
            params.insert("value".into(), Value::String(value.clone()));
            self.execute("set_field", params, &id).await;
        }
    }

    fn edit_in_app(&self, columns: &[(String, String)]) {
        let name = Self::cell(columns, "name");
        let amount = Self::cell(columns, "count");
        self.app.change(|list| list.edit_amount(&name, &amount));
    }

    /// The text entries of a sync response's `field` list.
    fn response_list(response: &Value, field: &str) -> Vec<String> {
        response
            .as_object()
            .and_then(|response| response.get(field))
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("a sync response carries no `{field}` list: {response:?}"))
            .iter()
            .map(|entry| match entry {
                Value::String(text) => text.clone(),
                other => panic!("a sync response's `{field}` holds a non-text entry: {other:?}"),
            })
            .collect()
    }

    /// A mirrored cell as the oracle spells it: text verbatim, a latch as
    /// `0`/`1` whichever way the column reads back.
    fn canonical_cell(value: Option<&Value>) -> String {
        match value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Integer(n)) => n.to_string(),
            Some(Value::Boolean(b)) => u8::from(*b).to_string(),
            Some(Value::Null) | None => String::new(),
            Some(other) => panic!("a mirrored cell of an unexpected kind: {other:?}"),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutRemoteListSync for RemoteListSut {
    async fn remote_list_mutate(&self, mutation: RemoteListMutation) {
        match &mutation {
            RemoteListMutation::Add { columns } => {
                let ticked = Self::cell(columns, "checked") == "1";
                self.app
                    .change(|list| list.add(Self::entry(columns), ticked));
            }
            RemoteListMutation::Remove { columns } => {
                self.app
                    .change(|list| list.remove(&Self::cell(columns, "name")));
            }
            RemoteListMutation::AddDuplicate { columns }
            | RemoteListMutation::AddActiveOverTicked { columns } => {
                self.app
                    .change(|list| list.add(Self::entry(columns), false));
            }
            RemoteListMutation::EditRemotely { columns } => self.edit_in_app(columns),
            RemoteListMutation::UntickRemotely { columns } => {
                self.app
                    .change(|list| list.untick(&Self::cell(columns, "name")));
            }
            RemoteListMutation::TickLocally { columns } => {
                let id = Self::id_of(columns);
                let mut params = holon_api::StorageEntity::default();
                params.insert("id".into(), Value::String(id.clone()));
                params.insert("field".into(), Value::String("checked".into()));
                params.insert("value".into(), Value::Integer(1));
                self.execute("set_field", params, &id).await;
            }
            RemoteListMutation::EditBoth { columns, peer } => {
                self.edit_in_app(peer);
                self.edit_in_holon(columns).await;
            }
            RemoteListMutation::AuthorLocally { columns } => {
                let id = Self::id_of(columns);
                let mut params = holon_api::StorageEntity::default();
                params.insert("id".into(), Value::String(id.clone()));
                for (column, value) in columns {
                    let value = match column.as_str() {
                        "checked" => Value::Integer(
                            value
                                .parse()
                                .expect("the remote-list checked flag is 0 or 1"),
                        ),
                        _ => Value::String(value.clone()),
                    };
                    params.insert(column.as_str().into(), value);
                }
                self.execute("create", params, &id).await;
            }
            RemoteListMutation::EditLocally { columns } => self.edit_in_holon(columns).await,
            RemoteListMutation::DeleteLocally { columns } => {
                let id = Self::id_of(columns);
                let mut params = holon_api::StorageEntity::default();
                params.insert("id".into(), Value::String(id.clone()));
                self.execute("delete", params, &id).await;
            }
        }
    }

    async fn remote_list_sync_round(&self) -> RemoteListRoundOutcome {
        // The round's push count, read from the peer it pushed to — the remote
        // end of the round, not the round's self-report.
        let before = self.app.commands_applied();
        let result = self
            .component
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
        let (refused, overwritten) = match &result.response {
            None => (0, Vec::new()),
            Some(response) => (
                Self::response_list(response, "refused").len(),
                Self::response_list(response, "overwritten"),
            ),
        };
        RemoteListRoundOutcome {
            committed: self.app.commands_applied() - before,
            refused,
            overwritten,
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

    async fn remote_list_peer_entries(&self) -> Vec<Vec<String>> {
        self.app.list().entries()
    }
}
