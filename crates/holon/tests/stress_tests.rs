use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use holon_loro::LoroDocument;
use tokio::time::sleep;

#[tokio::test]
async fn test_high_frequency_updates() -> Result<()> {
    let doc1 = LoroDocument::new("high-freq".to_string())?;
    let doc2 = LoroDocument::new("high-freq".to_string())?;

    let mut updates = Vec::new();
    for i in 0..1000 {
        let update = doc1.insert_text("editor", i, "x")?;
        updates.push(update);
    }

    let start = Instant::now();
    for update in updates {
        doc2.apply_update(&update)?;
    }
    let duration = start.elapsed();

    println!("Applied 1000 updates in {:?}", duration);
    assert!(
        duration.as_secs() < 10,
        "Should apply 1000 updates in under 10 seconds"
    );

    let text1 = doc1.get_text("editor")?;
    let text2 = doc2.get_text("editor")?;
    assert_eq!(text1, text2);
    assert_eq!(text1.len(), 1000);

    Ok(())
}

#[tokio::test]
async fn test_sustained_concurrent_operations() -> Result<()> {
    let doc1 = LoroDocument::new("sustained".to_string())?;
    let doc2 = LoroDocument::new("sustained".to_string())?;

    let doc1 = Arc::new(doc1);
    let doc2 = Arc::new(doc2);

    let doc1_clone = doc1.clone();
    let writer1 = tokio::spawn(async move {
        for i in 0..50 {
            doc1_clone.insert_text("editor", i, "A").ok();
            sleep(Duration::from_millis(10)).await;
        }
    });

    let doc2_clone = doc2.clone();
    let writer2 = tokio::spawn(async move {
        for i in 0..50 {
            doc2_clone.insert_text("editor", i, "B").ok();
            sleep(Duration::from_millis(10)).await;
        }
    });

    writer1.await?;
    writer2.await?;

    let update1 = doc1.export_snapshot()?;
    let update2 = doc2.export_snapshot()?;

    doc1.apply_update(&update2)?;
    doc2.apply_update(&update1)?;

    let text1 = doc1.get_text("editor")?;
    let text2 = doc2.get_text("editor")?;

    assert_eq!(text1, text2);

    Ok(())
}

#[tokio::test]
async fn test_memory_efficiency_large_doc() -> Result<()> {
    let doc = LoroDocument::new("memory-test".to_string())?;

    let iterations = 10000;
    for i in 0..iterations {
        doc.insert_text("editor", i, "x")?;
    }

    let snapshot = doc.export_snapshot()?;

    assert!(
        snapshot.len() < 1_000_000,
        "Snapshot should be reasonably compressed"
    );

    let text = doc.get_text("editor")?;
    assert_eq!(text.len(), iterations);

    Ok(())
}

#[tokio::test]
async fn test_update_size_efficiency() -> Result<()> {
    let doc = LoroDocument::new("update-size".to_string())?;

    let update1 = doc.insert_text("editor", 0, "Small")?;
    assert!(update1.len() < 1000, "Small update should be compact");

    let large_text = "x".repeat(100000);
    let update2 = doc.insert_text("editor", 5, &large_text)?;

    assert!(
        update2.len() < large_text.len() * 2,
        "Update should not be excessively larger than content"
    );

    Ok(())
}

#[tokio::test]
async fn test_long_running_stability() -> Result<()> {
    let doc1 = LoroDocument::new("stability".to_string())?;
    let doc2 = LoroDocument::new("stability".to_string())?;

    for round in 0..20 {
        for i in 0..10 {
            doc1.insert_text("editor", round * 10 + i, "x")?;
        }

        let update = doc1.export_snapshot()?;
        doc2.apply_update(&update)?;

        sleep(Duration::from_millis(50)).await;
    }

    let text1 = doc1.get_text("editor")?;
    let text2 = doc2.get_text("editor")?;

    assert_eq!(text1, text2);
    assert_eq!(text1.len(), 200);

    Ok(())
}
