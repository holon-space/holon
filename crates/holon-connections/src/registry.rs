//! Which list connections a build configured, and the one operation provider
//! that serves all of them.
//!
//! A transport — HTTP, a socket, a fixture — is the one thing this crate cannot
//! own, so it arrives as a [`ConfiguredLists`] that a build with a transport
//! provides. Everything else about a connection (its sidecar's declaration,
//! compiled against the declared type; the rows it mirrors; the clock) is
//! assembled behind that value, and `remote_list_sync` is registered ONCE for
//! every connection rather than once per peer.
//!
//! A build with no transport provides nothing; the composition root then
//! registers a provider that serves no operations, which is why this
//! registration can live in the graph the wasm images share rather than only in
//! the desktop one.

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::storage::types::StorageEntity;

use crate::provider::RemoteListOperations;
use crate::provider::RoundClock;
use crate::snapshot::LocalRowReader;
use crate::spec::CompiledListSync;
use crate::sync::RemoteListPeer;

/// One connection a build has configured, ready to be served.
///
/// The spec is already compiled: compiling needs the declared type, and the
/// crate that owns the type registry is the one that knows a connection is
/// configured at all — so the proof that a spec matches its type is carried
/// here rather than re-derived every round.
#[derive(Clone)]
pub struct ConfiguredList {
    /// The connection's name, for its error messages.
    pub connection: String,
    pub compiled: Arc<CompiledListSync>,
    pub peer: Arc<dyn RemoteListPeer>,
    pub rows: Arc<dyn LocalRowReader>,
    pub device_id: String,
}

/// Every list connection a build configured, as a transport wiring provides it.
///
/// `refusals` is the other half of that answer and is not diagnostics: a
/// sidecar that declares a connection whose spec does not compile is a
/// connection this build cannot serve, and a dispatch that finds no operation
/// must say so by name rather than report an empty configuration.
#[derive(Clone, Default)]
pub struct ConfiguredLists {
    lists: Vec<ConfiguredList>,
    refusals: Vec<String>,
}

impl ConfiguredLists {
    pub fn new(lists: Vec<ConfiguredList>, refusals: Vec<String>) -> Self {
        Self { lists, refusals }
    }

    pub fn lists(&self) -> &[ConfiguredList] {
        &self.lists
    }

    pub fn refusals(&self) -> &[String] {
        &self.refusals
    }

    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }
}

/// The one provider registered for every configured connection.
///
/// It exists so that a connection needs no registration of its own: adding a
/// sidecar adds a `remote_list_sync`, and `OperationProvider::operations`
/// answers with the connections that are actually there.
pub struct ConfiguredRemoteLists {
    operations: Vec<RemoteListOperations>,
    refusals: Vec<String>,
}

impl ConfiguredRemoteLists {
    pub fn new(clock: Arc<dyn RoundClock>, configured: ConfiguredLists) -> Self {
        let operations = configured
            .lists
            .into_iter()
            .map(|list| {
                RemoteListOperations::new(
                    list.connection,
                    list.compiled,
                    list.peer,
                    list.rows,
                    clock.clone(),
                    list.device_id,
                )
            })
            .collect();
        Self {
            operations,
            refusals: configured.refusals,
        }
    }

    /// The connections this provider serves, for a wiring that wants to assert
    /// what it configured.
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

#[async_trait]
impl OperationProvider for ConfiguredRemoteLists {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        self.operations
            .iter()
            .flat_map(|operation| operation.operations())
            .collect()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        entity: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        let serving: Vec<&RemoteListOperations> = self
            .operations
            .iter()
            .filter(|operation| {
                operation
                    .operations()
                    .iter()
                    .any(|d| d.entity_name == *entity_name && d.name == op_name)
            })
            .collect();
        match serving.as_slice() {
            [one] => one.execute_operation(entity_name, op_name, entity).await,
            [] => Err(self.no_route(entity_name, op_name).into()),
            many => Err(format!(
                "{} configured connections serve {entity_name}/{op_name}, and a dispatch must \
                 reach exactly one: {}",
                many.len(),
                self.describe()
            )
            .into()),
        }
    }
}

impl ConfiguredRemoteLists {
    fn describe(&self) -> String {
        if self.operations.is_empty() {
            return "none".to_string();
        }
        self.operations
            .iter()
            .map(|operation| operation.connection().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// No connection serves this dispatch. The refusals are named because the
    /// usual reason a connection is missing is that its sidecar was rejected,
    /// and "none configured" without them reads as "nothing was written".
    fn no_route(&self, entity_name: &EntityName, op_name: &str) -> String {
        let mut message = format!(
            "no configured connection serves {entity_name}/{op_name}; the connections this build \
             reached are: {}",
            self.describe()
        );
        for refusal in &self.refusals {
            message.push_str(&format!("\nrefused: {refusal}"));
        }
        message
    }
}
