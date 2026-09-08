//! The ONE answer to "does the vault contain this path?".
//!
//! Native-only: the rule is defined as whatever `ignore`'s walk does, and
//! `ignore` does not build for wasm32. The wasm surface of this crate is the
//! port traits, so nothing there walks a vault to begin with.

use std::fmt;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use ignore::gitignore::Gitignore;

use crate::vault_path::hidden_vault_segment;

/// Why a path is not part of the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultRefusal {
    /// A dot-prefixed segment below the root (`ignore`'s `hidden(true)`).
    Hidden(String),
    /// A directory on the way down is a symlink. The walk does not follow
    /// links, so nothing beneath one is ever part of the vault.
    BeyondSymlink(PathBuf),
    /// A gitignore rule — the root's, a nested one, a parent's, or the user's
    /// global excludes — claims the path.
    Gitignored(PathBuf),
}

impl fmt::Display for VaultRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hidden(segment) => write!(f, "hidden segment '{segment}'"),
            Self::BeyondSymlink(link) => write!(f, "beyond the symlink '{}'", link.display()),
            Self::Gitignored(rules) => write!(f, "gitignored by '{}'", rules.display()),
        }
    }
}

/// The ONE answer to "does the vault contain this path?", shared by every leg
/// that asks.
///
/// Three legs decide vault membership — the boot walk
/// ([`crate::fs_port::walk_directory`]), the live file watcher, and the
/// harness's in-memory scan — and a path only some of them accept is a path
/// whose document exists in some boots and not others. The walk delegates to
/// `ignore::WalkBuilder`; the other two have no walk to delegate to, so they
/// ask this filter, which restates the walk's rules once against the SAME
/// `ignore` machinery rather than re-deriving them per call site.
///
/// Built once per root: the gitignore set is read from disk at construction,
/// exactly as the walk reads it at the start of each walk.
pub struct VaultFilter {
    root: PathBuf,
    /// Gitignore matchers, each paired with the file it came from, ordered
    /// outermost-first so a nested rule is consulted last and wins.
    gitignores: Vec<(PathBuf, Gitignore)>,
}

impl VaultFilter {
    pub fn for_root(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            gitignores: collect_gitignores(root),
        }
    }

    pub fn admits(&self, path: &Path) -> bool {
        self.refusal(path).is_none()
    }

    /// The reason `path` is out of the vault's reach, or `None` when it is
    /// ingestable.
    pub fn refusal(&self, path: &Path) -> Option<VaultRefusal> {
        if let Some(segment) = hidden_vault_segment(&self.root, path) {
            return Some(VaultRefusal::Hidden(segment));
        }
        if let Some(link) = self.unfollowed_symlink(path) {
            return Some(VaultRefusal::BeyondSymlink(link));
        }
        let is_dir = path.is_dir();
        self.gitignores
            .iter()
            .rev()
            .find(|(_, matcher)| {
                path.starts_with(matcher.path())
                    && matcher
                        .matched_path_or_any_parents(path, is_dir)
                        .is_ignore()
            })
            .map(|(source, _)| VaultRefusal::Gitignored(source.clone()))
    }

    /// The first symlinked DIRECTORY between the root and `path`.
    ///
    /// `path` itself is exempt: the walk yields a symlink to a plain file as a
    /// file, and only refuses to DESCEND through one.
    fn unfollowed_symlink(&self, path: &Path) -> Option<PathBuf> {
        // A path outside the root crosses none of the root's own links; it is
        // already refused as un-contained by whoever holds the containment
        // proof.
        let Ok(below) = path.strip_prefix(&self.root) else {
            return None;
        };
        let mut segments: Vec<Component<'_>> = below.components().collect();
        segments.pop();
        let mut walked = self.root.clone();
        for segment in segments {
            walked.push(segment);
            if std::fs::symlink_metadata(&walked).is_ok_and(|m| m.file_type().is_symlink()) {
                return Some(walked);
            }
        }
        None
    }
}

/// Every `.gitignore` the walk would honour under `root`, outermost first.
///
/// Empty outside a git repository: `ignore`'s `require_git` default makes the
/// walk read no gitignore there, so honouring them here would drop files the
/// boot walk ingests.
fn collect_gitignores(root: &Path) -> Vec<(PathBuf, Gitignore)> {
    if !root.exists() || !in_git_repo(root) {
        return Vec::new();
    }
    let mut sources: Vec<PathBuf> = Vec::new();

    let (global, _) = Gitignore::global();
    let mut found: Vec<(PathBuf, Gitignore)> = if global.is_empty() {
        Vec::new()
    } else {
        vec![(PathBuf::from("<global excludes>"), global)]
    };

    // Parent `.gitignore`s up to the repository root — `ignore`'s
    // `parents(true)` default.
    let mut ancestors: Vec<&Path> = root.ancestors().skip(1).collect();
    ancestors.reverse();
    sources.extend(ancestors.into_iter().map(|dir| dir.join(".gitignore")));

    // Nested ones under the root. Hidden DIRECTORIES are pruned (nothing
    // inside one is ingestable anyway) while hidden FILES pass, because
    // `.gitignore` is itself one.
    sources.extend(
        ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .filter_entry(|entry| {
                !entry.file_type().is_some_and(|t| t.is_dir())
                    || !entry.file_name().to_string_lossy().starts_with('.')
            })
            .build()
            .flatten()
            .map(ignore::DirEntry::into_path)
            .filter(|path| path.file_name().is_some_and(|name| name == ".gitignore")),
    );

    for source in sources {
        let (matcher, _) = Gitignore::new(&source);
        if !matcher.is_empty() {
            found.push((source, matcher));
        }
    }
    found
}

fn in_git_repo(root: &Path) -> bool {
    root.ancestors().any(|dir| dir.join(".git").exists())
}
