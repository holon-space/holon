//! One sync scenario, run unchanged against BOTH fixture fakes.
//!
//! The point is the table: the scenario is written once against the
//! [`Fake`] contract, and every assertion must hold for an id-keyed cursor peer
//! AND a content-keyed snapshot peer with a cache-bust requirement. A property
//! that holds for one key shape and not the other would mean identity had
//! leaked into the Rust outside the sidecar's declaration.

use std::collections::BTreeSet;

use holon_connections::RemoteListReconciler;
use holon_connections::RowKey;
use holon_connections::sync_once;
use holon_connections_testing::CacheMode;
use holon_connections_testing::FixtureRows;
use holon_connections_testing::ListMutation;
use holon_connections_testing::apply_local_intents;
use holon_connections_testing::fakes;

/// The logical row the scenario's local-only addition names.
const LOCAL_ONLY: u8 = 9;

/// The row the scenario later drops from the peer.
const DROPPED: u8 = 2;

fn ids_of_rows(rows: &[holon_connections::LocalRow]) -> BTreeSet<String> {
    rows.iter().map(|r| r.id.clone()).collect()
}

fn peer_ids(peer: &holon_connections_testing::FixtureListPeer) -> BTreeSet<String> {
    peer.rows().into_iter().map(|(_, row)| row.id).collect()
}

/// The key of logical row `n`, derived the way the peer's `commit` reads it:
/// through the connection's declared key expression.
fn key_of(fake: &holon_connections_testing::Fake, n: u8) -> RowKey {
    let remote = (fake.remote_row)(n, 1, false);
    fake.compiled
        .key_of(&serde_json::to_value(&remote.columns).expect("a row serializes"))
        .expect("the row derives its key")
}

/// Run the scenario against one fake.
async fn run_scenario(fake: &holon_connections_testing::Fake) -> anyhow::Result<()> {
    let peer = fake.peer(&[1, 2], 4);
    let reconciler = RemoteListReconciler::new(fake.compiled.clone());
    let local_only = (fake.local_row)(LOCAL_ONLY, 1, false, false, false);
    let mut mirror = FixtureRows(vec![local_only]);

    // Round 1: the local-only row is pushed to the peer, and the peer's rows
    // are pulled into the mirror.
    let first = sync_once(&peer, &mirror, &reconciler, "device", 1_000).await?;
    assert_eq!(
        first.committed, 1,
        "{}: the addition was not committed",
        fake.label
    );
    assert!(
        !first.retried,
        "{}: an uncontended round retried",
        fake.label
    );
    mirror = FixtureRows(apply_local_intents(
        &fake.compiled,
        &mirror.0,
        &first.local,
    )?);
    assert_eq!(
        peer_ids(&peer),
        ids_of_rows(&mirror.0),
        "{}: the mirror does not match the peer after the first round",
        fake.label
    );

    // Round 2 with an unchanged peer is a no-op: nothing is re-pushed, and the
    // mirror still equals the peer.
    let second = sync_once(&peer, &mirror, &reconciler, "device", 2_000).await?;
    assert_eq!(
        second.committed, 0,
        "{}: the converged round did not stay converged",
        fake.label
    );
    mirror = FixtureRows(apply_local_intents(
        &fake.compiled,
        &mirror.0,
        &second.local,
    )?);
    assert_eq!(
        peer_ids(&peer),
        ids_of_rows(&mirror.0),
        "{}: a no-op round perturbed the mirror",
        fake.label
    );

    // The peer drops a row it had served; the next round must delete it from
    // the mirror, not resurrect it.
    peer.mutate(&ListMutation::Remove {
        key: key_of(fake, DROPPED),
    });
    let third = sync_once(&peer, &mirror, &reconciler, "device", 3_000).await?;
    mirror = FixtureRows(apply_local_intents(
        &fake.compiled,
        &mirror.0,
        &third.local,
    )?);
    assert_eq!(
        peer_ids(&peer),
        ids_of_rows(&mirror.0),
        "{}: a peer-side deletion did not reach the mirror",
        fake.label
    );
    assert!(
        !mirror
            .0
            .iter()
            .any(|r| r.id == (fake.remote_row)(DROPPED, 1, false).id),
        "{}: the dropped row is still in the mirror",
        fake.label
    );
    Ok(())
}

/// ONE round over a seeded peer of `fake`, with its transport's cache set to
/// `cache_mode` — everything else about the peer and the connection held fixed.
async fn one_round(
    fake: &holon_connections_testing::Fake,
    cache_mode: CacheMode,
) -> anyhow::Result<usize> {
    let peer = fake.peer_with_cache(cache_mode, &[1, 2], 4);
    let reconciler = RemoteListReconciler::new(fake.compiled.clone());
    let local_only = (fake.local_row)(LOCAL_ONLY, 1, false, false, false);
    let mirror = FixtureRows(vec![local_only]);
    Ok(sync_once(&peer, &mirror, &reconciler, "device", 1_000)
        .await?
        .committed)
}

/// The cache axis is only an axis if it CHANGES an outcome. Two peers that
/// differ in nothing but their transport cache are run over the same
/// connection, and the connection's own declaration decides which way it goes.
///
/// A connection that declares a `cache_buster` has every pull answered from
/// origin — that is what the declaration buys — so its round converges
/// whichever way the transport caches. A connection that declares none is
/// served the transport's cached body, and there a caching transport cannot
/// serve the write the round just made: the round refuses, by name, instead of
/// re-sending the commit it could not confirm.
#[tokio::test]
async fn the_cache_axis_changes_the_outcome() {
    for fake in fakes() {
        let declares_buster = fake.compiled.spec().cache_buster.is_declared();
        let fresh = one_round(&fake, CacheMode::Fresh).await;
        let caching = one_round(&fake, CacheMode::CachedNeedsBust).await;
        assert_eq!(
            fresh.as_ref().ok(),
            Some(&1),
            "{}: a fresh transport must converge on the one pushed row",
            fake.label
        );
        match caching {
            Ok(committed) => assert!(
                declares_buster,
                "{}: a caching transport with no declared cache buster converged \
                 ({committed} committed) — the cached body it served was not stale, so the \
                 axis is not driving anything",
                fake.label
            ),
            Err(e) => {
                assert!(
                    !declares_buster,
                    "{}: the connection declares a cache buster, so the pull is a fresh fetch \
                     and this round must converge; got: {e:#}",
                    fake.label
                );
                let message = format!("{e:#}");
                assert!(
                    message.contains("older than a write it just made"),
                    "{}: a caching transport must be refused as staleness, not as something \
                     else; got: {message}",
                    fake.label
                );
            }
        }
    }
}

#[tokio::test]
async fn the_same_scenario_runs_against_both_fakes() {
    for fake in fakes() {
        run_scenario(&fake).await.expect(fake.label);
    }
}

/// The two fakes must differ on EVERY axis, or the table is not the structural
/// spread it claims to be.
#[test]
fn the_two_fakes_span_all_four_axes() {
    let a = fakes()[0].profile;
    let b = fakes()[1].profile;
    assert_ne!(a.key_shape, b.key_shape, "key shape must differ");
    assert_ne!(
        a.change_detection, b.change_detection,
        "change detection must differ"
    );
    assert_ne!(
        a.commit_granularity, b.commit_granularity,
        "commit granularity must differ"
    );
    assert_ne!(a.cache_mode, b.cache_mode, "cache mode must differ");
}
