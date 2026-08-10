//! Transition: write an org file to the temp directory.
//!
//! @pbt rung external
//!   writes an org file to the watched temp dir -> FileSyncController
//! re-ingest. @pbt covers org-writeback-roundtrip — org disk write -> parse ->
//! block_raw
//!
//! Mirrors the legacy logic split across `state_machine.rs:326-338`
//! (generator), `state_machine.rs:3077-3101` (precondition),
//! `state_machine.rs:1738-1931` (ref-state apply),
//! `sut.rs:661-670` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::OrgRenderer;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::SutFixtureFs;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Seed a document's blocks before the app starts.
///
/// The generator produces `Block` instances directly (it always did, then
/// threw them away by rendering to org text). This transition carries those
/// blocks and decides how to materialise them against the SUT: serialise to
/// org text and write a file for a Turso/org wiring, or write them straight
/// into the Loro doc for a no-Turso wiring. The reference-state effect is the
/// same either way — the blocks are inserted as-is, with no re-parsing.
///
/// The generated blocks are parented to a `gen-placeholder` document uri; the
/// real per-document uri is resolved in `apply_to_ref` (and the placeholder is
/// also what the org renderer uses as the file id on the SUT side, so the
/// emitted text is byte-identical to the previous text-first generator).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WriteOrgFile {
    pub filename: String,
    #[serde(with = "holon_api::block::block_wire_vec")]
    pub blocks: Vec<Block>,
    /// Custom `#+TODO:` keyword set for this file (extended-gen axis 5).
    /// `None` = no header, parser defaults apply. `#[serde(default)]` keeps
    /// pre-axis-5 capture JSONs loadable.
    #[serde(default)]
    pub keyword_set: Option<crate::pbt::generators::TodoKeywordSet>,
}

/// The placeholder document uri the generator parents top-level blocks to.
/// Top-level headings carry this as their `parent_id`; `apply_to_ref` remaps
/// it to the resolved per-document uri, and the SUT-side renderer uses it as
/// the file id so the emitted org text matches the prior generator output.
pub(crate) const GEN_PLACEHOLDER: &str = "gen-placeholder";

impl WriteOrgFile {
    /// Build a `WriteOrgFile` from raw org text. Used by the Gherkin step
    /// matcher, where authors write org content directly in a docstring. Parses
    /// the text with the production org parser, then reparents top-level blocks
    /// onto the `GEN_PLACEHOLDER` document uri so they flow through the same
    /// seeding path as generator-produced blocks.
    pub fn from_org_text(filename: String, content: &str) -> anyhow::Result<Self> {
        let placeholder = EntityUri::block(GEN_PLACEHOLDER);
        let parsed = holon_orgmode::parse_org_file(
            std::path::Path::new(&filename),
            content,
            &placeholder,
            std::path::Path::new("."),
        )?;
        let doc_id = parsed.document.id.clone();
        let blocks = parsed
            .blocks
            .into_iter()
            .map(|mut b| {
                if b.parent_id == doc_id {
                    b.parent_id = placeholder.clone();
                }
                b
            })
            .collect();
        Ok(Self {
            filename,
            blocks,
            keyword_set: None,
        })
    }
}

/// The one hand-written vocabulary in the catalog: this transition's payload is
/// an ORG DOCUMENT, carried by the step's docstring, so the derive (which maps
/// fields to template placeholders) cannot express it. `blocks` and
/// `keyword_set` are read out of, and written back into, that docstring.
impl holon_pbt_core::step_vocabulary::StepVocabulary for WriteOrgFile {
    const TEMPLATE: &'static str = "an org file {filename}:";

    fn field_names() -> &'static [&'static str] {
        &["filename", "blocks", "keyword_set"]
    }

    fn template_fields() -> &'static [holon_pbt_core::step_vocabulary::TemplateField] {
        &[(
            "filename",
            <String as holon_pbt_core::step_vocabulary::StepField>::QUOTED,
        )]
    }

    fn render_step(&self) -> holon_pbt_core::step_vocabulary::RenderedStep {
        let text = holon_pbt_core::step_vocabulary::render_template(
            Self::TEMPLATE,
            &[("filename", true, self.filename.clone())],
        );
        let rendered = OrgRenderer::render_entitys(
            &self.blocks,
            std::path::Path::new(self.filename.as_str()),
            &EntityUri::block(GEN_PLACEHOLDER),
        );
        let docstring = match &self.keyword_set {
            Some(ks) => format!("{}\n{}", ks.to_org_header(), rendered),
            None => rendered,
        };
        holon_pbt_core::step_vocabulary::RenderedStep {
            text,
            docstring: Some(docstring),
        }
    }

    fn parse_step(text: &str, docstring: Option<&str>) -> Result<Option<Self>, String> {
        let Some(caps) = holon_pbt_core::step_vocabulary::capture_template(
            Self::TEMPLATE,
            Self::template_fields(),
            text,
        ) else {
            return Ok(None);
        };
        let filename = holon_pbt_core::step_vocabulary::captured(&caps, "filename").to_string();
        let content = docstring.ok_or_else(|| {
            format!("org-file step {text:?} needs a docstring holding the org content")
        })?;
        Self::from_org_text(filename, content)
            .map(Some)
            .map_err(|e| format!("failed to parse org-file step content: {e}"))
    }

    fn step_examples() -> Vec<Self> {
        // Keyword-set-carrying examples are deliberately absent: the parse side
        // reads the org text with the production parser, which resolves the
        // `#+TODO:` header into task states rather than handing the set back.
        vec![
            Self {
                filename: "example.org".to_string(),
                blocks: Vec::new(),
                keyword_set: None,
            },
            Self::from_org_text(
                "example.org".to_string(),
                "* HelloWorld\n:PROPERTIES:\n:ID: blk-a\n:END:\n",
            )
            .expect("the example org text must parse"),
        ]
    }

    fn step_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("WriteOrgFile is serializable")
    }
}

