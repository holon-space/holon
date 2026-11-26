//! A stop writes every edit the store holds to its org file, also when the
//! CDC delivery of that edit reaches the write-back late.
//!
//! The lateness is the real delivery path slowed per row, so the delivery is
//! still on its way when the stop begins and nothing else in the chain moves.
//!
//! @pbt kind property
//! @pbt covers stop-writes-late-delivered-edits — a stop returns only after
//! every edit made before it is in its org file, however late its CDC
//! delivery reaches the write-back
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never stops the
//! app while a delivery is held back

use std::sync::Arc;

use holon_api::latency_slo::fault_injection::DeliveryDelay;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironmentBuilder;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

const FILE: &str = "Late.org";
const CHILD: &str = "late-child";

fn page(child: &str) -> String {
    format!(
        "* Root\n:PROPERTIES:\n:ID: late-root\n:END:\n** {child}\n:PROPERTIES:\n:ID: \
         {CHILD}\n:END:\n"
    )
}

#[derive(Debug, Clone)]
struct Case {
    /// Per delivered row, so above the shortest quiet window a stop could
    /// trust.
    delay_ms: u64,
    edits: Vec<String>,
}

fn case() -> impl Strategy<Value = Case> {
    (150u64..=400, prop::collection::vec("[a-z]{1,8}", 1..=3))
        .prop_map(|(delay_ms, edits)| Case { delay_ms, edits })
}

fn check(case: &Case) -> Result<(), TestCaseError> {
    let delay = DeliveryDelay::default();
    let bound = delay.clone();
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .on_thread_start(move || bound.bind_to_this_thread())
            .build()
            .expect("build a runtime"),
    );
    rt.clone().block_on(async {
        let mut env = TestEnvironmentBuilder::new()
            .with_org_file(FILE, page("Child"))
            .build(rt.clone())
            .await
            .expect("boot the vault");
        env.settle().await;
        let path = env.org_file_path(FILE);
        let armed = delay.arm(case.delay_ms);
        for text in &case.edits {
            env.update_block_content(CHILD, text)
                .await
                .expect("edit the child block");
        }
        let stopped = env.stop_app().await;
        drop(armed);
        prop_assert!(stopped.is_ok(), "the stop failed: {stopped:?}");
        let disk = env
            .org_fs
            .read_to_string(&path)
            .await
            .unwrap_or_else(|e| panic!("read {FILE}: {e}"));
        let last = case.edits.last().expect("a case edits at least once");
        prop_assert!(
            disk.contains(&format!("** {last}\n")),
            "the stop returned before the last edit {last:?} reached {FILE}:\n{disk}"
        );
        Ok(())
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn a_stop_writes_every_edit_however_late_its_delivery(case in case()) {
        check(&case)?;
    }
}
