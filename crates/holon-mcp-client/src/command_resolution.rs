//! Turning a sidecar's `command` into a runnable binary, and saying precisely
//! why when it cannot be.
//!
//! `Command::new("claude-history-mcp")` resolves against the PARENT's `PATH`.
//! A `.app` started from Finder or the Dock inherits launchd's minimal `PATH`,
//! so a sidecar the user installed with cargo or Homebrew is invisible to the
//! packaged build while working from a dev shell. The failures also need
//! different remedies — not installed, installed where this process cannot
//! look, present but not executable — so they are told apart here rather than
//! collapsed into one `ENOENT`.
//!
//! UNIX ONLY. Elsewhere the OS's own resolution stays in charge: reproducing
//! Windows' `PATHEXT` search faithfully is a second implementation of a rule we
//! do not own, and getting it subtly wrong stops correctly-installed sidecars
//! from starting at all.

use std::fmt;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

use tokio::process::Command;

/// What a sidecar's `command` string resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandLookup {
    /// An executable file, ready to spawn.
    Found(PathBuf),
    /// A bare name that is on none of the searched directories — the tool is
    /// not installed, or not installed anywhere a GUI launch can see.
    NotOnPath {
        command: String,
        searched: Vec<PathBuf>,
    },
    /// A path that names no file at all.
    MissingFile { path: PathBuf },
    /// The file is right there and cannot be run — a bundled sidecar that lost
    /// its exec bit. Saying "not found" here would name the wrong cause.
    NotExecutable { path: PathBuf },
}

impl CommandLookup {
    /// The stable word for this outcome, stored alongside an integration's
    /// status so a later failure can name the boot cause without re-deriving
    /// it.
    pub fn kind(&self) -> &'static str {
        match self {
            CommandLookup::Found(_) => "found",
            CommandLookup::NotOnPath { .. } => "binary-not-on-path",
            CommandLookup::MissingFile { .. } => "binary-missing",
            CommandLookup::NotExecutable { .. } => "binary-not-executable",
        }
    }
}

impl fmt::Display for CommandLookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandLookup::Found(path) => write!(f, "found at {}", path.display()),
            CommandLookup::NotOnPath { command, searched } => write!(
                f,
                "binary '{command}' not found on PATH (searched {})",
                searched
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            CommandLookup::MissingFile { path } => {
                write!(f, "binary not found at {}", path.display())
            }
            CommandLookup::NotExecutable { path } => {
                write!(f, "found at {} but not executable", path.display())
            }
        }
    }
}

/// A `command` that could not be turned into something spawnable.
///
/// Carries the [`CommandLookup`] so a caller can record WHICH failure it was
/// rather than re-parsing an OS error string.
#[derive(Debug, Clone)]
pub struct SidecarCommandUnavailable(pub CommandLookup);

impl fmt::Display for SidecarCommandUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SidecarCommandUnavailable {}

#[cfg(unix)]
mod unix {
    use super::*;

    /// Directories searched after the inherited `PATH` for a bare command name.
    ///
    /// An explicit list rather than asking a login shell: it costs no
    /// subprocess per integration at boot and cannot be broken by the user's
    /// shell rc. Appended AFTER the inherited `PATH`, never in place of it, so
    /// a binary on an exotic shell `PATH` still resolves from where it always
    /// did.
    fn standard_install_dirs() -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs.push(home.join(".cargo/bin"));
            dirs.push(home.join(".local/bin"));
        }
        dirs.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/opt/local/bin",
                "/usr/bin",
                "/bin",
            ]
            .iter()
            .map(PathBuf::from),
        );
        dirs
    }

    fn path_dirs() -> Vec<PathBuf> {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default()
    }

    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    /// Resolve `command` to an executable, searching the inherited `PATH` and
    /// then [`standard_install_dirs`] when it carries no path separator.
    pub fn look_up_command(command: &str) -> CommandLookup {
        let as_path = Path::new(command);
        if as_path.components().count() > 1 {
            return classify_path(as_path);
        }

        let mut searched = path_dirs();
        for dir in standard_install_dirs() {
            if !searched.contains(&dir) {
                searched.push(dir);
            }
        }
        let mut present_but_unrunnable = None;
        for dir in &searched {
            let candidate = dir.join(command);
            if is_executable(&candidate) {
                return CommandLookup::Found(candidate);
            }
            if present_but_unrunnable.is_none() && candidate.is_file() {
                present_but_unrunnable = Some(candidate);
            }
        }
        // A name that matched a file nobody can run is a permission problem,
        // not an absence — "not found" would send the user hunting for an
        // install they already have.
        match present_but_unrunnable {
            Some(path) => CommandLookup::NotExecutable { path },
            None => CommandLookup::NotOnPath {
                command: command.to_string(),
                searched,
            },
        }
    }

    fn classify_path(path: &Path) -> CommandLookup {
        if is_executable(path) {
            CommandLookup::Found(path.to_path_buf())
        } else if path.is_file() {
            CommandLookup::NotExecutable {
                path: path.to_path_buf(),
            }
        } else {
            CommandLookup::MissingFile {
                path: path.to_path_buf(),
            }
        }
    }
}

#[cfg(unix)]
pub use unix::look_up_command;

