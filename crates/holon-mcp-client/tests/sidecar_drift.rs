//! An INSTALLED sidecar under `{config_dir}/integrations/` must never decide,
//! by itself, what a bundled provider's schema is. The app ships its own copy
//! of every provider it knows; an installed file only ENABLES that provider,
//! and supplies content only when it declares this build's schema version.
//!
//! Without that split the installed file silently outranks the shipped one, so
//! a copy taken before a schema requirement landed keeps the provider
//! permanently unavailable and the error blames the file's content without
//! saying it is stale or where the current one lives.

use holon_mcp_client::load_integration_configs;

/// A faithfully-shaped pre-`fetch_contract` `claude-history.yaml`: the
/// `session` render reads the `cc_message_fdw` foreign table while the
/// `message` entity declares no `vtable.fetch_contract` — exactly the pair
/// `finish_integration` rejects at connect, which is what makes the provider
/// unavailable for the whole session.
const STALE_INSTALLED_CLAUDE_HISTORY: &str = r#"
transport:
  child_process:
    command: claude-history-mcp
entity_prefix: "cc_"
entities:
  session:
    profile_variants:
      - name: default
        render: >-
          live_query(#{
            sql: "SELECT uuid, role, content FROM cc_message_fdw WHERE session_id = $context_local_id",
            item_template: text(col("content"))
          })
    schema:
      - name: id
        sql_type: TEXT
        primary_key: true
    vtable:
      list_resource: "claude-history://sessions"
  message:
    schema:
      - name: id
        sql_type: TEXT
        primary_key: true
      - name: session_id
        sql_type: TEXT
      - name: content
        sql_type: TEXT
        nullable: true
    vtable:
      list_resource: "claude-history://sessions/{session_id}/messages"
"#;

/// THE DRIFT RED. A stale installed copy of a BUNDLED provider must not be what
/// the app runs: the loaded `claude-history` must be the sidecar this build
/// ships, whose `message` entity declares a `fetch_contract`.
#[test]
fn a_stale_installed_claude_history_does_not_shadow_the_shipped_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("claude-history.yaml");
    std::fs::write(&installed, STALE_INSTALLED_CLAUDE_HISTORY).unwrap();

    let loaded = load_integration_configs(dir.path()).expect("load");
    let cfg = loaded
        .configs
        .iter()
        .find(|(name, _)| name == "claude-history")
        .map(|(_, cfg)| cfg)
        .expect("the installed file enables 'claude-history'");

    let message = cfg
        .entities
        .get("message")
        .expect("shipped claude-history declares a 'message' entity");
    let vtable = message
        .vtable
        .as_ref()
        .expect("shipped 'message' entity declares a vtable");
    assert!(
        vtable.fetch_contract.is_some(),
        "the app must run the sidecar it ships, not a copy installed before \
         `fetch_contract` was required — this is the drift that makes the \
         provider permanently unavailable"
    );
}

/// The supersede must be DISCLOSED, naming both paths and the incompatibility —
/// a silent auto-upgrade would be the same class of defect as the silent
/// staleness it replaces.
#[test]
fn superseding_a_stale_installed_sidecar_is_disclosed_with_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("claude-history.yaml");
    std::fs::write(&installed, STALE_INSTALLED_CLAUDE_HISTORY).unwrap();

    let loaded = load_integration_configs(dir.path()).expect("load");
    let notice = loaded
        .superseded
        .iter()
        .find(|n| n.provider == "claude-history")
        .expect("the supersede is disclosed, never silent");

    assert_eq!(notice.installed_path, installed);
    assert_eq!(
        notice.bundled_source,
        "docs/integrations/claude-history.yaml"
    );
    assert!(
        notice.incompatibility.contains("schema_version"),
        "the disclosure must name the incompatibility: {}",
        notice.incompatibility
    );
}

