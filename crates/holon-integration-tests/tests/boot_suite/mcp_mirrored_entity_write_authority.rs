//! Who writes an entity a connector mirrors, on every boot.
//!
//! A sidecar entity reaches the type registry as a free-standing type (its id
//! references nothing and it has persisted columns of its own), and the boot
//! sequence derives a SQL write authority for every free-standing type. Two
//! opposite mistakes live here, so both directions are asserted: deriving one
//! for an entity the connector WRITES gives the mirror table a local writer the
//! system of record never sees, and withholding one from an entity the
//! connector only READS leaves it with no writer at all.
//!
//! Both boots are asserted because the reported symptom arrived on an existing
//! vault, and a declaration that survived in the database would only show up
//! the second time.
//!
//! @pbt kind harness
//! @pbt covers mcp-mirrored-entity-write-authority — write authority for
//! mirrored entities is decided by what the connector writes, across a restart
//! @pbt overlaps general_e2e_composed_pbt — kept: the composed SUT implements
//! no `SutAppLifecycle`, so `StartApp` is cap-gated out of its alphabet and no
//! transition sequence boots a connector

use std::sync::Arc;

use holon::api::operation_dispatcher::OperationDispatcher;
use holon_api::EntityName;
use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn dispatcher(env: &TestEnvironment) -> Arc<OperationDispatcher> {
    env.injector()
        .expect("session must be running")
        .resolve_async::<OperationDispatcher>()
        .await
}

/// The op names each registered provider advertises for `entity`, one entry per
/// provider that claims it.
async fn authorities_for(env: &TestEnvironment, entity: &str) -> Vec<Vec<String>> {
    let entity = EntityName::new(entity);
    dispatcher(env)
        .await
        .providers()
        .iter()
        .filter_map(|provider| {
            let ops: Vec<String> = provider
                .operations()
                .iter()
                .filter(|op| op.entity_name == entity)
                .map(|op| op.name.clone())
                .collect();
            (!ops.is_empty()).then_some(ops)
        })
        .collect()
}

async fn assert_authorities(env: &TestEnvironment, boot: &str) {
    let claimed = authorities_for(env, "fake_probe").await;
    assert_eq!(
        claimed.len(),
        1,
        "[{boot}] 'fake_probe' is mirrored from the fake connector, so the connector must be its \
         only write authority; found {} providers claiming it: {claimed:?}",
        claimed.len()
    );
    assert!(
        claimed[0].iter().any(|op| op == "update_probe"),
        "[{boot}] the surviving authority for 'fake_probe' is not the connector — it advertises \
         {:?}, which does not include the connector's own tool",
        claimed[0]
    );

    // Two mirrored entities the connector does not WRITE. 'fake_shadow' has no
    // tool at all; 'fake_readonly' has a read tool, so it IS routable and would
    // satisfy a guard that only asked "does any provider claim this entity?".
    // Both must keep the authority derived from their columns — otherwise the
    // entity is left with no way to be written and, worse, the boot-time
    // capability check that would have said so never runs.
    for entity in ["fake_shadow", "fake_readonly"] {
        let unclaimed = authorities_for(env, entity).await;
        let ops: Vec<&String> = unclaimed.iter().flatten().collect();
        assert!(
            ops.iter().any(|op| *op == "set_field")
                && ops.iter().any(|op| *op == "create")
                && ops.iter().any(|op| *op == "delete"),
            "[{boot}] the connector does not write '{entity}', so the authority derived from its \
             columns must serve it; ops present: {ops:?}"
        );
        dispatcher(env)
            .await
            .assert_write_capability_for(entity)
            .unwrap_or_else(|e| panic!("[{boot}] '{entity}' is registered but not writable: {e}"));
    }
}

#[test]
fn mirrored_entity_keeps_the_connector_as_its_only_write_authority() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        // Desktop runs SqlOnly, which is the configuration the panic was
        // reported from.
        env.set_enable_loro(false);
        env.set_enable_fake_mcp(true);

        env.start_app(true).await.expect("boot-1 start_app");
        assert_authorities(&env, "boot-1").await;

        env.stop_app().await.expect("stop_app after boot-1");

        env.start_app(true).await.expect("boot-2 start_app");
        assert_authorities(&env, "boot-2").await;
    });
}
