//! The SQL store stamps a created block with the root's injected clock.
//!
//! In SqlOnly mode the block write authority is `SqlOperationProvider`, which
//! stamps `created_at`/`updated_at` on a create that carries none. A stamp from
//! any clock but the root's makes a timed test read wall time.
//!
//! @pbt kind harness
//! @pbt covers sql-store-stamps-the-injected-clock — a block created through
//! the SqlOnly product wiring carries the injected clock's instant
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never compares a
//! stored timestamp with its clock

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt"),
    )
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(4))]

    #[test]
    fn sql_store_stamps_the_injected_clock(
        at in 1_000_000_000_000i64..4_102_444_800_000,
        content in "[a-z]{1,12}",
    ) {
        let rt = runtime();
        let rows = rt.clone().block_on(async {
            let env = TestEnvironmentBuilder::new()
                .with_clock(Arc::new(holon_api::TestClock::new(at)))
                .without_loro()
                .build(rt.clone())
                .await
                .expect("boot");
            env.settle().await;
            let mut params: HashMap<String, Value> = HashMap::new();
            params.insert("id".into(), Value::String("block:stamped".into()));
            params.insert("parent_id".into(), Value::String(EntityUri::no_parent().to_string()));
            params.insert("content".into(), Value::String(content));
            env.execute_operation("block", "create", params)
                .await
                .expect("create");
            env.query_sql("SELECT created_at, updated_at FROM block_raw WHERE id = 'block:stamped'")
                .await
                .expect("read stamps")
        });
        proptest::prop_assert_eq!(rows.len(), 1);
        proptest::prop_assert_eq!(rows[0].get("created_at"), Some(&Value::Integer(at)));
        proptest::prop_assert_eq!(rows[0].get("updated_at"), Some(&Value::Integer(at)));
    }
}
