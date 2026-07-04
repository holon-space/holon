//! `inv-no-write-outside-vault-root` — the vault is a write BOUNDARY.
//!
//! @pbt oracle internal-consistency — every path the filesystem was asked to
//!   write or create lies under the run's vault root (no ref)
//! @pbt covers vault-containment — a path derived from author- or sync-supplied
//!   content that escapes the vault root
//! @pbt slips-if-removed a traversal segment in block content reaches
//!   `root.join(…)` unnormalized and the app writes bytes anywhere on the
//!   user's disk
//!
//! Holon owns exactly one directory. Every path it writes is DERIVED — from a
//! page's name chain, from an image block's content, from a `file:` URI — and
//! each of those sources carries data the user or a synced peer authored. A
//! derivation that yields `<root>/../x` is not a rare edge case; it is what a
//! hostile or merely malformed peer document produces.
//!
//! Component-wise containment after `..` resolution, not a string prefix:
//! `<root>-backup/x` shares a textual prefix with `<root>` and is outside it.

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use holon_pbt_core::capabilities::SutFsWrites;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvNoWriteOutsideVaultRoot;

impl InvNoWriteOutsideVaultRoot {
    pub const ID: InvariantId = InvariantId("inv-no-write-outside-vault-root");
}

/// Resolve `.` and `..` textually — a write target need not exist on disk, so
/// `canonicalize` is unavailable. Mirrors `holon_filesystem::VaultPath`'s
/// normalization, which is what production proves containment with.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoWriteOutsideVaultRoot
where
    S: SutFsWrites,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let (root, targets) = sut.vault_write_targets().await;
        let normalized_root = lexically_normalize(Path::new(&root));

        // The root's own ancestors are created by harness setup before the
        // vault exists; only targets AT or BELOW the root are the app's.
        let escaped: Vec<&String> = targets
            .iter()
            .filter(|t| {
                let normalized = lexically_normalize(Path::new(t));
                !normalized.starts_with(&normalized_root)
                    && !normalized_root.starts_with(&normalized)
            })
            .collect();

        match escaped.first() {
            Some(first) => InvariantResult::Fail(format!(
                "{} write target(s) escaped the vault root '{}' — first: '{}' (normalizes to \
                 '{}'). A derived path left the one directory Holon owns; the bytes landed on \
                 the user's disk outside the vault. All escapes: {:?}",
                escaped.len(),
                root,
                first,
                lexically_normalize(Path::new(first)).display(),
                escaped,
            )),
            None => InvariantResult::Ok,
        }
    }
}
