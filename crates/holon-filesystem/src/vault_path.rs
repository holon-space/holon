//! Containment-proof write-back targets.
//!
//! Page files are named by a page's name chain. A chain segment that is empty,
//! `.`, `..`, or itself absolute silently turns `root.join(…)` into a path
//! OUTSIDE the vault — `join("")` names the root's sibling, and an absolute
//! component discards the base entirely. [`VaultPath`] is the single gate that
//! makes such a target unrepresentable.

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::bail;

/// A write-back target PROVEN to be a strict descendant of the vault root.
///
/// The invariant is established once, at construction; a value of this type is
/// a proof that the path is inside the vault, so no downstream caller has to
/// re-check (and none can forget to).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultPath(PathBuf);

impl VaultPath {
    /// Derive the page file `<root>/<seg₁>/…/<segₙ>.org` from a page's name
    /// chain.
    ///
    /// An empty chain is an Err, not a path: "this block owns no file" is a
    /// distinct verdict the caller must reach BEFORE asking for a path.
    pub fn page_file_from_name_chain(root: &Path, chain: &[String]) -> Result<Self> {
        if chain.is_empty() {
            bail!(
                "page-file derivation under vault root '{}': an EMPTY name chain names no page \
                 file — 'owns no file' must be decided before a path is derived",
                root.display()
            );
        }
        let mut path = root.to_path_buf();
        for segment in chain {
            if segment.is_empty() || segment == "." || segment == ".." {
                bail!(
                    "page-file derivation under vault root '{}': name-chain segment '{segment}' \
                     names no directory entry, so the chain cannot name a file inside the vault \
                     root; chain = {chain:?}",
                    root.display()
                );
            }
            path.push(segment);
        }
        Self::inside(root, path.with_extension("org"))
    }

    /// Accept an already-built path only if it is a strict descendant of
    /// `root`.
    pub fn inside(root: &Path, path: PathBuf) -> Result<Self> {
        let normalized_root = lexically_normalize(root);
        let normalized = lexically_normalize(&path);
        if normalized == normalized_root || !normalized.starts_with(&normalized_root) {
            bail!(
                "'{}' lies OUTSIDE the vault root '{}' — refusing it as a write-back target",
                path.display(),
                root.display()
            );
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

/// Resolve `.` and `..` textually — the target need not exist on disk, so
/// `canonicalize` is not available. A `..` that would climb past the start
/// simply shortens the path, which the descendant check then rejects.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nested_chain_derives_a_descendant_page_file() {
        let root = Path::new("/vault");
        let derived =
            VaultPath::page_file_from_name_chain(root, &chain(&["Projects", "Optimize RAG"]))
                .expect("a well-formed chain must derive");
        assert_eq!(
            derived.as_path(),
            Path::new("/vault/Projects/Optimize RAG.org")
        );
    }

    /// The live-vault husk shape: `join("")` would name the root's SIBLING.
    #[test]
    fn empty_only_chain_is_refused() {
        let err = VaultPath::page_file_from_name_chain(Path::new("/vault"), &chain(&[""]))
            .expect_err("an empty segment must not derive a path");
        assert!(format!("{err:#}").contains("vault root"), "{err:#}");
    }

    /// The stranded-child shape: an empty leading segment made
    /// `chain.join("/")` absolute, which discarded the base.
    #[test]
    fn empty_leading_segment_is_refused() {
        VaultPath::page_file_from_name_chain(Path::new("/vault"), &chain(&["", "Optimize RAG"]))
            .expect_err("an empty leading segment must not derive a path");
    }

    #[test]
    fn dot_dot_climb_out_is_refused() {
        VaultPath::page_file_from_name_chain(Path::new("/vault"), &chain(&["..", "escaped"]))
            .expect_err("a climbing segment must not derive a path");
        VaultPath::inside(
            Path::new("/vault"),
            PathBuf::from("/vault/sub/../../escaped.org"),
        )
        .expect_err("a path that climbs out must be refused");
    }

    #[test]
    fn empty_chain_is_refused() {
        VaultPath::page_file_from_name_chain(Path::new("/vault"), &[])
            .expect_err("an empty chain names no page file");
    }

    #[test]
    fn the_root_itself_is_not_a_page_file() {
        VaultPath::inside(Path::new("/vault"), PathBuf::from("/vault"))
            .expect_err("the root is not a strict descendant of itself");
    }

    /// String-prefix containment would wrongly accept a SIBLING directory whose
    /// name extends the root's.
    #[test]
    fn sibling_with_a_shared_name_prefix_is_refused() {
        VaultPath::inside(Path::new("/vault"), PathBuf::from("/vault-backup/page.org"))
            .expect_err("a sibling sharing a name prefix is not inside the root");
    }
}