impl<R: RefDocumentsMut + Clone + 'static> TransitionFactory<R> for WriteOrgFile {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pre_startup_file_count = state.document_count();
        let file_weight = if pre_startup_file_count < 3 { 3 } else { 1 };

        // Layout overrides (custom `index.org` query layouts) are OFF by
        // default: a vanilla seed layout renders blocks interactively, so the
        // edit/split/cursor transitions are reachable. Opt back in with
        // `HOLON_PBT_LAYOUT_OVERRIDE=1` to exercise custom-layout paths.
        let state_for_preconditions = state.clone();
        let allow_index_override = std::env::var("HOLON_PBT_LAYOUT_OVERRIDE").is_ok();
        // Gate the advice-rule arm at generation time: mint a rule only when the
        // reference holds no NON-SEED rule yet, so `active_rule`'s ≤1-active
        // invariant holds. The bundled INACTIVE `index.org` rule seeds every
        // vault (and the reference), so counting it would silently kill this
        // arm forever. Re-checked under shrinking in `preconditions` below.
        let allow_advice_rule = !state.has_non_seed_advice_rule();
        // Axis 5 (promoted 2026-06-10): ~half the files carry a custom
        // `#+TODO:` keyword set, emitted as the org header on the SUT side
        // and adopted by the reference doc block.
        let strat = proptest::option::of(crate::pbt::generators::todo_keyword_set_strategy())
            .prop_flat_map(move |keyword_set| {
                crate::pbt::generators::generate_org_file_content_with_keywords(
                    keyword_set.clone(),
                    allow_index_override,
                    allow_advice_rule,
                )
                .prop_map(move |(filename, blocks)| WriteOrgFile {
                    filename,
                    blocks,
                    keyword_set: keyword_set.clone(),
                })
            })
            .prop_filter("WriteOrgFile preconditions", move |t| {
                t.preconditions(&state_for_preconditions).is_good()
            })
            .boxed();

        Validated::Good((file_weight, strat))
    }
}

impl<R: RefDocumentsMut> TransitionRef<R> for WriteOrgFile {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![];

        // Reject if any heading block in this file already exists under a
        // different document. Mirrors the previous `:ID:`-drawer collision
        // check: only text/heading blocks carry an `:ID:` drawer, so source
        // blocks (`{id}::src::N`) are excluded.
        let doc_name = std::path::Path::new(self.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.filename);
        let doc_uri = state
            .doc_uri_by_name(doc_name)
            .unwrap_or_else(|| EntityUri::block("precondition-placeholder"));
        let any_collision = self
            .blocks
            .iter()
            .filter(|b| b.content_type != holon_api::ContentType::Source)
            .any(|b| {
                state
                    .block_document_of(&b.id)
                    .is_some_and(|existing_doc| existing_doc != doc_uri)
            });
        checks.push(check(!any_collision, Reason::BlockIdAlreadyExists));

        // Shrink-safe ≤1-rule gate: a file that seeds an advice-rule block may
        // only land when the reference holds no advice-rule block yet. Generation
        // time already gates this, but the shrinker can reorder/drop earlier
        // transitions and revalidate this file against a state that now already
        // has a rule — so the invariant (`active_rule` asserts ≤1 active) must be
        // enforced here too, not only at generation.
        let this_seeds_rule = self.blocks.iter().any(|b| {
            b.source_language
                .as_ref()
                .map(|sl| sl.to_string())
                .as_deref()
                == Some(holon_advice::ADVICE_RULE_SOURCE_LANGUAGE)
        });
        if this_seeds_rule {
            // Non-seed count only: the bundled INACTIVE seed rule is always
            // present and must not veto minting (see `has_non_seed_advice_rule`).
            let state_has_rule = state.has_non_seed_advice_rule();
            checks.push(check(!state_has_rule, Reason::PreconditionFailed));
        }

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole pre-startup org-file seed effect (page-block insert, block
        // remap/normalize, index-layout classification, canonical re-sequencing,
        // pre-startup counter bump) lives in `RefDocumentsMut::seed_org_file`.
        state.seed_org_file(
            &self.filename,
            &self.blocks,
            self.keyword_set.as_ref().map(|ks| ks.0.clone()),
        );
    }
}

