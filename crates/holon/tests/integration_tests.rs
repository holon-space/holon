use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use holon_loro::LoroDocument;
use serial_test::serial;
use tokio::time::sleep;
use tokio::time::timeout;

// Loro-only tests (no P2P adapter needed)

#[tokio::test]
#[serial]
async fn test_update_idempotency() -> Result<()> {
    let doc1 = LoroDocument::new("idempotent".to_string())?;
    let doc2 = LoroDocument::new("idempotent".to_string())?;

    let update = doc1.insert_text("editor", 0, "Test")?;

    doc2.apply_update(&update)?;
    doc2.apply_update(&update)?;
    doc2.apply_update(&update)?;

    let text = doc2.get_text("editor")?;

    assert_eq!(
        text, "Test",
        "Applying same update multiple times should be idempotent"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_snapshot_consistency() -> Result<()> {
    let doc1 = LoroDocument::new("snapshot".to_string())?;

    doc1.insert_text("editor", 0, "Hello")?;
    doc1.insert_text("editor", 5, " World")?;

    let snapshot1 = doc1.export_snapshot()?;
    let snapshot2 = doc1.export_snapshot()?;

    assert_eq!(
        snapshot1, snapshot2,
        "Multiple snapshots of unchanged document should be identical"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_peer_id_uniqueness() -> Result<()> {
    let doc1 = LoroDocument::new("unique-peer".to_string())?;
    let doc2 = LoroDocument::new("unique-peer".to_string())?;
    let doc3 = LoroDocument::new("unique-peer".to_string())?;

    let peer1 = doc1.peer_id();
    let peer2 = doc2.peer_id();
    let peer3 = doc3.peer_id();

    assert_ne!(peer1, peer2);
    assert_ne!(peer1, peer3);
    assert_ne!(peer2, peer3);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_utf8_content_sync() -> Result<()> {
    let doc1 = LoroDocument::new("utf8".to_string())?;
    let doc2 = LoroDocument::new("utf8".to_string())?;

    let utf8_content = "Hello 世界 🌍 Здравствуй مرحبا";
    doc1.insert_text("editor", 0, utf8_content)?;

    let snapshot = doc1.export_snapshot()?;
    doc2.apply_update(&snapshot)?;

    let text2 = doc2.get_text("editor")?;
    assert_eq!(text2, utf8_content);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_special_characters_in_content() -> Result<()> {
    let doc1 = LoroDocument::new("special-chars".to_string())?;
    let doc2 = LoroDocument::new("special-chars".to_string())?;

    let special_content = "Line1\nLine2\tTabbed\r\nWindows\0Null";
    doc1.insert_text("editor", 0, special_content)?;

    let snapshot = doc1.export_snapshot()?;
    doc2.apply_update(&snapshot)?;

    let text2 = doc2.get_text("editor")?;
    assert_eq!(text2, special_content);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_zero_length_insert() -> Result<()> {
    let doc = LoroDocument::new("zero-insert".to_string())?;

    doc.insert_text("editor", 0, "")?;
    let text = doc.get_text("editor")?;

    assert_eq!(text, "");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_conflicting_edits_convergence() -> Result<()> {
    let doc1 = LoroDocument::new("conflict-test".to_string())?;
    let doc2 = LoroDocument::new("conflict-test".to_string())?;
    let doc3 = LoroDocument::new("conflict-test".to_string())?;

    doc1.insert_text("editor", 0, "Base")?;

    let update_base = doc1.export_snapshot()?;
    doc2.apply_update(&update_base)?;
    doc3.apply_update(&update_base)?;

    let update1 = doc1.insert_text("editor", 4, " from 1")?;
    let update2 = doc2.insert_text("editor", 4, " from 2")?;
    let update3 = doc3.insert_text("editor", 4, " from 3")?;

    doc1.apply_update(&update2)?;
    doc1.apply_update(&update3)?;

    doc2.apply_update(&update1)?;
    doc2.apply_update(&update3)?;

    doc3.apply_update(&update1)?;
    doc3.apply_update(&update2)?;

    let text1 = doc1.get_text("editor")?;
    let text2 = doc2.get_text("editor")?;
    let text3 = doc3.get_text("editor")?;

    assert_eq!(text1, text2);
    assert_eq!(text2, text3);
    assert!(text1.contains("Base"));

    Ok(())
}
