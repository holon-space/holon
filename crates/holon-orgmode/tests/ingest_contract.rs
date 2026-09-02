//! The generic ingest contract, driven through the real dispatcher with a
//! fixture format adapter, so what is pinned is the contract every adapter
//! rides (org, markdown, cooklang, a future plugin), not one format's parser.
//!
//! Four obligations, one per dogfood entry
//! (`docs/Testing/bugfunnel/entries/2026-09-02-*`):
//!
//! 1. A parsed document's title and property bag reach the persisted document
//!    block, on the first ingest and on re-ingest.
//! 2. A refused parse leaves no document block: ingest of one file is atomic.
//! 3. A read-only-tier document is never a re-render candidate.
//! 4. A refusal raises a degraded condition naming the refusing format, and
//!    that condition is retracted when the file next ingests.

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_core::file_format::FormatRegistry;
use holon_core::file_format::WriteTier;
use holon_core::file_format::WritebackDropVerdict;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::fs_port::RealFileSystem;
use holon_orgmode::file_format::OrgFormatAdapter;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

// ── The fixture format ──────────────────────────────────────────────────────

/// A `.fixture` adapter: the SMALLEST thing that satisfies the trait while
/// standing in for any format. Its parse is scripted (metadata, or a refusal)
/// and its write tier is a knob; everything the contract does not speak to is
/// delegated to org so the fixture stays about the contract.
struct FixtureAdapter {
    tier: WriteTier,
    /// When set, `parse` refuses with this message — a format adapter saying
    /// "this file is not mine to read", the way cooklang refuses an unknown
    /// timer unit.
    refuse: Option<String>,
    /// Restricts `refuse` to paths whose file name contains this needle, so one
    /// registry can hold a format that reads one file and refuses its sibling.
    refuse_only: Option<String>,
    /// What the parsed document declares about itself. Shared with the test so
    /// it can stand for the user editing the file's metadata between ingests.
    props: Arc<Mutex<Vec<(String, String)>>>,
    org: OrgFormatAdapter,
}

/// What a fixture file declares before any test edits it.
fn declared_props() -> Arc<Mutex<Vec<(String, String)>>> {
    Arc::new(Mutex::new(vec![
        ("servings".to_string(), "4".to_string()),
        ("source".to_string(), "Familienrezept".to_string()),
    ]))
}

impl FixtureAdapter {
    fn parsing(tier: WriteTier) -> Self {
        Self {
            tier,
            refuse: None,
            refuse_only: None,
            props: declared_props(),
            org: OrgFormatAdapter::new(),
        }
    }

    fn refusing(reason: &str) -> Self {
        Self {
            tier: WriteTier::ReadOnly,
            refuse: Some(reason.to_string()),
            refuse_only: None,
            props: declared_props(),
            org: OrgFormatAdapter::new(),
        }
    }

    /// Refuses only the files whose name contains `needle`; reads the rest.
    fn refusing_only(reason: &str, needle: &str) -> Self {
        Self {
            refuse_only: Some(needle.to_string()),
            ..Self::refusing(reason)
        }
    }
}

