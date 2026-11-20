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

/// What two paths share when APFS (the macOS default) names one entry by
/// both: every component under [`holon_api::caseless_fold`]. Two pages whose
/// files share a key write one file on a Mac, whatever the file system that
/// derived them, so the second one's write-back is refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathCollisionKey(Vec<String>);

impl PathCollisionKey {
    pub fn of(path: &Path) -> Self {
        Self(
            path.components()
                .map(|c| holon_api::caseless_fold(&c.as_os_str().to_string_lossy()))
                .collect(),
        )
    }
}

/// Why the files `a` and `b` name one page, as a sentence: a page's name
/// chain drops the extension and compares titles by
/// [`holon_api::PageTitleKey`], and this names which of those differences the
/// two paths actually have.
pub fn why_two_files_name_one_page(a: &Path, b: &Path) -> String {
    let extension = a.extension() != b.extension();
    let stem = |p: &Path| p.with_extension("").to_string_lossy().into_owned();
    let Some(spelling) = holon_api::SpellingDifference::between(&stem(a), &stem(b)) else {
        return format!(
            "{} and {} name one page by their name chains",
            a.display(),
            b.display()
        );
    };
    let differ = match (extension, spelling.to_string().as_str()) {
        (true, "nothing") => "only in their extension".to_string(),
        (true, d) => format!("only in their extension and in {d}"),
        (false, d) => format!("only in {d}"),
    };
    let mut why = format!(
        "{} and {} differ {differ}; a page name drops the extension and compares titles up to \
         case, Unicode normalization and spacing",
        a.display(),
        b.display()
    );
    if spelling.spacing {
        why.push_str(
            ". Page identity trims a title and collapses its runs of whitespace, as a page id \
             does, so the two files name ONE page although the file system keeps them apart",
        );
    }
    why
}

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
            // APPEND, never `with_extension`: that REPLACES whatever follows
            // the leaf's last dot, so `citrix-STX.BROWSER_AGENT` filed itself
            // as `citrix-STX.org` — a title that no longer round-trips, and one
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

    /// Accept an existing `path` only if the entry it names, every symlink
    /// resolved, is a strict descendant of the entry `root` names.
    ///
    /// The value carries the resolved path, so a read through it opens the
    /// file containment was proven for and not whatever a link in the
    /// caller's spelling points at.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn inside_canonical(
        fs: &dyn crate::fs_port::FileSystem,
        root: &Path,
        path: &Path,
    ) -> std::result::Result<Self, CanonicalRefusal> {
        let refusal = |why| CanonicalRefusal {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
            why,
        };
        let canonical_root = fs
            .canonicalize(root)
            .map_err(|e| refusal(RefusalReason::RootUnresolvable(e)))?;
        // `realpath` resolves `..` and a trailing `/` past a link to a file,
        // spellings the kernel refuses to open.
        if !fs.exists(path) {
            let normalized = lexically_normalize(path);
            let under = |r: &Path| normalized != r && normalized.starts_with(r);
            return Err(refusal(
                if under(&lexically_normalize(root)) || under(&canonical_root) {
                    RefusalReason::NamesNoEntry
                } else {
                    RefusalReason::Outside
                },
            ));
        }
        let canonical = fs
            .canonicalize(path)
            .map_err(|e| refusal(RefusalReason::Unresolvable(e)))?;
        if canonical == canonical_root || !canonical.starts_with(&canonical_root) {
            return Err(refusal(RefusalReason::Outside));
        }
        Ok(Self(canonical))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Why [`VaultPath::inside_canonical`] refused a path. The text names only
/// the path and the root as the caller spelled them: where a path resolves
/// would tell the caller which host paths exist and where a link points.
#[derive(Debug)]
pub struct CanonicalRefusal {
    path: PathBuf,
    root: PathBuf,
    why: RefusalReason,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
enum RefusalReason {
    /// Missing, and inside the root as spelled.
    NamesNoEntry,
    /// Resolved outside the root, or missing and outside it as spelled, so
    /// a missing and an existing outside path read alike.
    Outside,
    RootUnresolvable(std::io::Error),
    Unresolvable(std::io::Error),
}

#[cfg(not(target_arch = "wasm32"))]
impl CanonicalRefusal {
    pub fn names_no_entry(&self) -> bool {
        matches!(self.why, RefusalReason::NamesNoEntry)
    }

    /// The refusal with `subject` standing for the path.
    pub fn naming(&self, subject: &str) -> String {
        let root = self.root.display();
        match &self.why {
            RefusalReason::NamesNoEntry => format!("{subject} names no entry"),
            RefusalReason::Outside => {
                format!("{subject} lies OUTSIDE the vault root '{root}'")
            }
            RefusalReason::RootUnresolvable(e) => {
                format!("resolving the vault root '{root}': {e}")
            }
            RefusalReason::Unresolvable(e) => format!("resolving {subject}: {e}"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for CanonicalRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.naming(&format!("'{}'", self.path.display())))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::error::Error for CanonicalRefusal {}

/// The dot-prefixed segment BELOW `root` that puts `path` out of the vault's
/// reach, or `None` when the path is ingestable.
///
/// Two legs decide what the vault contains — the boot walk and the live file
/// watcher — and a path only one of them accepts is a path whose document
/// exists in some boots and not others. The walk delegates the question to
/// `ignore`'s `hidden(true)`; the watcher, which has no such builder, asks
/// here, so this function is the walk's rule restated once rather than retyped
/// per call site.
///
/// Only segments below `root` are judged: a vault that itself lives under
/// `~/.pkm` is still a vault.
pub fn hidden_vault_segment(root: &Path, path: &Path) -> Option<String> {
    // A path we cannot relate to `root` is judged whole. It is outside the
    // vault and uningestable either way, and judging it whole keeps the
    // pre-root VCS-internal segments visible to the caller.
    let below = path.strip_prefix(root).unwrap_or(path);
    below.components().find_map(|component| {
        let Component::Normal(segment) = component else {
            return None;
        };
        let text = segment.to_string_lossy();
        text.starts_with('.').then(|| text.into_owned())
    })
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

    #[test]
    fn an_ordinary_page_has_no_hidden_segment() {
        assert_eq!(
            hidden_vault_segment(Path::new("/vault"), Path::new("/vault/Projects/Now.org")),
            None
        );
    }

    /// The shape that reached ingest: a full vault copy inside an agent's
    /// jj-workspace.
    #[test]
    fn a_nested_worktree_copy_names_the_segment_that_excludes_it() {
        assert_eq!(
            hidden_vault_segment(
                Path::new("/vault"),
                Path::new("/vault/.claude/worktrees/agent-x/Projects/Now.org")
            ),
            Some(".claude".to_string())
        );
    }

    /// Every dot-directory counts, not a hand-listed few — `.obsidian` and
    /// `.logseq` carry a companion editor's own copies of the same notes.
    #[test]
    fn any_dot_directory_excludes_not_just_vcs_internals() {
        for hidden in [".obsidian", ".logseq", ".git", ".jj"] {
            assert_eq!(
                hidden_vault_segment(
                    Path::new("/vault"),
                    &Path::new("/vault").join(hidden).join("A.org")
                ),
                Some(hidden.to_string())
            );
        }
    }

    /// Judging the whole path would make a vault under `~/.pkm` ingest
    /// nothing at all — only segments BELOW the root are the vault's own.
    #[test]
    fn a_root_that_is_itself_hidden_still_holds_ingestable_pages() {
        assert_eq!(
            hidden_vault_segment(
                Path::new("/Users/m/.pkm/vault"),
                Path::new("/Users/m/.pkm/vault/Projects/Now.org")
            ),
            None
        );
    }

    /// A dotfile is as invisible to the boot walk as a dot-directory.
    #[test]
    fn a_hidden_leaf_file_is_excluded_too() {
        assert_eq!(
            hidden_vault_segment(Path::new("/vault"), Path::new("/vault/.draft.org")),
            Some(".draft.org".to_string())
        );
    }

    /// A vault, a sibling holding the secret, a sibling whose name extends
    /// the vault's, links that leave the vault, stay in it, or name its root,
    /// and a link beside the vault that leads into it.
    #[cfg(unix)]
    fn linked_vault() -> tempfile::TempDir {
        let base = tempfile::tempdir().expect("temp dir");
        let at = |rel: &str| base.path().join(rel);
        for dir in ["vault/notes", "outside", "vault-evil"] {
            std::fs::create_dir_all(at(dir)).expect("create fixture dir");
        }
        for file in [
            "vault/notes/a.org",
            "outside/secret.org",
            "vault-evil/secret.org",
        ] {
            std::fs::write(at(file), file).expect("write fixture file");
        }
        for (link, target) in [
            ("vault/file_link", "../outside/secret.org"),
            ("vault/dir_link", "../outside"),
            ("vault/inside_link", "./notes/a.org"),
            ("vault/notes/root_link", ".."),
            ("vault_link", "vault"),
        ] {
            std::os::unix::fs::symlink(target, at(link)).expect("create fixture link");
        }
        base
    }

    /// The entries strictly inside the vault, by inode, so the oracle judges
    /// what a path reaches without resolving or comparing any path.
    #[cfg(unix)]
    fn inodes_inside(vault: &Path) -> std::collections::BTreeSet<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let mut inside = std::collections::BTreeSet::new();
        let mut todo = vec![vault.to_path_buf()];
        while let Some(dir) = todo.pop() {
            for entry in std::fs::read_dir(&dir).expect("read fixture dir") {
                let entry = entry.expect("fixture dir entry");
                let meta = std::fs::symlink_metadata(entry.path()).expect("fixture entry");
                if meta.file_type().is_symlink() {
                    continue;
                }
                inside.insert((meta.dev(), meta.ino()));
                if meta.is_dir() {
                    todo.push(entry.path());
                }
            }
        }
        inside
    }

    const SEGMENTS: &[&str] = &[
        "notes",
        "a.org",
        "notes/a.org",
        "..",
        ".",
        "",
        "dir_link",
        "file_link",
        "inside_link",
        "root_link",
        "secret.org",
        "outside",
        "vault",
        "vault-evil",
        "vault_link",
        "missing.org",
    ];

    fn segment_path() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        prop::collection::vec(prop::sample::select(SEGMENTS), 0..5).prop_map(|s| s.join("/"))
    }

    /// Runs `check` on every example and fails naming each one that failed,
    /// so one red run shows them all.
    fn each_example<T: std::fmt::Debug>(examples: &[T], check: impl Fn(&T)) {
        let failed: Vec<&T> = examples
            .iter()
            .filter(|e| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(e))).is_err()
            })
            .collect();
        assert!(failed.is_empty(), "failed examples: {failed:?}");
    }

    /// A refusal names only what the caller gave: with every `given` spelling
    /// removed, longest first so a spelling inside another goes with it, no
    /// path remains.
    fn assert_names_only_given(text: &str, given: &[String]) {
        let mut given: Vec<&String> = given.iter().filter(|g| !g.is_empty()).collect();
        given.sort_by_key(|g| std::cmp::Reverse(g.len()));
        let rest = given
            .into_iter()
            .fold(text.to_string(), |rest, g| rest.replace(g.as_str(), ""));
        assert!(
            !rest.contains('/'),
            "the refusal names a path the caller did not give: {text}"
        );
    }

    /// `.` and `..` resolved by spelling alone, the oracle's own copy.
    fn spelled(path: &Path) -> PathBuf {
        path.components().fold(PathBuf::new(), |mut out, c| {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other),
            }
            out
        })
    }

    /// A path outside the vault as spelled, refused, reads like a missing
    /// path appended to it: the text tells nothing about what exists.
    /// Returns whether the case was compared.
    fn outside_reads_like_missing(
        fs: &dyn crate::fs_port::FileSystem,
        roots: &[&Path],
        path: &Path,
    ) -> bool {
        let root = roots[0];
        let spelled_path = spelled(path);
        if roots.iter().any(|r| spelled_path.starts_with(spelled(r))) {
            return false;
        }
        let Err(refused) = VaultPath::inside_canonical(fs, root, path) else {
            return false;
        };
        let missing = path.join("never-made");
        assert!(!fs.exists(&missing), "{}", missing.display());
        let missing_refused = VaultPath::inside_canonical(fs, root, &missing)
            .expect_err("a missing path outside the vault is refused");
        let alike =
            |p: &Path, text: String| text.replace(&format!("'{}'", p.display()), "'<argument>'");
        assert_eq!(
            alike(path, refused.to_string()),
            alike(&missing, missing_refused.to_string()),
            "'{}' and the missing '{}' read apart",
            path.display(),
            missing.display()
        );
        true
    }

    /// Over the real file system: a path is accepted exactly when the entry
    /// it reaches is one of the vault's own, and the accepted value reaches
    /// that same entry. The root is given as the temp dir spells it, which
    /// on macOS runs through the `/var` link.
    #[cfg(unix)]
    #[test]
    fn inside_canonical_accepts_exactly_what_reaches_a_vault_entry() {
        use std::os::unix::fs::MetadataExt;

        use proptest::prelude::*;

        let base = linked_vault();
        let root = base.path().join("vault");
        let canonical_base = std::fs::canonicalize(base.path()).expect("canonical base");
        let inside = inodes_inside(&root);
        let fs = crate::fs_port::RealFileSystem;
        let canonical_root = canonical_base.join("vault");
        let (refused, compared) = (std::cell::Cell::new(0), std::cell::Cell::new(0));

        let check = |path: &Path| {
            let reached = match std::fs::metadata(path) {
                Ok(m) => Some((m.dev(), m.ino())),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    None
                }
                Err(e) => panic!("stat '{}': {e}", path.display()),
            };
            let verdict = VaultPath::inside_canonical(&fs, &root, path);
            match reached.filter(|r| inside.contains(r)) {
                Some(entry) => {
                    let accepted = verdict.unwrap_or_else(|e| {
                        panic!("'{}' reaches a vault entry: {e:#}", path.display())
                    });
                    let meta = std::fs::metadata(accepted.as_path()).expect("accepted path exists");
                    assert_eq!((meta.dev(), meta.ino()), entry, "{}", path.display());
                }
                None => {
                    let e = verdict.expect_err(&format!(
                        "'{}' reaches no vault entry but was accepted",
                        path.display()
                    ));
                    refused.set(refused.get() + 1);
                    if outside_reads_like_missing(&fs, &[&root, &canonical_root], path) {
                        compared.set(compared.get() + 1);
                    }
                    assert_names_only_given(
                        &e.to_string(),
                        &[path.display().to_string(), root.display().to_string()],
                    );
                }
            }
        };

        let examples = [
            "../outside/secret.org",
            "dir_link/secret.org",
            "file_link",
            "notes/root_link",
            "",
            ".",
            "inside_link",
            "inside_link/",
            "inside_link/..",
            "notes/a.org",
            "../vault-evil/secret.org",
            "../outside/none.org",
            "notes/none.org",
            "../vault_link/notes/a.org",
        ]
        .map(|rel| root.join(rel));
        each_example(
            &[
                &examples[..],
                &[PathBuf::from(format!("{}-evil/secret.org", root.display()))],
            ]
            .concat(),
            |path| check(path),
        );
        assert_eq!(
            VaultPath::inside_canonical(&fs, &root, &root.join("notes/none.org"))
                .expect_err("a missing path is refused")
                .to_string(),
            format!("'{}' names no entry", root.join("notes/none.org").display()),
            "a missing path inside the vault keeps its own refusal"
        );

        let prefixes = prop_oneof![
            Just(root.clone()),
            Just(canonical_base.join("vault")),
            Just(base.path().to_path_buf()),
            Just(PathBuf::from(format!("{}-evil", root.display()))),
        ];
        proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(512))
            .run(&(prefixes, segment_path()), |(prefix, rel)| {
                check(&prefix.join(rel));
                Ok(())
            })
            .expect("inside_canonical property");
        eprintln!(
            "refused {} cases, compared {} outside cases with a missing sibling",
            refused.get(),
            compared.get()
        );
        assert!(refused.get() > 0 && compared.get() > 0);
    }

    /// The in-memory adapter holds no links, so only spelling can climb out:
    /// `..`, `.`, a root-equal path and a name-prefix sibling.
    #[test]
    fn inside_canonical_over_the_in_memory_adapter_refuses_every_lexical_escape() {
        use proptest::prelude::*;

        use crate::fs_port::FileSystem;

        let fs = crate::in_memory::InMemoryFileSystem::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        for dir in ["/vault/notes", "/outside", "/vault-evil"] {
            fs.mkdir_all(Path::new(dir));
        }
        for file in [
            "/vault/notes/a.org",
            "/outside/secret.org",
            "/vault-evil/secret.org",
        ] {
            runtime
                .block_on(fs.write(Path::new(file), b"x"))
                .expect("write fixture file");
        }
        let inside: [&Path; 2] = [Path::new("/vault/notes"), Path::new("/vault/notes/a.org")];
        let root = Path::new("/vault");
        let (refused, compared) = (std::cell::Cell::new(0), std::cell::Cell::new(0));

        let check = |path: &Path| {
            let mut reached = PathBuf::from("/");
            for component in path.components() {
                match component {
                    Component::ParentDir => {
                        reached.pop();
                    }
                    Component::Normal(name) => reached.push(name),
                    Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
                }
            }
            let verdict = VaultPath::inside_canonical(&fs, root, path);
            if inside.contains(&reached.as_path()) {
                assert_eq!(
                    verdict.expect("a vault entry is accepted").as_path(),
                    reached.as_path()
                );
            } else {
                let e = verdict.expect_err(&format!("'{}' was accepted", path.display()));
                refused.set(refused.get() + 1);
                if outside_reads_like_missing(&fs, &[root], path) {
                    compared.set(compared.get() + 1);
                }
                assert_names_only_given(
                    &e.to_string(),
                    &[path.display().to_string(), root.display().to_string()],
                );
            }
        };

        let examples = [
            "/vault",
            "/vault/.",
            "/vault/notes/..",
            "/vault/../outside/secret.org",
            "/vault/../outside/none.org",
        ];
        each_example(&examples, |path| check(Path::new(path)));
        let prefixes = prop_oneof![Just("/vault"), Just("/"), Just("/vault-evil")];
        proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(256))
            .run(&(prefixes, segment_path()), |(prefix, rel)| {
                check(&Path::new(prefix).join(rel));
                Ok(())
            })
            .expect("inside_canonical in-memory property");
        eprintln!(
            "refused {} cases, compared {} outside cases with a missing sibling",
            refused.get(),
            compared.get()
        );
        assert!(refused.get() > 0 && compared.get() > 0);
    }
}
