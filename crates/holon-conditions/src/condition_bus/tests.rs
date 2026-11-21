use super::*;

fn raised(change: ConditionChange) -> Condition {
    change.raised().expect("expected Raised")
}

fn connect_failed(name: &str) -> Condition {
    Condition {
        subject: name.into(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: name.into(),
            error: "sidecar died".into(),
        },
    }
}

/// Kinds are the sticky identity, so two different degradations of the
/// same subject must not overwrite each other.
#[tokio::test(flavor = "current_thread")]
async fn distinct_kinds_on_one_subject_coexist() {
    let bus = ConditionBus::new();
    bus.emit(Condition {
        subject: "s".into(),
        reason: ConditionKind::SnapshotSaveFailed("disk full".into()),
    });
    bus.emit(Condition {
        subject: "s".into(),
        reason: ConditionKind::SqlProjectionFailed("table locked".into()),
    });
    assert_eq!(bus.subscribe().current.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn subscriber_receives_event() {
    let bus = ConditionBus::new();
    let mut sub = bus.subscribe();
    bus.emit(Condition {
        subject: "abc".into(),
        reason: ConditionKind::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
    });
    let ev = raised(sub.changes.recv().await.unwrap());
    assert_eq!(ev.subject, "abc");
    assert!(matches!(
        ev.reason,
        ConditionKind::SnapshotLoadFailed(ref p) if p.contains("corrupt")
    ));
}

/// The boot seam: integration failures are raised inside boot DI, before
/// the window (the only consumer) exists.
#[tokio::test(flavor = "current_thread")]
async fn boot_time_conditions_reach_a_later_subscriber() {
    let bus = ConditionBus::new();
    bus.emit(connect_failed("github"));
    bus.emit(Condition {
        subject: "linear".into(),
        reason: ConditionKind::IntegrationNeedsAuth {
            integration: "linear".into(),
            auth_url: "https://linear.app/oauth".into(),
        },
    });

    // window launches, strictly later
    let sub = bus.subscribe();

    let learned: Vec<String> = sub.current.iter().map(|e| e.subject.clone()).collect();
    assert_eq!(
        learned,
        vec!["github".to_string(), "linear".to_string()],
        "a subscriber that arrives after boot must still learn the current degraded conditions"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn re_raising_a_condition_replaces_rather_than_accumulates() {
    let bus = ConditionBus::new();
    bus.emit(connect_failed("github"));
    bus.emit(Condition {
        subject: "github".into(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: "github".into(),
            error: "still dead".into(),
        },
    });
    let sub = bus.subscribe();
    assert_eq!(sub.current.len(), 1);
    assert!(matches!(
        sub.current[0].reason,
        ConditionKind::IntegrationConnectFailed { ref error, .. } if error == "still dead"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn clear_removes_the_condition_and_notifies() {
    let bus = ConditionBus::new();
    bus.emit(connect_failed("github"));
    let mut sub = bus.subscribe();
    assert_eq!(sub.current.len(), 1);

    let key = ConditionKey {
        subject: "github".into(),
        kind: "integration-connect-failed",
    };
    bus.clear(&key);

    assert!(matches!(
        sub.changes.recv().await.unwrap(),
        ConditionChange::Cleared(k) if k == key
    ));
    assert!(bus.subscribe().current.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn clearing_an_unraised_condition_broadcasts_nothing() {
    let bus = ConditionBus::new();
    let mut sub = bus.subscribe();
    bus.clear(&ConditionKey {
        subject: "github".into(),
        kind: "integration-connect-failed",
    });
    assert!(matches!(
        sub.changes.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}
