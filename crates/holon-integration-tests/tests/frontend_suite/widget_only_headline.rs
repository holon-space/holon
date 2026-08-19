//! A user-authored headline that owns a query source must render its TEXT as
//! well as its widget. Widget-only is an explicit per-block opt-out.
//!
//! The `query_block` variant rendered a bare `live_block()`, so any block with
//! a query-source child lost its headline. That is right for the seeded layout
//! regions (Left Sidebar / Main Panel / Right Sidebar / Advice Rules), whose
//! heading is a container name the user never wants drawn — and wrong for
//! everything a user writes, e.g. `** Projects` on the ClaudeCode page, which
//! rendered as a blank disclosure row.
//!
//! @pbt kind harness
//! @pbt covers widget-only-opt-out — title+widget is the default for a
//! query-source block; `:widget_only: t` opts a block back into widget-only

#![cfg(feature = "pbt")]

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::SourceLanguage;
use holon_api::block::Block;
use holon_api::render_types::extract_widget_names;
use holon_core::storage::BlockQuerySource;
use holon_core::storage::BlockSnapshot;
use holon_core::storage::FocusRoot;
use holon_core::storage::from_sync;
use holon_frontend::FrontendSession;
use holon_integration_tests::pbt::reference_state::block_to_data_row;

type Blocks = Arc<Mutex<Vec<Block>>>;

fn source_over(blocks: Blocks) -> Arc<dyn BlockQuerySource> {
    Arc::new(from_sync(move || {
        Ok(BlockSnapshot::from_ordered(
            blocks.lock().unwrap().clone(),
            Vec::<FocusRoot>::new(),
        ))
    })) as Arc<dyn BlockQuerySource>
}

fn prql() -> String {
    SourceLanguage::Query(QueryLanguage::HolonPrql).to_string()
}

fn heading(id: &str, content: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), EntityUri::no_parent(), content);
    b.content = content.to_string();
    b
}

fn source_child(id: &str, parent: &str) -> Block {
    Block::new_source(
        EntityUri::block(id),
        EntityUri::block(parent),
        &prql(),
        "from block",
    )
}

fn boot(blocks: &Blocks) -> FrontendSession {
    holon_app::from_block_query_source(source_over(Arc::clone(blocks)), None)
}

/// The lookup-backed `has_query_source` resolves off a polled block-source
/// snapshot, so a freshly booted session needs a moment before the owner is
/// seen as owning a query child.
fn await_render(
    session: &FrontendSession,
    block: &Block,
    want_widget: &str,
) -> (Vec<String>, bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let profile = session.profiles().resolve(&block_to_data_row(block));
        let widgets = extract_widget_names(&profile.render);
        let cols = profile.render.visible_columns();
        if widgets.contains(want_widget) || Instant::now() >= deadline {
            return (cols, widgets.contains(want_widget));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The DEFAULT for a block owning a query source: its own content text renders
/// alongside the widget.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_source_headline_renders_its_title_and_its_widget() {
    let owner = heading("wo-plain-owner", "Projects");
    let blocks: Blocks = Arc::new(Mutex::new(vec![
        owner.clone(),
        source_child("wo-plain-owner-src", "wo-plain-owner"),
    ]));
    let session = boot(&blocks);

    let (cols, has_live_block) = await_render(&session, &owner, "live_block");
    assert!(
        has_live_block,
        "a block owning a query source must still render its query widget"
    );
    assert!(
        cols.iter().any(|c| c == "content"),
        "a user-authored headline that owns a query source must render its OWN text too; the \
         render expression reads no `content` column, so the headline draws blank. columns={cols:?}"
    );
}

/// The opt-out: `widget_only` suppresses the headline, which is what the
/// seeded layout regions rely on.
#[tokio::test(flavor = "multi_thread")]
async fn a_widget_only_block_renders_only_its_widget() {
    let mut owner = heading("wo-flagged-owner", "Main Panel");
    owner.widget_only = true;
    let blocks: Blocks = Arc::new(Mutex::new(vec![
        owner.clone(),
        source_child("wo-flagged-owner-src", "wo-flagged-owner"),
    ]));
    let session = boot(&blocks);

    let (cols, has_live_block) = await_render(&session, &owner, "live_block");
    assert!(
        has_live_block,
        "a widget_only block still renders its widget"
    );
    assert!(
        !cols.iter().any(|c| c == "content"),
        "a widget_only block must NOT draw its own headline text; columns={cols:?}"
    );
}
