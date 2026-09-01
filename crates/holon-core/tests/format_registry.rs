//! `FormatRegistry` routes a vault path to the adapter that claims its
//! extension, and refuses at CONSTRUCTION when two adapters claim one.
//!
//! The duplicate-claim refusal is the load-bearing one. Both markdown adapters
//! claim `md` and are separated by a vault-flavor discriminator that does not
//! exist yet; a first-wins `Vec` would let a wiring pick a flavor by accident,
//! where an `Err` forces the discriminator to be built before either is
//! registered.
//!
//! @pbt kind harness
//! @pbt covers format-registry-extension-routing — per-extension adapter
//! resolution, the duplicate-claim construction refusal, and the union of
//! registered extensions
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone drives a vault
//! through ONE format and never constructs a registry

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_core::file_format::FormatRegistry;
use holon_core::file_format::WriteTier;
use holon_core::file_format::WritebackDropVerdict;

/// A stub adapter that claims `exts` and answers nothing else — the registry
/// under test routes on extensions alone.
struct StubAdapter {
    exts: &'static [&'static str],
    tier: WriteTier,
}

impl FileFormatAdapter for StubAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        self.exts
    }
    fn write_tier(&self) -> WriteTier {
        self.tier
    }
    fn parse(&self, _: &Path, _: &str, _: &EntityUri, _: &Path) -> Result<FileFormatParseResult> {
        unimplemented!("routing-only stub")
    }
    fn render_document(&self, _: &Block, _: &[Block], _: &Path, _: &EntityUri) -> String {
        unimplemented!("routing-only stub")
    }
    fn render_blocks(&self, _: &[Block], _: &Path, _: &EntityUri) -> String {
        unimplemented!("routing-only stub")
    }
    fn doc_id_from_content(&self, _: &str) -> Option<String> {
        None
    }
    fn build_block_params(
        &self,
        _: &Block,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&Block>,
    ) -> StorageEntity {
        unimplemented!("routing-only stub")
    }
    fn content_differs(&self, _: &Block, _: &Block) -> bool {
        false
    }
    fn sync_document_metadata(&self, _: &Block, _: &mut Block) -> bool {
        false
    }
    fn writeback_drops(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: &[(&Path, &str)],
        _: &HashSet<String>,
        _: &Path,
    ) -> Result<WritebackDropVerdict> {
        unimplemented!("routing-only stub")
    }
}

fn adapter(exts: &'static [&'static str], tier: WriteTier) -> Arc<dyn FileFormatAdapter> {
    Arc::new(StubAdapter { exts, tier })
}

fn org_and_cook() -> FormatRegistry {
    FormatRegistry::new(vec![
        adapter(&["org"], WriteTier::ReadWrite),
        adapter(&["cook"], WriteTier::ReadOnly),
    ])
    .expect("disjoint extensions must build a registry")
}

#[test]
fn each_extension_routes_to_the_adapter_that_claims_it() {
    let registry = org_and_cook();

    let org = registry
        .adapter_for(Path::new("/vault/Notes.org"))
        .expect("`.org` is claimed");
    assert_eq!(org.write_tier(), WriteTier::ReadWrite);

    let cook = registry
        .adapter_for(Path::new("/vault/Pancakes.cook"))
        .expect("`.cook` is claimed");
    assert_eq!(cook.write_tier(), WriteTier::ReadOnly);
}

/// An unclaimed extension is NOT a vault document — a typed absence, not an
/// error. `require` is the other half: at a site the scan already admitted the
/// path to, an unclaimed extension is a wiring bug and must be loud.
#[test]
fn an_unclaimed_extension_is_a_typed_absence_but_a_loud_require() {
    let registry = org_and_cook();
    let stray = Path::new("/vault/notes.txt");

    assert!(registry.adapter_for(stray).is_none());
    assert!(!registry.handles(stray));

    // `match`, not `expect_err`: the Ok half is `&dyn FileFormatAdapter`,
    // which is not `Debug`.
    let msg = match registry.require(stray) {
        Ok(_) => panic!("require must refuse an unclaimed extension"),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        msg.contains("notes.txt") && msg.contains("org") && msg.contains("cook"),
        "the refusal must name the path AND every registered extension, so the wiring bug is \
         diagnosable from the message alone; got: {msg}"
    );
}

/// A path with no extension at all is likewise not a vault document.
#[test]
fn an_extensionless_path_is_not_a_vault_document() {
    let registry = org_and_cook();
    assert!(registry.adapter_for(Path::new("/vault/LICENSE")).is_none());
}

#[test]
fn extension_matching_is_case_insensitive() {
    let registry = org_and_cook();
    assert!(registry.handles(Path::new("/vault/Shouty.ORG")));
    assert!(registry.handles(Path::new("/vault/Recipe.Cook")));
}

/// The construction refusal: `md` claimed twice names BOTH claimants, because
/// the fix is to pick one (or build the vault-flavor discriminator), and a
/// message naming only the extension does not say which two to choose between.
#[test]
fn two_adapters_claiming_one_extension_is_a_construction_error() {
    let err = FormatRegistry::new(vec![
        adapter(&["org"], WriteTier::ReadWrite),
        adapter(&["md", "markdown"], WriteTier::ReadOnly),
        adapter(&["md"], WriteTier::ReadOnly),
    ])
    .expect_err("a duplicate extension claim must not build a registry");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("md"),
        "the refusal must name the contested extension; got: {msg}"
    );
}

#[test]
fn the_extension_union_is_every_claim_of_every_adapter() {
    let registry = FormatRegistry::new(vec![
        adapter(&["org"], WriteTier::ReadWrite),
        adapter(&["md", "markdown"], WriteTier::ReadOnly),
    ])
    .expect("disjoint extensions must build a registry");

    let mut union: Vec<&str> = registry.extensions().collect();
    union.sort_unstable();
    assert_eq!(union, vec!["markdown", "md", "org"]);
}

/// An empty registry is legal but claims nothing — every path is a typed
/// absence. Nothing may silently default to org.
#[test]
fn an_empty_registry_claims_nothing() {
    let registry = FormatRegistry::new(Vec::new()).expect("an empty registry is legal");
    assert_eq!(registry.extensions().count(), 0);
    assert!(
        registry
            .adapter_for(Path::new("/vault/Notes.org"))
            .is_none()
    );
}