impl FileFormatAdapter for FixtureAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["fixture"]
    }

    fn write_tier(&self) -> WriteTier {
        self.tier
    }

    fn format_name(&self) -> &'static str {
        "fixture"
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> anyhow::Result<FileFormatParseResult> {
        if let Some(reason) = &self.refuse {
            let mine = self.refuse_only.as_ref().is_none_or(|needle| {
                path.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(needle.as_str()))
            });
            if mine {
                anyhow::bail!("{}: {reason}", path.display());
            }
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        let file_id = EntityUri::file(&rel.to_string_lossy());
        let mut document = Block::new_text(
            file_id.clone(),
            parent_dir_id.clone(),
            DECLARED_TITLE.to_string(),
        );
        document.set_page(true);
        for (key, value) in self.props.lock().unwrap().iter() {
            document.set_property(key.clone(), value.clone());
        }

        let blocks = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, line)| {
                Block::new_text(
                    EntityUri::block(&format!("{}::b::{i}", file_id.id())),
                    file_id.clone(),
                    line.trim().to_string(),
                )
            })
            .collect();

        Ok(FileFormatParseResult {
            document,
            blocks,
            blocks_needing_ids: Vec::new(),
            typed_rows: Vec::new(),
        })
    }

    fn render_document(
        &self,
        document: &Block,
        blocks: &[Block],
        path: &Path,
        id: &EntityUri,
    ) -> String {
        self.org.render_document(document, blocks, path, id)
    }

    fn render_blocks(&self, blocks: &[Block], path: &Path, id: &EntityUri) -> String {
        self.org.render_blocks(blocks, path, id)
    }

    fn doc_id_from_content(&self, _: &str) -> Option<String> {
        // Like cooklang: the format embeds no identity, so the controller
        // resolves the document by its path-derived name chain.
        None
    }

    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
        previous: Option<&Block>,
    ) -> holon_api::StorageEntity {
        self.org
            .build_block_params(block, parent_id, document_uri, previous)
    }

    fn content_differs(&self, a: &Block, b: &Block) -> bool {
        a.content != b.content
    }

    fn writeback_drops(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: &[(&Path, &str)],
        _: &HashSet<String>,
        _: &Path,
    ) -> anyhow::Result<WritebackDropVerdict> {
        Ok(WritebackDropVerdict::default())
    }
}

/// The title the fixture's parsed document declares — deliberately NOT the file
/// stem, which is what the walk would name the page.
const DECLARED_TITLE: &str = "Spaghetti Carbonara";

// ── Store doubles ───────────────────────────────────────────────────────────

/// A document store that behaves like the real one on the two axes this
/// contract touches: it REMEMBERS what was created, and it answers
/// `find_by_parent_and_name` from that memory — so "did the ingest leave a
/// document block behind?" is an honest question to ask it.
#[derive(Default)]
struct RecordingDocManager {
    docs: Mutex<HashMap<EntityUri, Block>>,
}

impl RecordingDocManager {
    fn get(&self, id: &EntityUri) -> Option<Block> {
        self.docs.lock().unwrap().get(id).cloned()
    }

    /// Every page the store holds, by title — the sidebar's view of the vault.
    fn titles(&self) -> Vec<String> {
        self.docs
            .lock()
            .unwrap()
            .values()
            .map(Block::title)
            .collect()
    }
}

#[async_trait]
impl DocumentManager for RecordingDocManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .docs
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.title() == title)
            .cloned())
    }

    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.docs
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.get(id))
    }

    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.docs
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

/// Block store double: holds whatever the ingest places into it.
#[derive(Default)]
struct RecordingReader {
    blocks: Mutex<Vec<Block>>,
}

#[async_trait]
impl BlockReader for RecordingReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *doc_id)
            .cloned()
            .collect())
    }

    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .get_blocks(doc_id)
            .await?
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == *id)
            .cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// The consolidator's downstream sink. `create_in_tree` reporting `true` means
/// a separate consolidator persisted the create, and the controller then
/// REFUSES to proceed without a sink for it — so the double must be present for
/// the ingest to run at all. Nothing to flush: this harness's ordering writes
/// straight into the block store.
struct DirectProjection;

#[async_trait]
impl holon_core::downstream_projection::DownstreamProjection for DirectProjection {
    async fn flush(
        &self,
    ) -> holon_core::traits::Result<holon_core::downstream_projection::ProjectionPass> {
        Ok(holon_core::downstream_projection::ProjectionPass::Converged)
    }
}

/// Ordering double that lands `create_in_tree` into the block store, so the
/// blocks an ingest emits are observable the way production's are.
struct RecordingOrdering {
    reader: Arc<RecordingReader>,
}

