//! Transition: an external editor saves a new org file holding one heading
//! whose only source child is a render that does not parse.
//!
//! @pbt rung external
//!   writes the org file to the watched vault -> FileSyncController ingest,
//!   the same seam `WriteOrgFile` drives.
//! @pbt covers unparseable-render-source-is-visible — a render source that
//!   does not parse draws an error naming the block and the parse error, never
//!   a silent `table()`

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::SutFixtureFs;
use holon_pbt_core::validation::Reason;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::author_render_only_block::render_panel_file;
use super::author_render_only_block::render_panel_names;
use super::author_render_only_block::render_panel_names_are_new;
use super::write_org_file::WriteOrgFile;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "an external editor saves {filename} where block {block_id} has the broken render {dsl}"
)]
pub struct AuthorMalformedRender {
    pub filename: String,
    pub block_id: String,
    pub dsl: String,
}

/// Render sources an author can type that the render DSL refuses.
const MALFORMED_RENDERS: [&str; 4] = [
    "column(text(\"unclosed\"",
    "text(",
    "row(text(\"a\")))",
    "list(#{item_template: })",
];

impl AuthorMalformedRender {
    fn org_file(&self) -> WriteOrgFile {
        render_panel_file(&self.filename, &self.block_id, &self.dsl)
    }
}

impl<R: RefDocumentsMut + Clone + 'static> TransitionFactory<R> for AuthorMalformedRender {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let state = state.clone();
        let strat = (
            render_panel_names("bad_render"),
            proptest::sample::select(MALFORMED_RENDERS.to_vec()),
        )
            .prop_map(|((filename, block_id, _), dsl)| AuthorMalformedRender {
                filename,
                block_id,
                dsl: dsl.to_string(),
            })
            .prop_filter("AuthorMalformedRender preconditions", move |t| {
                t.preconditions(&state).is_good()
            })
            .boxed();
        Validated::Good((1, strat))
    }
}

impl<R: RefDocumentsMut> TransitionRef<R> for AuthorMalformedRender {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        assert!(
            holon_api::render_dsl::parse_render_dsl(&self.dsl).is_err(),
            "AuthorMalformedRender carries a render that does not parse, got {:?}",
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
    AuthorMalformedRender: SutFixtureFs,
    where R: [ RefDocumentsMut ],
    |me, state, sut| {
        me.org_file().apply_to_sut(state, sut).await;
    }
    sql_budget: |me, state| {
        holon_pbt_core::budget::SqlBudget::expected_sql(&me.org_file(), state)
    }
}
