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
        let last = chain.len() - 1;
        for (i, segment) in chain.iter().enumerate() {
            if segment.is_empty() || segment == "." || segment == ".." {
                bail!(
                    "page-file derivation under vault root '{}': name-chain segment '{segment}' \
                     names no directory entry, so the chain cannot name a file inside the vault \
                     root; chain = {chain:?}",
                    root.display()
                );
            }
            // APPEND, never `with_extension`: that REPLACES whatever follows the
            // leaf's last dot, so `citrix-STX.BROWSER_AGENT` filed itself as
            // `citrix-STX.org` — a title that no longer round-trips, and one
            // file two differently-titled pages both claim.
            if i == last {
                path.push(format!("{segment}.org"));
            } else {
                path.push(segment);
            }
        }
        Self::inside(root, path)
    }

    /// Accept an already-built path only if it is a strict descendant of
    /// `root`.
    ///
    /// The value carries the NORMALIZED path — the one containment was proven
    /// for. Handing back the caller's spelling would let a proven-contained
    /// verdict travel with a path that still reads `..`, which is how a check
    /// and the write it guards come to disagree.
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
        Ok(Self(normalized))
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

    /// `Path::with_extension` REPLACES the segment after the last dot, so a
    /// dotted title lands in a TRUNCATED file and re-ingests under a different
    /// title. Derivation must APPEND `.org` to the leaf, whatever it contains.
    #[test]
    fn a_dotted_leaf_title_keeps_its_dots() {
        let root = Path::new("/vault");
        for (leaf, expected) in [
            ("citrix-STX.BROWSER_AGENT", "citrix-STX.BROWSER_AGENT.org"),
            ("a.b", "a.b.org"),
            ("Trailing.", "Trailing..org"),
            ("x.y.z", "x.y.z.org"),
        ] {
            let derived = VaultPath::page_file_from_name_chain(root, &chain(&["Agents", leaf]))
                .expect("a dotted title is a well-formed chain");
            assert_eq!(
                derived.as_path(),
                Path::new("/vault/Agents").join(expected),
                "title '{leaf}' must derive its own file"
            );
        }
    }

    /// A dotted title and the dotless title it truncates to must NOT share a
    /// file — the truncation silently overwrote one page with the other.
    #[test]
    fn a_dotted_title_does_not_collide_with_its_truncation() {
        let root = Path::new("/vault");
        let dotted = VaultPath::page_file_from_name_chain(root, &chain(&["citrix-STX.BROWSER"]))
            .expect("dotted title derives");
        let truncated = VaultPath::page_file_from_name_chain(root, &chain(&["citrix-STX"]))
            .expect("dotless title derives");
        assert_ne!(
            dotted, truncated,
            "two distinct page titles must not derive the same file"
        );
    }

    /// A dot in a NON-leaf segment names a directory; only the leaf gains
    /// `.org`.
    #[test]
    fn a_dotted_interior_segment_stays_a_directory() {
        let derived = VaultPath::page_file_from_name_chain(
            Path::new("/vault"),
            &chain(&["v1.2", "Optimize RAG"]),
        )
        .expect("a dotted directory segment derives");
        assert_eq!(derived.as_path(), Path::new("/vault/v1.2/Optimize RAG.org"));
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

    /// A contained path that merely SPELLS a climb normalizes on the way in, so
    /// the proven value and the write target are the same path.
    #[test]
    fn a_contained_path_is_carried_normalized() {
        let derived = VaultPath::inside(
            Path::new("/vault"),
            PathBuf::from("/vault/attachments/../img.png"),
        )
        .expect("a path that stays inside must be accepted");
        assert_eq!(derived.as_path(), Path::new("/vault/img.png"));
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
