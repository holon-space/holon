#![cfg(feature = "di")]

use holon_filesystem::RealFileSystem;
use holon_orgmode::di::OrgModeConfig;
use holon_orgmode::di::seed_default_org_assets;

#[tokio::test]
async fn the_seed_keeps_a_users_journals_org_that_declares_another_id() {
    let dir = tempfile::tempdir().unwrap();
    let user = "#+ID: 6f1c2a4e-user-page\n* My journals index\nhand-written notes\n";
    std::fs::write(dir.path().join("Journals.org"), user).unwrap();
    let mut config = OrgModeConfig::new(dir.path().to_path_buf());
    config.seed_assets = vec![(
        "Journals.org".to_string(),
        include_str!("../../../assets/default/Journals.org").to_string(),
    )];
    seed_default_org_assets(&RealFileSystem, &config)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Journals.org")).unwrap(),
        user,
        "the seed replaced the user's Journals.org with the bundled asset"
    );
}
