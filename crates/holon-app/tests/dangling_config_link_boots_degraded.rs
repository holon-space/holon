//! A `holon.toml` that is a symlink to a missing file must not stop Holon from
//! starting: the session boots on the defaults and raises a banner that names
//! the link, its target and the fix.
//!
//! The config crate's link PBT draws the link layouts against `load_config`;
//! this test pins the one step it cannot reach, that a real boot puts the
//! condition `load_config` returned onto the bus. That step passes a value
//! through without looking at it, so one layout is enough.

#![cfg(unix)]

use std::sync::Arc;

use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::load_config;

#[test]
fn a_dangling_holon_toml_link_boots_and_raises_the_config_condition() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_dir = dir.path().join("cfg");
    std::fs::create_dir(&config_dir).unwrap();
    let link = config_dir.join("holon.toml");
    std::os::unix::fs::symlink("../missing/holon.toml", &link).unwrap();

    let (traced, locked, note) =
        load_config(&config_dir, HolonConfig::default()).expect("boot must not abort");
    let mut holon_config = traced.into_inner();
    holon_config.db_path = Some(dir.path().join("boot.db"));
    holon_config.vault.root = Some(dir.path().join("vault"));
    std::fs::create_dir(dir.path().join("vault")).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let (_session, _engine, bus) = runtime
        .block_on(holon_app::new_from_config_with_di(
            holon_config,
            SessionConfig::new(
                holon_api::UiInfo::permissive(),
                Arc::new(holon_api::SystemClock),
            )
            .without_wait()
            .with_boot_notes(note),
            config_dir.clone(),
            locked,
            Arc::new(ConditionBus::new()),
            |injector| {
                holon::testing::database_stuck_guard::report_database_stuck_in(injector);
                Ok(())
            },
            |injector| {
                injector
                    .try_resolve::<Arc<ConditionBus>>()
                    .map(|b| (*b).clone())
                    .expect("the composition root registers a ConditionBus")
            },
        ))
        .expect("the session must boot with a dangling holon.toml link");

    let current = bus.subscribe().current;
    let raised = current
        .iter()
        .find(|c| matches!(c.reason, ConditionKind::ConfigFileUnusable { .. }))
        .unwrap_or_else(|| {
            panic!(
                "a boot on the defaults must say so; the bus carries {:?}",
                current
                    .iter()
                    .map(|c| c.reason.condition_kind())
                    .collect::<Vec<_>>()
            )
        });
    let detail = raised.reason.detail(&raised.subject);
    let shown = format!("{} {}", detail.headline, detail.body.join(" "));
    for named in [
        link.display().to_string(),
        "../missing/holon.toml".to_string(),
    ] {
        assert!(
            shown.contains(&named),
            "the banner must name {named}: {shown}"
        );
    }
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "boot must leave the link as it is"
    );
    assert!(
        !dir.path().join("missing").exists(),
        "boot must write nothing"
    );
}