#[async_trait]
impl BlockOrdering for RecordingOrdering {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        Ok(())
    }
    async fn prev_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn next_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn first_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn last_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn children(&self, parent_id: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .reader
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id.clone())
            .collect())
    }
    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        _: Option<&EntityUri>,
        id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &HashMap<String, holon_api::Value>,
        _: &holon_api::BlockEdges,
    ) -> OrderingResult<bool> {
        let mut block = Block::new_text(
            id.clone(),
            parent_id.clone(),
            content.as_text().unwrap_or_default().to_string(),
        );
        block.properties = properties.clone();
        let mut blocks = self.reader.blocks.lock().unwrap();
        match blocks.iter_mut().find(|b| b.id == *id) {
            Some(existing) => *existing = block,
            None => blocks.push(block),
        }
        Ok(true)
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

// ── Harness ─────────────────────────────────────────────────────────────────

struct Vault {
    controller: FileSyncController,
    docs: Arc<RecordingDocManager>,
    /// The metadata the fixture file declares; edit it between ingests to stand
    /// for the user changing the file's own metadata.
    props: Arc<Mutex<Vec<(String, String)>>>,
    _tmp: tempfile::TempDir,
    file: PathBuf,
}

impl Vault {
    /// The one stored document block.
    fn document(&self) -> Block {
        self.docs
            .docs
            .lock()
            .unwrap()
            .values()
            .next()
            .cloned()
            .expect("a successful ingest stores a document block")
    }
}

fn vault(adapter: FixtureAdapter, contents: &str) -> Vault {
    let tmp = tempfile::tempdir().unwrap();
    // macOS hands out `/var/...` temp dirs that canonicalize to `/private/var/...`;
    // the controller strips the root prefix from each path, so both must agree.
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let file = root.join("Spaghetti-Carbonara.fixture");
    std::fs::write(&file, contents).unwrap();

    let reader = Arc::new(RecordingReader::default());
    let docs = Arc::new(RecordingDocManager::default());
    let props = adapter.props.clone();
    let formats = Arc::new(FormatRegistry::new(vec![Arc::new(adapter)]).unwrap());
    let controller = FileSyncController::with_formats(
        reader.clone(),
        docs.clone(),
        root,
        formats,
        Arc::new(RecordingOrdering {
            reader: reader.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_downstream_projection(Arc::new(DirectProjection));
    Vault {
        controller,
        docs,
        props,
        _tmp: tmp,
        file,
    }
}

// ── 1. Parsed document metadata survives storage ────────────────────────────

/// Entry `a-cook-recipe-loses-its-title-and-all-its-metadata`, stated
/// generically: whatever an adapter's parsed document declares about itself —
/// its title and every property — is on the stored document block afterwards.
/// The adapter's return value already carried them; what was never pinned is
/// the hop from there into the store.
#[tokio::test]
async fn a_parsed_documents_title_and_properties_reach_the_document_block() {
    let mut v = vault(
        FixtureAdapter::parsing(WriteTier::ReadOnly),
        "Boil water\nStir\n",
    );
    let path = v.file.clone();
    v.controller
        .on_file_changed(&path)
        .await
        .expect("the fixture file ingests");

    let doc = v
        .docs
        .docs
        .lock()
        .unwrap()
        .values()
        .next()
        .cloned()
        .expect("a successful ingest stores a document block");

    assert_eq!(
        doc.get_property_str(holon_core::DOCUMENT_TITLE_KEY)
            .as_deref(),
        Some(DECLARED_TITLE),
        "the document's DECLARED title never reached the store — the page renders \
         the file stem and the title the file states is lost",
    );
    assert_eq!(
        doc.get_property_str("servings").as_deref(),
        Some("4"),
        "the parsed document's properties never reached the store — `servings` \
         exists nowhere in the system, with no error",
    );
    assert_eq!(
        doc.get_property_str("source").as_deref(),
        Some("Familienrezept"),
        "the parsed document's properties never reached the store",
    );
}

/// The contract holds on RE-ingest too — a second pass must not drop what the
/// first stored, nor mint a second page for the same file.
#[tokio::test]
async fn re_ingest_keeps_the_documents_metadata_and_mints_no_second_page() {
    let mut v = vault(FixtureAdapter::parsing(WriteTier::ReadOnly), "Boil water\n");
    let path = v.file.clone();
    v.controller.on_file_changed(&path).await.unwrap();
    std::fs::write(&path, "Boil water\nStir\n").unwrap();
    v.controller
        .on_file_changed(&path)
        .await
        .expect("the edited fixture file re-ingests");

    assert_eq!(
        v.docs.titles().len(),
        1,
        "a re-ingest minted a SECOND page for one file: {:?}",
        v.docs.titles(),
    );
    let doc = v
        .docs
        .docs
        .lock()
        .unwrap()
        .values()
        .next()
        .cloned()
        .unwrap();
    assert_eq!(
        doc.get_property_str(holon_core::DOCUMENT_TITLE_KEY)
            .as_deref(),
        Some(DECLARED_TITLE),
        "the re-ingest dropped the document's declared title",
    );
    assert_eq!(
        doc.get_property_str("servings").as_deref(),
        Some("4"),
        "the re-ingest dropped the document's properties",
    );
}

/// The file is the authority for its own metadata in BOTH directions: a key the
/// user deleted from it is gone from the document block after the next ingest.
/// Entry `2026-09-03-deleted-file-metadata-survives-re-ingest`.
#[tokio::test]
async fn re_ingest_removes_metadata_the_file_no_longer_declares() {
    let mut v = vault(FixtureAdapter::parsing(WriteTier::ReadOnly), "Boil water\n");
    let path = v.file.clone();
    v.controller.on_file_changed(&path).await.unwrap();

    v.props.lock().unwrap().retain(|(k, _)| k != "source");
    std::fs::write(&path, "Boil water\nStir\n").unwrap();
    v.controller
        .on_file_changed(&path)
        .await
        .expect("the edited fixture file re-ingests");

    let doc = v.document();
    assert_eq!(
        doc.get_property_str("source"),
        None,
        "the property the user DELETED from the file is still on the document \
         block — the store shows metadata the file does not have. Props: {:?}",
        doc.properties,
    );
    assert_eq!(
        doc.get_property_str("servings").as_deref(),
        Some("4"),
        "the removal swept a property the file still declares",
    );
    assert_eq!(
        doc.get_property_str(holon_core::DOCUMENT_TITLE_KEY)
            .as_deref(),
        Some(DECLARED_TITLE),
        "the removal swept the document's declared title",
    );
}

// ── 2. A refused parse is atomic ────────────────────────────────────────────

/// Entry `a-refused-cook-file-still-leaves-a-document-block`: an adapter that
/// refuses a file must leave NOTHING of it in the store. A document block for
/// a file whose content was never read is an empty page in the sidebar and a
/// permanent write-tier refusal about changes that exist only because the file
/// was refused.
#[tokio::test]
async fn a_refused_parse_leaves_no_document_block() {
    let mut v = vault(
        FixtureAdapter::refusing("unknown timer unit"),
        "Boil water for ~{5%Minuten}\n",
    );
    let path = v.file.clone();
    let outcome = v.controller.on_file_changed(&path).await;

    let err = outcome.expect_err("a refused parse must fail the ingest, loudly");
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("unknown timer unit"),
        "the refusal must carry the adapter's reason; got: {rendered}",
    );
    assert!(
        rendered.contains("Spaghetti-Carbonara.fixture"),
        "the refusal must name the file that was refused; got: {rendered}",
    );
    assert!(
        v.docs.titles().is_empty(),
        "a REFUSED file left a document block behind: {:?} — the sidebar grows an \
         empty page for a file nothing was read from, and every later write-back \
         trips the tier gate over it",
        v.docs.titles(),
    );
}

// ── 3. A read-only tier is not a re-render candidate ────────────────────────

/// Entry `re-render-all-tracked-renders-read-only-cook-files`: the debounced
/// re-render walks every TRACKED file. A read-only-tier document must be
/// skipped by the CALLER, so the write-tier gate — whose own message reads
/// "reaching this render means a write path skipped the gate" — is never asked
/// about a healthy file.
#[tokio::test]
async fn a_read_only_document_is_not_a_re_render_candidate() {
    let mut v = vault(FixtureAdapter::parsing(WriteTier::ReadOnly), "Boil water\n");
    let path = v.file.clone();
    v.controller.on_file_changed(&path).await.unwrap();

    v.controller
        .re_render_all_tracked(&HashSet::new())
        .await
        .expect(
            "the debounced re-render raised the write-tier gate over a read-only \
             file — an ERROR on the ordinary success path, once per ingest",
        );

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after, "Boil water\n",
        "the re-render wrote over an authoritative read-only file",
    );
}

/// The same walk must still re-render a WRITABLE document — the skip is the
/// tier's, not a blanket disabling of re-render.
#[tokio::test]
async fn a_writable_document_is_still_a_re_render_candidate() {
    let mut v = vault(
        FixtureAdapter::parsing(WriteTier::ReadWrite),
        "Boil water\n",
    );
    let path = v.file.clone();
    v.controller.on_file_changed(&path).await.unwrap();

    v.controller
        .re_render_all_tracked(&HashSet::new())
        .await
        .expect("a writable document re-renders");
}

/// INFO-level tracing capture, the shape `vault_path_escape.rs` uses for ERROR.
#[derive(Clone, Default)]
struct InfoCapture(Arc<Mutex<Vec<String>>>);

impl InfoCapture {
    fn skip_disclosures(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.contains("not a re-render candidate"))
            .count()
    }
}

struct MsgVisitor<'a>(&'a mut String);
impl Visit for MsgVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