/// The spawnable command for a sidecar's `command` string.
///
/// On unix this resolves first, so the failure carries a cause. Elsewhere the
/// OS resolves, and a spawn failure says only what the OS knows.
#[cfg(unix)]
pub fn build_command(command: &str, args: &[String]) -> anyhow::Result<Command> {
    match look_up_command(command) {
        CommandLookup::Found(path) => {
            let mut cmd = Command::new(path);
            cmd.args(args);
            Ok(cmd)
        }
        unavailable => Err(anyhow::Error::new(SidecarCommandUnavailable(unavailable))),
    }
}

#[cfg(not(unix))]
pub fn build_command(command: &str, args: &[String]) -> anyhow::Result<Command> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    Ok(cmd)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::MutexGuard;
    use std::sync::OnceLock;

    use super::*;

    /// `set_var` is process-global, so these tests must not overlap even under
    /// a threaded `cargo test` (nextest's process-per-test makes it moot, but
    /// the guard must not depend on which runner is used).
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Restores `HOME` and `PATH` on drop, so a panicking assertion cannot
    /// leave the rest of the binary running against a clobbered environment.
    struct EnvGuard {
        home: Option<std::ffi::OsString>,
        path: Option<std::ffi::OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(home: &Path, path: &str) -> Self {
            let guard = Self {
                home: std::env::var_os("HOME"),
                path: std::env::var_os("PATH"),
                _lock: env_lock(),
            };
            // SAFETY: the process-wide lock above serializes every mutation
            // here, and Drop restores both variables on any exit path.
            unsafe {
                std::env::set_var("HOME", home);
                std::env::set_var("PATH", path);
            }
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.home.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match self.path.take() {
                    Some(v) => std::env::set_var("PATH", v),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    fn write_file(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").expect("write stub");
        let mut perms = std::fs::metadata(&path).expect("stat stub").permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(&path, perms).expect("chmod stub");
        path
    }

    #[test]
    fn an_explicit_path_that_does_not_exist_is_a_missing_binary_not_a_path_problem() {
        let lookup = look_up_command("/nonexistent/holon-test-sidecar");
        assert_eq!(lookup.kind(), "binary-missing");
        assert_eq!(
            lookup,
            CommandLookup::MissingFile {
                path: PathBuf::from("/nonexistent/holon-test-sidecar")
            }
        );
        assert!(
            lookup
                .to_string()
                .contains("/nonexistent/holon-test-sidecar"),
            "the disclosure must carry the path that was configured: {lookup}"
        );
    }

    #[test]
    fn a_bare_name_nobody_installed_reports_where_it_looked() {
        let lookup = look_up_command("holon-definitely-not-installed-sidecar");
        assert_eq!(lookup.kind(), "binary-not-on-path");
        let CommandLookup::NotOnPath { command, searched } = &lookup else {
            panic!("expected NotOnPath, got {lookup:?}");
        };
        assert_eq!(command, "holon-definitely-not-installed-sidecar");
        assert!(
            !searched.is_empty(),
            "the disclosure must name the directories searched"
        );
        assert!(
            lookup.to_string().contains("not found on PATH"),
            "message must distinguish PATH from a missing file: {lookup}"
        );
    }

    /// A bundled sidecar shipped without its exec bit is present, not absent.
    /// "not found" would send the user hunting for an install they have.
    #[test]
    fn a_file_that_exists_without_the_exec_bit_names_the_permission_not_an_absence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = write_file(dir.path(), "holon-unrunnable-probe", 0o644);

        let lookup = look_up_command(stub.to_str().expect("utf8 path"));

        assert_eq!(lookup.kind(), "binary-not-executable");
        assert_eq!(lookup, CommandLookup::NotExecutable { path: stub.clone() });
        assert_eq!(
            lookup.to_string(),
            format!("found at {} but not executable", stub.display())
        );
    }

    /// The packaging fix: a Finder-launched `.app` inherits launchd's minimal
    /// `PATH`, so resolution must reach the standard install directories too.
    #[test]
    fn a_bare_name_off_the_inherited_path_still_resolves_from_a_standard_install_dir() {
        let home = tempfile::tempdir().expect("tempdir");
        let cargo_bin = home.path().join(".cargo/bin");
        std::fs::create_dir_all(&cargo_bin).expect("mkdir .cargo/bin");
        let stub = write_file(&cargo_bin, "holon-install-dir-probe", 0o755);

        let _guard = EnvGuard::set(home.path(), "/nonexistent-launchd-path");
        let lookup = look_up_command("holon-install-dir-probe");

        assert_eq!(
            lookup,
            CommandLookup::Found(stub),
            "a tool in ~/.cargo/bin must resolve even when PATH is launchd's minimal one"
        );
    }

    /// The inherited `PATH` must stay authoritative: a binary that lives ONLY
    /// on an exotic shell `PATH` still resolves from there.
    #[test]
    fn the_inherited_path_is_searched_before_the_standard_install_dirs() {
        let home = tempfile::tempdir().expect("tempdir");
        let exotic = tempfile::tempdir().expect("tempdir");
        let stub = write_file(exotic.path(), "holon-exotic-probe", 0o755);

        let _guard = EnvGuard::set(home.path(), exotic.path().to_str().expect("utf8 path"));
        let lookup = look_up_command("holon-exotic-probe");

        assert_eq!(lookup, CommandLookup::Found(stub));
    }
}
