//! `inv-every-page-has-its-own-file` wired into the composed catalog — the
//! materialization-side twin of `inv-companion-has-no-child-page-headings`.
//! Every block the reference models as a `Page` doc-root must own EXACTLY ONE
//! file on disk after settle (`#+ID: <page-id>`): neither fileless (row 137,
//! the core Fork B loss) nor double-homed. Every file on disk roots a block
//! the reference holds, so a deleted page leaves no file.
//!
//! `Needs SutOrgRender` (SUT disk pairs) + `RefBlockTree` (page authority via
//! `is_page_block` over `all_non_seed_block_ids` — the sole page predicate,
//! OQ1). Selected only by a slice supplying both (the frontend slice over its
//! real `HeadlessFrontendComponent` disk + a block-tree ref).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefCopies;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::every_page_has_its_own_file::InvEveryPageHasItsOwnFile;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEveryPageHasItsOwnFile,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutOrgRender>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefBlockTree>(),
                CapId::of::<dyn RefCopies>(),
            ],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use holon_filesystem::FileSystem;
    use proptest::prelude::*;

    use super::*;
    use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;

    /// Bytes of a file the store roots no block for: empty, an `#+ID:` the
    /// store never held, or no `#+ID:` at all.
    fn unrooted_file_bytes() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(String::new()),
            "[a-z]{3,8}( [A-Za-z]{1,8}){0,3}"
                .prop_map(|s| format!("#+ID: stray-{}\n* {s}\n", s.replace(' ', "-"))),
            "[A-Za-z]{1,8}( [A-Za-z]{1,8}){0,5}\n",
        ]
    }

    fn check(files: &[String]) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let comp = HeadlessFrontendComponent::new(
                &[("Keeper.org", "#+ID: keeper\n* Keeper\n")],
                Duration::from_millis(300),
            )
            .await;
            // The watcher misses each write, so nothing ingests these files.
            for (i, bytes) in files.iter().enumerate() {
                let path = comp.org_root().join(format!("Stray{i}.org"));
                comp.org_fs().lose_next_change(&path);
                FileSystem::write(comp.org_fs().as_ref(), &path, bytes.as_bytes())
                    .await
                    .expect("write unrooted file");
            }
            let pairs = comp.snapshot_org_render_pairs(&Default::default()).await;
            for (i, bytes) in files.iter().enumerate() {
                let name = format!("Stray{i}.org");
                let (_, disk, rendered) = pairs
                    .iter()
                    .find(|(path, _, _)| path.ends_with(&name))
                    .unwrap_or_else(|| panic!("{name} missing from the render pairs"));
                assert_eq!(disk, bytes, "{name}: disk side");
                assert_eq!(
                    rendered, bytes,
                    "{name}: an unrooted file renders as its bytes"
                );
            }
        });
    }

    #[test]
    fn an_unrooted_file_renders_as_its_own_bytes_fixed_cases() {
        check(&[
            "#+ID: stray-axv-CvDGdwMf\n* axv CvDGdwMf\n".to_string(),
            String::new(),
        ]);
    }

    // Each case boots a full headless session, so the case count stays small.
    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 4,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]
        #[test]
        fn an_unrooted_file_renders_as_its_own_bytes(
            files in proptest::collection::vec(unrooted_file_bytes(), 1..4)
        ) {
            check(&files);
        }
    }
}
