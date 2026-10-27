use tokio_stream::wrappers::ReceiverStream;

use super::*;

#[test]
fn test_map_diff_apply() {
    let mut state = HashMap::new();
    apply_map_diff(&mut state, MapDiff::Insert { key: "a", value: 1 });
    assert_eq!(state.get("a"), Some(&1));

    apply_map_diff(&mut state, MapDiff::Update { key: "a", value: 2 });
    assert_eq!(state.get("a"), Some(&2));

    apply_map_diff(&mut state, MapDiff::Remove { key: "a" });
    assert!(state.is_empty());
}

#[test]
fn test_map_diff_replace() {
    let mut state = HashMap::from([("old", 1)]);
    let new = HashMap::from([("new", 2)]);
    apply_map_diff(&mut state, MapDiff::Replace(new.clone()));
    assert_eq!(state, new);
}

#[tokio::test]
async fn test_scan_state() {
    let (tx, rx) = mpsc::channel(16);
    let stream = ReceiverStream::new(rx);

    let mut scanned = stream.scan_state(0i32, |state, item: i32| *state += item);

    tx.send(1).await.unwrap();
    tx.send(2).await.unwrap();
    tx.send(3).await.unwrap();
    drop(tx);

    assert_eq!(scanned.next().await, Some(1));
    assert_eq!(scanned.next().await, Some(3));
    assert_eq!(scanned.next().await, Some(6));
    assert_eq!(scanned.next().await, None);
}

#[tokio::test]
async fn test_switch_map_uses_latest_inner() {
    let (outer_tx, outer_rx) = mpsc::channel::<u32>(16);
    let outer = ReceiverStream::new(outer_rx);

    // Inner streams: each sends a single tagged value after a short delay
    let mut switched = outer.switch_map(|n| {
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = tx.send(n * 100).await;
        });
        ReceiverStream::new(rx)
    });

    // Send one outer value and wait for its inner result
    outer_tx.send(1).await.unwrap();
    let first = switched.next().await;
    assert_eq!(first, Some(100));

    // Send another — should get result from the new inner
    outer_tx.send(2).await.unwrap();
    let second = switched.next().await;
    assert_eq!(second, Some(200));

    drop(outer_tx);
    assert_eq!(switched.next().await, None);
}

#[tokio::test]
async fn test_combine_latest_waits_for_both() {
    let (tx_a, rx_a) = mpsc::channel(16);
    let (tx_b, rx_b) = mpsc::channel(16);

    let mut combined = combine_latest(
        ReceiverStream::new(rx_a),
        ReceiverStream::new(rx_b),
        |a: &i32, b: &i32| (*a, *b),
    );

    tx_a.send(1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    // B arrives — now both have values
    tx_b.send(10).await.unwrap();
    assert_eq!(combined.next().await, Some((1, 10)));

    // Update A — uses latest B
    tx_a.send(2).await.unwrap();
    assert_eq!(combined.next().await, Some((2, 10)));

    drop(tx_a);
    drop(tx_b);
}

#[tokio::test]
async fn test_coalesce_batching() {
    let (tx, rx) = mpsc::channel(16);
    let stream = ReceiverStream::new(rx);

    let mut coalesced = coalesce(stream, Duration::from_millis(50), |batch: Vec<i32>| {
        vec![batch.into_iter().sum()]
    });

    tx.send(1).await.unwrap();
    tx.send(2).await.unwrap();
    tx.send(3).await.unwrap();
    drop(tx);

    assert_eq!(coalesced.next().await, Some(6));
}

#[test]
fn test_cdc_accumulator_from_rows() {
    use crate::Value;

    let rows = vec![
        HashMap::from([
            ("id".to_string(), Value::String("a".to_string())),
            ("content".to_string(), Value::String("hello".to_string())),
        ]),
        HashMap::from([
            ("id".to_string(), Value::String("b".to_string())),
            ("content".to_string(), Value::String("world".to_string())),
        ]),
    ];

    let acc = CdcAccumulator::from_rows(rows);
    assert_eq!(acc.state().len(), 2);
    assert!(acc.state().contains_key("a"));
    assert!(acc.state().contains_key("b"));
}

#[test]
fn test_cdc_accumulator_apply_change() {
    use crate::ChangeOrigin;
    use crate::Value;

    let mut acc = CdcAccumulator::<crate::widget_spec::DataRow>::from_rows(vec![]);

    let row = HashMap::from([
        ("id".to_string(), Value::String("x".to_string())),
        ("content".to_string(), Value::String("test".to_string())),
    ]);

    acc.apply_change(crate::Change::Created {
        data: row,
        origin: ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    });
    assert_eq!(acc.state().len(), 1);

    acc.apply_change(crate::Change::Deleted {
        id: "x".to_string(),
        origin: ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    });
    assert_eq!(acc.state().len(), 0);
}

#[test]
fn test_cdc_accumulator_fields_changed_patches() {
    use crate::ChangeOrigin;
    use crate::Value;

    let mut acc = CdcAccumulator::from_rows(vec![HashMap::from([
        ("id".to_string(), Value::String("x".to_string())),
        ("content".to_string(), Value::String("old".to_string())),
        ("tags".to_string(), Value::String("a,b".to_string())),
    ])]);

    acc.apply_change(crate::Change::FieldsChanged {
        entity_id: "x".to_string(),
        fields: vec![(
            "content".to_string(),
            Value::String("old".to_string()),
            Value::String("new".to_string()),
        )],
        origin: ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    });

    let row = acc.state().get("x").unwrap();
    // content should be patched
    assert_eq!(row.get("content").unwrap().as_string(), Some("new"));
    // tags should be preserved
    assert_eq!(row.get("tags").unwrap().as_string(), Some("a,b"));
}