impl<S: tracing::Subscriber> Layer<S> for InfoCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::INFO {
            let mut buf = String::new();
            event.record(&mut MsgVisitor(&mut buf));
            self.0.lock().unwrap().push(buf);
        }
    }
}

/// The skip is disclosed once per file per incarnation. A supervisor `Reset`
/// starts a new one, so the same file's next skip is loud again.
#[tokio::test]
async fn a_reset_re_arms_the_read_only_skip_disclosure() {
    let cap = InfoCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let mut v = vault(FixtureAdapter::parsing(WriteTier::ReadOnly), "Boil water\n");
    let path = v.file.clone();
    v.controller.on_file_changed(&path).await.unwrap();

    v.controller
        .re_render_all_tracked(&HashSet::new())
        .await
        .unwrap();
    v.controller
        .re_render_all_tracked(&HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        cap.skip_disclosures(),
        1,
        "a healthy steady state was disclosed on every re-render burst",
    );

    v.controller.reset_holder();
    v.controller
        .re_render_all_tracked(&HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        cap.skip_disclosures(),
        2,
        "the skip set outlived the incarnation it describes: after a Reset the \
         file is silently skipped and the set never shrinks",
    );
}

// ── 4. The degraded signal names the format and clears on recovery ──────────

/// Records what the controller disclosed about ingest, in order.
#[derive(Default)]
struct DisclosureLog {
    entries: Mutex<Vec<String>>,
    /// The refusal SUBJECTS alone, unmixed with the reason — the reason names
    /// the path too, so a rendered line cannot tell what the condition is keyed
    /// by.
    subjects: Mutex<Vec<PathBuf>>,
}

