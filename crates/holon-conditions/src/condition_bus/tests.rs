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

/// Every condition names an all-clear, and clearing it works uniformly —
/// stickiness without a clearing path is a permanent banner.
#[tokio::test(flavor = "current_thread")]
async fn a_share_condition_clears_by_its_kind() {
    let bus = ConditionBus::new();
    bus.emit(Condition {
        subject: "s".into(),
        reason: ConditionKind::SnapshotSaveFailed("disk full".into()),
    });
    bus.clear(&ConditionKey {
        subject: "s".into(),
        kind: ConditionKind::SNAPSHOT_SAVE_FAILED,
    });
    assert!(
        bus.subscribe().current.is_empty(),
        "the next successful save must leave no banner behind"
    );
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

/// The boot race the integration seam already fixed, for the OTHER
/// emitters: `wiring.rs`'s detached `post_ready` task emits
/// `VaultIngestFailed` while the window is still launching. A fast-failing
/// initial scan lands before the disclosure bridge subscribes; without
/// replay the banner is dropped and the vault silently half-syncs.
#[tokio::test(flavor = "current_thread")]
async fn every_degradation_reaches_a_subscriber_that_arrives_after_it_was_raised() {
    let raised_before_anyone_listens = vec![
        ConditionKind::VaultFileEmptied { seen: "0".into() },
        ConditionKind::SnapshotSaveFailed("disk full".into()),
        ConditionKind::SnapshotLoadFailed("/v/s.loro.corrupt-1".into()),
        ConditionKind::RehydrationFailed("advertiser: port in use".into()),
        ConditionKind::SqlProjectionFailed("table locked".into()),
        ConditionKind::ForeignIdCollision("block:journals".into()),
        ConditionKind::SharedSubtreeNotMaterialized {
            file: "/vault/Projects/Shared.org".into(),
        },
        ConditionKind::WritebackDegraded("stream died 3x".into()),
    ];

    for reason in raised_before_anyone_listens {
        let bus = ConditionBus::new();
        bus.emit(Condition {
            subject: "subject".into(),
            reason: reason.clone(),
        });

        // The window (the only consumer) launches strictly later.
        let sub = bus.subscribe();

        assert_eq!(
            sub.current.len(),
            1,
            "a degradation raised before the disclosure bridge subscribed must still reach \
                 it — otherwise the app boots looking healthy while {reason:?} is in effect"
        );
    }
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

#[tokio::test(flavor = "current_thread")]
async fn multiple_subscribers_all_see_events() {
    let bus = ConditionBus::new();
    let mut sub1 = bus.subscribe();
    let mut sub2 = bus.subscribe();
    bus.emit(Condition {
        subject: "x".into(),
        reason: ConditionKind::RehydrationFailed("endpoint".into()),
    });
    assert_eq!(raised(sub1.changes.recv().await.unwrap()).subject, "x");
    assert_eq!(raised(sub2.changes.recv().await.unwrap()).subject, "x");
}
