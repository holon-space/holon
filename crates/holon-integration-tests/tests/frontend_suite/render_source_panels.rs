//! A block's render-source child decides what the block draws, in both render
//! arms: the Turso arm (`BlockDomain::render_entity` behind the
//! `BackendEngine` watcher) and the snapshot arm (`LoroUiWatcher` behind a
//! no-Turso session). The recovery screen is made of such blocks, so it must
//! render from parsed org text alone.
//!
//! - A block whose only source child is a render draws that render.
//! - A render source that does not parse draws an `error` node that names the
//!   block and the parse error.
//!
//! @pbt kind harness
//! @pbt covers render-only-panel-renders-its-render — a panel with only a
//! render child draws the render, not a bare leaf
//! @pbt covers unparseable-render-source-is-visible — a render source that does
//! not parse is an error node naming the block, never a silent `table()`

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon::api::UiWatcher;
use holon_api::EntityUri;
use holon_api::RenderExpr;
use holon_api::SourceLanguage;
use holon_api::UiEvent;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::storage::BlockQuerySource;
use holon_core::storage::BlockSnapshot;
use holon_core::storage::FocusRoot;
use holon_core::storage::from_sync;
use holon_integration_tests::TestEnvironmentBuilder;

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

const RENDER_ONLY: &str = "c1-render-only";
const RENDER_ONLY_DSL: &str = r#"column(text("c1 render-only probe"))"#;
const BAD_WITH_QUERY: &str = "c1-bad-render-query";
const BAD_ONLY: &str = "c1-bad-render-only";
const BAD_DSL: &str = r#"column(text("unclosed""#;

fn probe_org() -> String {
    format!(
        "* Render Only\n:PROPERTIES:\n:ID: {RENDER_ONLY}\n:END:\n#+BEGIN_SRC render :id \
         {RENDER_ONLY}::render::0\n{RENDER_ONLY_DSL}\n#+END_SRC\n* Bad Render With Query\n\
         :PROPERTIES:\n:ID: {BAD_WITH_QUERY}\n:END:\n#+BEGIN_SRC render :id \
         {BAD_WITH_QUERY}::render::0\n{BAD_DSL}\n#+END_SRC\n#+BEGIN_SRC holon_sql :id \
         {BAD_WITH_QUERY}::src::0\nSELECT * FROM block WHERE id = 'block:{BAD_WITH_QUERY}'\n\
         #+END_SRC\n* Bad Render Only\n:PROPERTIES:\n:ID: {BAD_ONLY}\n:END:\n#+BEGIN_SRC render \
         :id {BAD_ONLY}::render::0\n{BAD_DSL}\n#+END_SRC\n"
    )
}

async fn first_structure(watcher: Arc<dyn UiWatcher>, id: &str) -> RenderExpr {
    let mut handle = watcher
        .watch_ui(EntityUri::block(id))
        .await
        .unwrap_or_else(|e| panic!("watch_ui({id}) failed: {e:#}"));
    loop {
        let event = tokio::time::timeout(SYNC_TIMEOUT, handle.recv())
            .await
            .unwrap_or_else(|_| panic!("no Structure event for {id} within {SYNC_TIMEOUT:?}"))
            .unwrap_or_else(|| panic!("the watch of {id} closed before a Structure event"));
        if let UiEvent::Structure { render_expr, .. } = event {
            return render_expr;
        }
    }
}

fn assert_draws_its_render(expr: &RenderExpr, arm: &str) {
    let expected = holon_api::render_dsl::parse_render_dsl(RENDER_ONLY_DSL).expect("probe parses");
    assert_eq!(
        expr.to_rhai(),
        expected.to_rhai(),
        "[{arm}] a block whose only source child is a render must draw that render"
    );
}

fn assert_parse_error_node(expr: &RenderExpr, id: &str, arm: &str) {
    let parse_error = holon_api::render_dsl::parse_render_dsl(BAD_DSL)
        .expect_err("the bad probe must not parse")
        .to_string();
    let RenderExpr::FunctionCall { name, args } = expr else {
        panic!(
            "[{arm}] {id}: expected an error node, got {}",
            expr.to_rhai()
        );
    };
    assert_eq!(
        name,
        "error",
        "[{arm}] a render source of {id} that does not parse must draw an error node, got {}",
        expr.to_rhai()
    );
    let message = args
        .iter()
        .find_map(|a| match (&a.name, &a.value) {
            (
                Some(n),
                RenderExpr::Literal {
                    value: Value::String(m),
                },
            ) if n == "message" => Some(m.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("[{arm}] the error node has no message: {}", expr.to_rhai()));
    assert!(
        message.contains(&format!("block:{id}")) && message.contains(&parse_error),
        "[{arm}] the error must name the block ({id}) and the parse error ({parse_error}); got \
         {message:?}"
    );
}

#[test]
fn turso_arm_renders_render_children_and_shows_parse_errors() {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime"),
    );
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("c1-probes.org", probe_org())
            .build(rt.clone())
            .await
            .expect("build environment");
        for id in [
            format!("{RENDER_ONLY}::render::0"),
            format!("{BAD_WITH_QUERY}::render::0"),
            format!("{BAD_WITH_QUERY}::src::0"),
            format!("{BAD_ONLY}::render::0"),
        ] {
            assert!(
                env.wait_for_block(&id, SYNC_TIMEOUT).await,
                "{id} must sync"
            );
        }
        let watcher = env.engine().clone() as Arc<dyn UiWatcher>;

        assert_parse_error_node(
            &first_structure(watcher.clone(), BAD_WITH_QUERY).await,
            BAD_WITH_QUERY,
            "turso",
        );
        assert_draws_its_render(
            &first_structure(watcher.clone(), RENDER_ONLY).await,
            "turso",
        );
        assert_parse_error_node(&first_structure(watcher, BAD_ONLY).await, BAD_ONLY, "turso");
    });
}

fn heading(id: &str) -> Block {
    Block::new_text(EntityUri::block(id), EntityUri::no_parent(), id)
}

fn render_child(parent: &str, dsl: &str) -> Block {
    Block::new_source(
        EntityUri::block(&format!("{parent}::render::0")),
        EntityUri::block(parent),
        SourceLanguage::Render.to_string(),
        dsl,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_arm_renders_render_children_and_shows_parse_errors() {
    let blocks = vec![
        heading(RENDER_ONLY),
        render_child(RENDER_ONLY, RENDER_ONLY_DSL),
        heading(BAD_ONLY),
        render_child(BAD_ONLY, BAD_DSL),
    ];
    let blocks = Arc::new(Mutex::new(blocks));
    let source = Arc::new(from_sync(move || {
        Ok(BlockSnapshot::from_ordered(
            blocks.lock().unwrap().clone(),
            Vec::<FocusRoot>::new(),
        ))
    })) as Arc<dyn BlockQuerySource>;
    let watcher = Arc::new(holon_loro_wiring::loro_ui_watcher::LoroUiWatcher::new(
        source,
    )) as Arc<dyn UiWatcher>;

    assert_draws_its_render(
        &first_structure(watcher.clone(), RENDER_ONLY).await,
        "snapshot",
    );
    assert_parse_error_node(
        &first_structure(watcher, BAD_ONLY).await,
        BAD_ONLY,
        "snapshot",
    );
}