crate::cap_transition! {
    WriteOrgFile: SutFixtureFs,
    where R: [ RefDocumentsMut ],
    |me, state, sut| {
        let doc_name = std::path::Path::new(me.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&me.filename);

        // Serialise the generated blocks to org text. The blocks are parented
        // to `GEN_PLACEHOLDER`, so the renderer's file id must match for them
        // to land at the top level — this reproduces the exact text the
        // previous text-first generator emitted.
        let rendered = OrgRenderer::render_entitys(
            &me.blocks,
            std::path::Path::new(me.filename.as_str()),
            &EntityUri::block(GEN_PLACEHOLDER),
        );

        // Pin the document's identity into the file so production's
        // file_sync_controller picks up the same `block:ref-doc-N` URI the
        // reference state minted, instead of falling back to name-chain
        // resolution and assigning a fresh UUID. Without this the two ID
        // spaces diverge for documents (Page blocks), but agree for content
        // blocks — because content blocks already carry `:ID:` in the body.
        let content = match state.doc_uri_by_name(doc_name) {
            Some(uri) if holon_orgmode::parser::parse_doc_id(&rendered).is_none() => {
                format!("#+ID: {}\n{}", uri.id(), rendered)
            }
            _ => rendered,
        };
        // Axis 5: custom keywords (STARTED/NEXT/…) are not in the parser's
        // default set — without the `#+TODO:` header they'd re-parse as
        // headline content instead of task states.
        let content = match &me.keyword_set {
            Some(ks) => format!("{}\n{}", ks.to_org_header(), content),
            None => content,
        };
        sut.write_org_file(&me.filename, &content).await;
    }
    sql_budget: |_me, state| {
        // A budget of 0 was fiction: an external file write is ingested, so it
        // pays a full parse + home-resolution + CDC pass. Dedup reads 9-25
        // over 9 samples (9 ×6, 16, 18, 25; all d=4), composed of
        // `org.ingest_file`, `home.locate` (+ `home.resolve_doc`),
        // `home.prev_sibling`, `org.on_block_feed ▸ org.on_block_changed` and
        // `query_and_watch ▸ query_view ▸ query_view_ordered`.
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()) + 22,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

#[cfg(test)]
mod keyword_set_round_trip_tests {
    use holon_orgmode::OrgBlockExt;
    use holon_orgmode::OrgDocumentExt;

    use super::*;
    use crate::pbt::generators::generate_org_file_content_with_keywords;
    use crate::pbt::generators::todo_keyword_set_strategy;

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            failure_persistence: None,
            ..proptest::test_runner::Config::default()
        })]

        /// Axis-5 parity guard: a generated keyword set serialized the way
        /// `apply_to_sut` does (`#+TODO:` header + `render_entitys`) must
        /// re-parse to the same per-block task states and the same document
        /// keyword set the reference model adopts. Without the header,
        /// custom keywords (STARTED/NEXT/…) re-parse as headline content —
        /// this pins the divergence shut independent of slice sampling.
        #[test]
        fn keyword_set_survives_sut_serialize_parse(
            (ks, (filename, blocks)) in todo_keyword_set_strategy().prop_flat_map(|ks| {
                (Just(ks.clone()), generate_org_file_content_with_keywords(Some(ks), false, false))
            })
        ) {
            let placeholder = EntityUri::block(GEN_PLACEHOLDER);
            let rendered = OrgRenderer::render_entitys(
                &blocks,
                std::path::Path::new(filename.as_str()),
                &placeholder,
            );
            let content = format!("{}\n{}", ks.to_org_header(), rendered);

            let parsed = holon_orgmode::parse_org_file(
                std::path::Path::new(&filename),
                &content,
                &placeholder,
                std::path::Path::new("."),
            )
            .expect("generated org content must parse");

            // Document adopts the keyword set (what apply_to_ref mirrors).
            prop_assert_eq!(
                parsed.document.todo_keywords(),
                Some(ks.0.clone()),
                "doc todo_keywords must round-trip"
            );

            // Each generated block's task state survives the round-trip —
            // keyword AND category (the parser categorizes via the doc's
            // done-list, the generator via TaskState::from_keyword).
            for generated in &blocks {
                let reparsed = parsed
                    .blocks
                    .iter()
                    .find(|b| b.id == generated.id)
                    .unwrap_or_else(|| panic!("block {} lost in round-trip", generated.id));
                prop_assert_eq!(
                    reparsed.task_state(),
                    generated.task_state(),
                    "task_state diverged for block {} (content {:?})",
                    generated.id,
                    generated.content
                );
            }
        }
    }
}
