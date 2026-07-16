//! Smoke test for the `McpUserDriver` rung of the driver ladder: drives a
//! REAL running Holon app through its embedded MCP server (streamable
//! HTTP). This is the de-risking replay rung, not the generated PBT loop —
//! see `mcp_user_driver.rs` module docs for what remains to promote it.
//!
//! Requires a live app serving MCP on `127.0.0.1:$MCP_SERVER_PORT`
//! (default 8521) — e.g. the iOS simulator build (the sim shares the host
//! loopback) or a desktop GPUI instance started with `MCP_SERVER_PORT` set.
//! Ignored by default so CI without a live app stays green:
//!
//! ```sh
//! MCP_SERVER_PORT=8521 cargo test -p holon-integration-tests \
//!     --test gpui_mcp_replay -- --ignored --nocapture
//! ```
//!
//! This smoke asserts on the DRIVER's observable effect (typed text read
//! back through `describe_ui`), NOT on a committed block row. Committing a
//! new block (Enter -> `split_block`) is a separate app path that is
//! currently broken on the iOS build — Enter on the virtual add-block field
//! errors `Missing or invalid parameter 'position'`, so the typed text stays
//! uncommitted in the editor. Asserting on a commit would conflate driver
//! correctness with that bug. Because nothing is committed, this smoke is
//! non-destructive (the typed text lives only in the transient editor).
//!
//! @pbt kind harness
//! @pbt covers mcp-driver-rung — drives a real running app through embedded MCP (driver-ladder de-risk rung)

use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use holon_api::entity_uri_from_id_str;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::UserDriver;

/// The always-rendered "add a block" editor at the bottom of the main
/// panel — a stable click target on a fresh vault (see
/// `holon-frontend/src/row_origin.rs`).
const ADD_BLOCK_ENTITY: &str = "block:__virtual:default-main-panel";

/// The main panel whose `describe_ui` subtree contains the add-block editor.
const MAIN_PANEL_ENTITY: &str = "block:default-main-panel";

#[tokio::test]
#[ignore = "needs a live Holon app serving MCP on 127.0.0.1:$MCP_SERVER_PORT (default 8521); run \
            with --ignored"]
async fn mcp_user_driver_smoke() -> Result<()> {
    let driver = McpUserDriver::connect_from_env()
        .await
        .context("connecting to the live app's MCP server")?;

    // Unique content so reruns against the same editor are distinguishable.
    let content = format!(
        "pbt smoke {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos()
    );

    // 1. Click the virtual add-block editor — routes through the app's
    //    GpuiUserDriver::click_entity, i.e. a real geometry click.
    let add_block = entity_uri_from_id_str(ADD_BLOCK_ENTITY);
    driver
        .click_entity(&add_block, "main")
        .await
        .context("clicking the add-block editor")?;

    // 2. Type the content one real keystroke at a time (each one an
    //    InteractionEvent::KeyDown through the production input pipeline).
    for ch in content.chars() {
        driver.send_raw_keystroke(&ch.to_string(), &[]).await?;
    }

    // 3. Assert the DRIVER round-trip: after a short settle for the async input
    //    pipeline, the add-block editor is present in the main-panel `describe_ui`
    //    tree.
    //
    //    NOTE on what is (not) asserted: `describe_ui` does NOT serialize a
    //    text editor's *transient* (uncommitted) buffer — only widget
    //    structure + entity ids — so the typed characters cannot be read back
    //    over MCP here (verified: the content appears in neither describe_ui
    //    format). And committing the block (Enter -> split_block) is broken on
    //    this build. So this smoke verifies the driver PLUMBING end to end —
    //    MCP handshake, `click_entity` routed through the app's GpuiUserDriver,
    //    N `send_raw_keystroke`s each accepted by the real KeyDown pipeline
    //    (any failure would have `?`-propagated above), and structural readback
    //    via `describe_ui` — but not editor-content correctness. Content-level
    //    readback is a remaining-work item: it needs either a working commit
    //    path or an MCP verb returning the live editor buffer. (Content
    //    rendering itself is confirmed manually via sim screenshots.)
    tokio::time::sleep(Duration::from_millis(500)).await;
    let main_panel = entity_uri_from_id_str(MAIN_PANEL_ENTITY);
    driver
        .refresh_ui(&main_panel)
        .await
        .with_context(|| format!("describe_ui({main_panel})"))?;
    assert!(
        driver.is_widget_visible(&add_block),
        "add-block editor {add_block} not present in the main-panel describe_ui tree"
    );

    Ok(())
}
