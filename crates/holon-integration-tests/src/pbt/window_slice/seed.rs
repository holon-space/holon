//! Increment 3a SUT-side seeding: graft the **fixed shared** `parent`/`c1`/`c2`
//! tree into a live window's backend so the windowed `inv-displayed-text` oracle
//! has known blocks to compare against (approach **B1** — graft under the Main
//! panel's rendered focus root).
//!
//! The Main panel renders the descendants of `focus_roots.root_id` (region
//! `main`). Attaching `parent` directly under that root makes `parent`/`c1`/`c2`
//! render as text widgets, and because the ids are the same fixed ids the ref
//! seeds ([`crate::pbt::composed::subsystem_seed::seed_ref_tree`] /
//! [`super::builders::window_ref_caps_seeded`]), the rendered widgets resolve to
//! ref-known blocks and the text comparison bites. The vault's pre-existing
//! random-UUID blocks stay unknown to the ref and are skipped.
//!
//! `inv-displayed-text` compares **content by id only** — the grafted parent need
//! not match the ref's notion of *where* `parent` lives (the ref seeds it under
//! `no_parent`); only the per-id content has to agree. So this graft is decoupled
//! from the ref's tree shape.

use anyhow::{Context, Result};

use holon_api::Value;

use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids};
use crate::test_environment::TestEnvironment;

/// Query the `root_id` the Main panel renders descendants of. Fail-loud: the
/// windowed slice boots a Turso session via `start_app`, so `focus_roots` must
/// exist and carry a `main` row once the window has settled.
async fn main_focus_root(env: &TestEnvironment) -> Result<String> {
    let rows = env
        .engine()
        .execute_query(
            "SELECT root_id FROM focus_roots WHERE region = 'main'".to_string(),
            std::collections::HashMap::new(),
            None,
        )
        .await
        .context("query focus_roots for the Main render root")?;
    let row = rows
        .first()
        .context("focus_roots has no 'main' row — window not settled / no focus")?;
    match row.get("root_id") {
        Some(Value::String(s)) => Ok(s.clone()),
        other => anyhow::bail!("focus_roots.root_id is not a string: {other:?}"),
    }
}

/// Graft the fixed `parent`/`c1`/`c2` tree under the Main focus root of a live,
/// settled window. Mirrors [`seed_ref_tree`](crate::pbt::composed::subsystem_seed::seed_ref_tree)'s
/// content (`PARENT`/`C1`/`C2`) and ids ([`fixed_ids`]) so the ref↔SUT id mapping
/// is the identity. The caller must re-settle the window afterwards so the new
/// blocks paint before the invariants read geometry/VM.
pub async fn graft_displayed_text_tree(env: &TestEnvironment) -> Result<()> {
    let root = main_focus_root(env).await?;
    let ids = fixed_ids();
    env.create_block(ids.parent.as_str(), &root, PARENT)
        .await
        .context("graft parent under Main focus root")?;
    env.create_block(ids.c1.as_str(), ids.parent.as_str(), C1)
        .await
        .context("graft c1 under parent")?;
    env.create_block(ids.c2.as_str(), ids.parent.as_str(), C2)
        .await
        .context("graft c2 under parent")?;
    Ok(())
}