impl DisclosureLog {
    fn entries(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }

    fn subject_names(&self) -> Vec<String> {
        self.subjects
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }
}

impl holon_filesystem::WritebackDisclosure for DisclosureLog {
    fn writeback_degraded(&self, _: &str) {}

    fn ingest_refused(&self, path: &Path, format: &str, reason: &str) {
        self.subjects.lock().unwrap().push(path.to_path_buf());
        self.entries.lock().unwrap().push(format!(
            "refused {} format={format} reason={reason}",
            path.display()
        ));
    }

    fn ingest_recovered(&self, path: &Path) {
        self.entries
            .lock()
            .unwrap()
            .push(format!("recovered {}", path.display()));
    }
}

/// Entry `the-degraded-toast-is-stale-and-calls-cook-files-org`: the refusal
/// signal must name the FORMAT that refused (from the registry, never a
/// hard-coded "org"), be keyed by the FILE, and be RETRACTED the moment that
/// file's next ingest succeeds — a banner that outlives its own repair trains
/// the user to disbelieve every banner.
#[tokio::test]
async fn a_refusal_names_its_format_and_is_retracted_when_the_file_ingests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let file = root.join("Spaghetti-Carbonara.fixture");
    std::fs::write(&file, "Boil water\n").unwrap();

    let log = Arc::new(DisclosureLog::default());
    let reader = Arc::new(RecordingReader::default());
    let docs = Arc::new(RecordingDocManager::default());

    // The refusing adapter first: the file is bad on this boot.
    let refusing = Arc::new(
        FormatRegistry::new(vec![Arc::new(FixtureAdapter::refusing(
            "unknown timer unit",
        ))])
        .unwrap(),
    );
    let mut controller = FileSyncController::with_formats(
        reader.clone(),
        docs.clone(),
        root.clone(),
        refusing,
        Arc::new(RecordingOrdering {
            reader: reader.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_downstream_projection(Arc::new(DirectProjection))
    .with_writeback_disclosure(log.clone());

    controller
        .on_file_changed(&file)
        .await
        .expect_err("the refusing adapter refuses");

    let refusal = log.entries().first().cloned().expect(
        "a refused file raised NO degraded condition — the app looks healthy while \
         one of its files is not ingested",
    );
    assert!(
        refusal.contains("format=fixture"),
        "the condition must name the format that refused, from the registry — a \
         message naming the wrong format sends the reader to look for a defect the \
         file cannot have. Got: {refusal}",
    );
    assert!(
        refusal.contains("unknown timer unit"),
        "the condition must carry the adapter's reason. Got: {refusal}",
    );
    assert!(
        refusal.contains("Spaghetti-Carbonara.fixture"),
        "the condition must name the file, so a repair lifts that file's banner and \
         leaves the others standing. Got: {refusal}",
    );

    // The file is repaired on disk: a parsing adapter now reads it.
    let parsing = Arc::new(
        FormatRegistry::new(vec![Arc::new(FixtureAdapter::parsing(WriteTier::ReadOnly))]).unwrap(),
    );
    let mut repaired = FileSyncController::with_formats(
        reader.clone(),
        docs.clone(),
        root,
        parsing,
        Arc::new(RecordingOrdering {
            reader: reader.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_downstream_projection(Arc::new(DirectProjection))
    .with_writeback_disclosure(log.clone());

    repaired
        .on_file_changed(&file)
        .await
        .expect("the repaired file ingests");

    let entries = log.entries();
    assert!(
        entries.iter().any(|e| e.starts_with("recovered ")),
        "the repaired file's successful ingest retracted NOTHING — the degraded \
         banner survives the fix it describes. Log: {entries:?}",
    );
}

/// The condition's SUBJECT is the file, asserted on its own: a rendered line
/// folds in the reason, which already names the path, so it holds whatever the
/// keying is. One refused file among healthy siblings of the same format must
/// raise one condition, named after that file — key it by the format instead
/// and one file's repair lifts every other refused file's banner.
#[tokio::test]
async fn a_refusal_is_keyed_by_the_file_not_by_its_format() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let good = root.join("Linsensuppe.fixture");
    let bad = root.join("Spaghetti-Carbonara.fixture");
    std::fs::write(&good, "Boil water\n").unwrap();
    std::fs::write(&bad, "Boil water for ~{5%Minuten}\n").unwrap();

    let log = Arc::new(DisclosureLog::default());
    let reader = Arc::new(RecordingReader::default());
    let docs = Arc::new(RecordingDocManager::default());
    let formats = Arc::new(
        FormatRegistry::new(vec![Arc::new(FixtureAdapter::refusing_only(
            "unknown timer unit",
            "Spaghetti",
        ))])
        .unwrap(),
    );
    let mut controller = FileSyncController::with_formats(
        reader.clone(),
        docs.clone(),
        root,
        formats,
        Arc::new(RecordingOrdering {
            reader: reader.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_downstream_projection(Arc::new(DirectProjection))
    .with_writeback_disclosure(log.clone());

    controller
        .on_file_changed(&good)
        .await
        .expect("the healthy sibling ingests");
    controller
        .on_file_changed(&bad)
        .await
        .expect_err("the adapter refuses the bad file");

    assert_eq!(
        log.subject_names(),
        vec![bad.display().to_string()],
        "the refusal is not keyed by the file it is about, so one file's repair \
         clears every other refused file's banner",
    );
}
