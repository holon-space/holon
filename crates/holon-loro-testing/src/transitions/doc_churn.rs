//! Churn on one `LoroBackend` doc as a poller of that doc sees it: local
//! edits, an import from a concurrent peer, an edit of an attached shared tree
//! and idle spans. A peer forks from the doc as it stood after the root was
//! made, so its ops can carry Lamport stamps below the doc's own.

use std::sync::Arc;

use holon::api::repository::CoreOperations;
use holon::api::repository::Lifecycle;
use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_loro::LoroBackend;
use holon_loro::LoroDocument;
use holon_loro::shared_tree::InMemorySharedTreeStore;
use loro::ExportMode;
use loro::LoroDoc;
use proptest::prelude::*;

#[derive(Clone, Debug)]
pub enum ChurnContent {
    Text(String),
    Source { language: String, code: String },
}

impl ChurnContent {
    fn block_content(&self) -> BlockContent {
        match self {
            Self::Text(text) => BlockContent::text(text.clone()),
            Self::Source { language, code } => BlockContent::source(language.clone(), code.clone()),
        }
    }
}

/// Block and target indices are taken modulo the live blocks at apply time.
#[derive(Clone, Debug)]
pub enum DocChurnStep {
    Create {
        parent: usize,
        content: ChurnContent,
    },
    Update {
        target: usize,
        content: ChurnContent,
    },
    Delete {
        target: usize,
    },
    ConcurrentImport {
        creates: Vec<ChurnContent>,
    },
    SharedTreeEdit {
        tree: usize,
    },
    Idle {
        polls: usize,
    },
}

/// Every source language the refresh treats differently, plus one it ignores.
fn source_language() -> impl Strategy<Value = String> {
    let mut languages: Vec<String> = QueryLanguage::ALL.iter().map(|l| l.to_string()).collect();
    languages.extend(
        [
            "holon_rule",
            "action",
            "render",
            "holon_filter",
            "holon_entity_profile_yaml",
            "python",
        ]
        .map(String::from),
    );
    proptest::sample::select(languages)
}

pub fn churn_content() -> impl Strategy<Value = ChurnContent> {
    prop_oneof![
        "[a-z ]{0,8}".prop_map(ChurnContent::Text),
        (source_language(), "[a-z: ]{0,8}")
            .prop_map(|(language, code)| ChurnContent::Source { language, code }),
    ]
}

pub fn doc_churn_step(max_idle_polls: usize) -> impl Strategy<Value = DocChurnStep> {
    prop_oneof![
        3 => (any::<usize>(), churn_content())
            .prop_map(|(parent, content)| DocChurnStep::Create { parent, content }),
        2 => (any::<usize>(), churn_content())
            .prop_map(|(target, content)| DocChurnStep::Update { target, content }),
        1 => any::<usize>().prop_map(|target| DocChurnStep::Delete { target }),
        2 => proptest::collection::vec(churn_content(), 1..=3)
            .prop_map(|creates| DocChurnStep::ConcurrentImport { creates }),
        1 => any::<usize>().prop_map(|tree| DocChurnStep::SharedTreeEdit { tree }),
        2 => (1..=max_idle_polls).prop_map(|polls| DocChurnStep::Idle { polls }),
    ]
}

/// A real backend plus the handles the steps need: the root every peer
/// shares, the doc state peers fork from, and the attached shared-tree docs.
pub struct DocChurn {
    pub backend: Arc<LoroBackend>,
    pub root: EntityUri,
    fork_base: Vec<u8>,
    shared_trees: Vec<Arc<LoroDoc>>,
}

impl DocChurn {
    pub async fn new(shared_tree_count: usize) -> anyhow::Result<Self> {
        let mut store = InMemorySharedTreeStore::new();
        let shared_trees: Vec<Arc<LoroDoc>> = (0..shared_tree_count)
            .map(|_| Arc::new(LoroDoc::new()))
            .collect();
        for (i, doc) in shared_trees.iter().enumerate() {
            store.insert_arc(format!("shared-{i}"), Arc::clone(doc));
        }
        let mut backend =
            LoroBackend::create_new("churn".into(), Arc::new(holon_api::SystemClock)).await?;
        if shared_tree_count > 0 {
            backend.set_shared_trees(Arc::new(store));
        }
        let root = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
            .await?
            .id;
        let fork_base = backend.collab_for_test().export_snapshot()?;
        Ok(Self {
            backend: Arc::new(backend),
            root,
            fork_base,
            shared_trees,
        })
    }

    /// Apply `step`; `Idle` changes nothing here, the caller polls.
    /// `live` is every block of the doc but the root, in a stable order.
    pub async fn apply(&mut self, step: &DocChurnStep, live: &[EntityUri]) -> anyhow::Result<()> {
        match step {
            DocChurnStep::Create { parent, content } => {
                let parent = match parent % (live.len() + 1) {
                    0 => self.root.clone(),
                    i => live[i - 1].clone(),
                };
                self.backend
                    .create_block(parent, content.block_content(), None)
                    .await?;
            }
            DocChurnStep::Update { target, content } if !live.is_empty() => {
                let id = &live[target % live.len()];
                self.backend
                    .update_block(id.as_str(), content.block_content())
                    .await?;
            }
            DocChurnStep::Delete { target } if !live.is_empty() => {
                self.backend
                    .delete_block(live[target % live.len()].as_str())
                    .await?;
            }
            DocChurnStep::Update { .. } | DocChurnStep::Delete { .. } => {}
            DocChurnStep::ConcurrentImport { creates } => {
                let peer_doc = Arc::new(LoroDocument::new("churn".into())?);
                peer_doc.apply_update(&self.fork_base)?;
                let peer = LoroBackend::from_document(
                    Arc::clone(&peer_doc),
                    Arc::new(holon_api::SystemClock),
                );
                for content in creates {
                    peer.create_block(self.root.clone(), content.block_content(), None)
                        .await?;
                }
                let primary = self.backend.collab_for_test();
                let since = primary.with_read(|doc| Ok(doc.oplog_vv()))?;
                let update =
                    peer_doc.with_read(|doc| Ok(doc.export(ExportMode::updates(&since))?))?;
                primary.apply_update(&update)?;
            }
            DocChurnStep::SharedTreeEdit { tree } if !self.shared_trees.is_empty() => {
                let doc = &self.shared_trees[tree % self.shared_trees.len()];
                doc.get_tree("tree").create(None)?;
                doc.commit();
            }
            DocChurnStep::SharedTreeEdit { .. } | DocChurnStep::Idle { .. } => {}
        }
        Ok(())
    }
}
