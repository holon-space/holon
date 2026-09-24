//! `*::rebuild_views` raises a condition for exactly as long as it runs.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

fn rebuilding(bus: &ConditionBus) -> bool {
    bus.current()
        .iter()
        .any(|c| c.condition_key().kind == ConditionKind::WATCH_VIEWS_REBUILDING)
}

#[tokio::test(flavor = "multi_thread")]
async fn rebuild_views_is_disclosed_while_it_runs_and_not_after() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = HolonConfig {
        db_path: Some(dir.path().join("rebuild.db")),
        vault: VaultConfig {
            root: Some(dir.path().to_path_buf()),
        },
        ..Default::default()
    };
    let (_session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.path().to_path_buf(),
        HashSet::new(),
        |_injector| Ok(()),
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring must boot a session");
    let bus = injector.resolve::<Arc<ConditionBus>>();
    assert!(!rebuilding(&bus), "nothing is rebuilding before the op");

    let wildcard = EntityName::from("*");
    let mut op = Box::pin(engine.execute_operation(
        &wildcard,
        "rebuild_views",
        holon_api::StorageEntity::new(),
        OpOrigin::User,
    ));
    let mut pending_polls = 0;
    let result = std::future::poll_fn(|cx| match op.as_mut().poll(cx) {
        Poll::Ready(result) => Poll::Ready(result),
        Poll::Pending => {
            pending_polls += 1;
            assert!(
                rebuilding(&bus),
                "rebuild_views is running (pending poll {pending_polls}) and the bus does not \
                 say so: {:?}",
                bus.current()
            );
            Poll::Pending
        }
    })
    .await;
    result.expect("rebuild_views");

    assert!(
        pending_polls > 0,
        "the op never yielded, so nothing observed it while it ran"
    );
    assert!(
        !rebuilding(&bus),
        "the op has ended and its condition is still raised: {:?}",
        bus.current()
    );
}