/// An installed file that declares THIS build's schema version is a deliberate
/// override and is honored verbatim — the mechanism must not confiscate the
/// escape hatch it depends on.
#[test]
fn a_current_schema_version_override_is_honored() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = format!(
        "schema_version: {}\n{}",
        holon_mcp_client::SIDECAR_SCHEMA_VERSION,
        STALE_INSTALLED_CLAUDE_HISTORY
    );
    std::fs::write(dir.path().join("claude-history.yaml"), yaml).unwrap();

    let loaded = load_integration_configs(dir.path()).expect("load");
    let cfg = loaded
        .configs
        .iter()
        .find(|(name, _)| name == "claude-history")
        .map(|(_, cfg)| cfg)
        .expect("provider enabled");

    assert!(
        cfg.entities["message"]
            .vtable
            .as_ref()
            .unwrap()
            .fetch_contract
            .is_none(),
        "an override declaring this build's schema version is used as written"
    );
    assert!(
        loaded.superseded.is_empty(),
        "an honored override is not a supersede"
    );
}

/// An installed file for a BUNDLED provider that no longer parses is the same
/// drift in its harshest form — `deny_unknown_fields` means a key removed from
/// the format turns a once-valid sidecar into an unreadable one. It must not
/// kill the boot (there IS something to run) and it must not pass silently.
#[test]
fn an_unparseable_installed_sidecar_is_superseded_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("claude-history.yaml");
    std::fs::write(&installed, "not: [valid: yaml: config").unwrap();

    let loaded = load_integration_configs(dir.path())
        .expect("an unreadable override falls back to the bundled sidecar, it does not abort boot");

    let cfg = loaded
        .configs
        .iter()
        .find(|(name, _)| name == "claude-history")
        .map(|(_, cfg)| cfg)
        .expect("provider enabled, running the bundled content");
    assert!(
        cfg.entities["message"]
            .vtable
            .as_ref()
            .unwrap()
            .fetch_contract
            .is_some(),
        "the bundled sidecar is what runs"
    );

    let notice = loaded
        .superseded
        .iter()
        .find(|n| n.provider == "claude-history")
        .expect("an unreadable override is disclosed");
    assert_eq!(notice.installed_path, installed);
    assert!(
        notice
            .incompatibility
            .contains("no schema_version could be established"),
        "the disclosure must say the file could not be read at all: {}",
        notice.incompatibility
    );
}

/// A zero-byte installed file is the same arm, reached by a different route
/// (serde sees a null document, not a config) — and a truncated file is a
/// realistic way for an installed sidecar to end up in that state.
#[test]
fn an_empty_installed_sidecar_is_superseded_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("claude-history.yaml"), "").unwrap();

    let loaded = load_integration_configs(dir.path()).expect("empty override is not fatal");
    assert!(
        loaded.configs.iter().any(|(n, _)| n == "claude-history"),
        "provider stays enabled on the bundled content"
    );
    assert_eq!(
        loaded.superseded.len(),
        1,
        "an empty file is disclosed, never silently equivalent to the bundled one"
    );
}

/// A provider the build does not ship is loaded from disk exactly as before —
/// bundling must not confiscate user-authored integrations.
#[test]
fn an_unbundled_provider_still_loads_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("my-own-thing.yaml"),
        "transport:\n  child_process:\n    command: echo\nentities: {}\n",
    )
    .unwrap();

    let loaded = load_integration_configs(dir.path()).expect("load");
    assert_eq!(loaded.configs.len(), 1);
    assert_eq!(loaded.configs[0].0, "my-own-thing");
    assert!(loaded.superseded.is_empty());
}

/// A bundled provider with no installed file stays OFF: the installed file is
/// the enablement signal, so bundling must not switch on every provider the
/// build happens to know.
#[test]
fn bundled_providers_are_not_enabled_without_an_installed_file() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load_integration_configs(dir.path()).expect("load");
    assert!(loaded.configs.is_empty());
    assert!(loaded.superseded.is_empty());
}

/// An installed copy byte-identical to the bundled one is not an override and
/// not a supersede — it is the same file, and must produce no disclosure noise.
#[test]
fn a_byte_identical_installed_copy_discloses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = holon_mcp_client::bundled_sidecar("claude-history").expect("bundled");
    std::fs::write(dir.path().join("claude-history.yaml"), bundled.yaml).unwrap();

    let loaded = load_integration_configs(dir.path()).expect("load");
    assert_eq!(loaded.configs.len(), 1);
    assert!(loaded.superseded.is_empty());
}
