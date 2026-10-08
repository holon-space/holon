//! Transition: an external editor saves a new org file holding one heading
//! whose only source child is a render.
//!
//! @pbt rung external
//!   writes the org file to the watched vault -> FileSyncController ingest,
//!   the same seam `WriteOrgFile` drives.
//! @pbt covers render-only-panel-renders-its-render — a block whose only
//!   source child is a render draws that render, not a bare entity leaf

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::SutFixtureFs;
use holon_pbt_core::validation::Reason;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::write_org_file::GEN_PLACEHOLDER;
use super::write_org_file::WriteOrgFile;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external editor saves {filename} where block {block_id} renders {dsl}")]
pub struct AuthorRenderOnlyBlock {
    pub filename: String,
    pub block_id: String,
    pub dsl: String,
}

/// The org file a render panel lives in: one heading `block_id` whose only
/// child is the render source `dsl`.
pub(crate) fn render_panel_file(filename: &str, block_id: &str, dsl: &str) -> WriteOrgFile {
    let heading_uri = EntityUri::block(block_id);
    let mut heading = Block::new_text(
        heading_uri.clone(),
        EntityUri::block(GEN_PLACEHOLDER),
        &format!("Panel {block_id}"),
    );
    heading.set_property("ID", Value::String(block_id.to_string()));
    let render = Block::new_source(
        EntityUri::block(&format!("{block_id}::render::0")),
        heading_uri,
        holon_api::SourceLanguage::Render.to_string(),
        dsl,
    );
    WriteOrgFile {
        filename: filename.to_string(),
        blocks: vec![heading, render],
        keyword_set: None,
    }
}

/// A new file and block name: neither the document nor the block exists yet.
pub(crate) fn render_panel_names_are_new<R: RefDocumentsMut>(
    state: &R,
    filename: &str,
    block_id: &str,
) -> bool {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| panic!("render panel file {filename:?} has no stem"));
    state.doc_uri_by_name(stem).is_none()
        && state
            .block_document_of(&EntityUri::block(block_id))
            .is_none()
}

/// `(filename, block_id, n)` for a fresh render panel, `prefix` naming the
/// kind.
pub(crate) fn render_panel_names(
    prefix: &'static str,
) -> impl Strategy<Value = (String, String, u32)> {
    (0u32..10_000).prop_map(move |n| {
        (
            format!("{prefix}_{n}.org"),
            format!("{prefix}-{n}").replace('_', "-"),
            n,
        )
    })
}

impl AuthorRenderOnlyBlock {
    fn org_file(&self) -> WriteOrgFile {
        render_panel_file(&self.filename, &self.block_id, &self.dsl)
    }
}

impl<R: RefDocumentsMut + Clone + 'static> TransitionFactory<R> for AuthorRenderOnlyBlock {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let state = state.clone();
        let strat = (render_panel_names("render_panel"), 0usize..3)
            .prop_map(|((filename, block_id, n), shape)| {
                let label = format!("render panel {n}");
                let dsl = match shape {
                    0 => format!("text(\"{label}\")"),
                    1 => format!("column(text(\"{label}\"))"),
                    _ => format!("row(text(\"{label}\"), text(\"second\"))"),
                };
                AuthorRenderOnlyBlock {
                    filename,
                    block_id,
                    dsl,
                }
            })
            .prop_filter("AuthorRenderOnlyBlock preconditions", move |t| {
                t.preconditions(&state).is_good()
            })
            .boxed();
        Validated::Good((1, strat))
    }
}

impl<R: RefDocumentsMut> TransitionRef<R> for AuthorRenderOnlyBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        assert!(
            holon_api::render_dsl::parse_render_dsl(&self.dsl).is_ok(),
            "AuthorRenderOnlyBlock carries a render that parses, got {:?}",
            self.dsl
        );
        vec![
            holon_pbt_core::validation::check(
                render_panel_names_are_new(state, &self.filename, &self.block_id),
                Reason::BlockIdAlreadyExists,
            ),
            self.org_file().preconditions(state),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        self.org_file().apply_to_ref(state);
    }
}

crate::cap_transition! {
    AuthorRenderOnlyBlock: SutFixtureFs,
    where R: [ RefDocumentsMut ],
    |me, state, sut| {
        me.org_file().apply_to_sut(state, sut).await;
    }
    sql_budget: |me, state| {
        holon_pbt_core::budget::SqlBudget::expected_sql(&me.org_file(), state)
    }
}
