//! wasi-compatible drop-in for the `symlink` crate. Mirrors the public API of
//! `symlink` 0.1.0 but defines the functions on every target. On unix/windows
//! it delegates to `std`; on targets without symlink support (wasm32-wasip1)
//! it returns an `Unsupported` error so dependents compile and fail loudly at
//! runtime rather than at compile time.

use std::io;
use std::path::Path;

#[cfg(not(any(unix, windows)))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "symlinks are not supported on this target",
    )
}

#[cfg(unix)]
pub fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}
#[cfg(windows)]
pub fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    std::os::windows::fs::symlink_file(src, dst)
}
#[cfg(not(any(unix, windows)))]
pub fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(_: P, _: Q) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(unix)]
pub fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}
#[cfg(windows)]
pub fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}
#[cfg(not(any(unix, windows)))]
pub fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(_: P, _: Q) -> io::Result<()> {
    Err(unsupported())
}

/// On unix a symlink has no file/dir distinction; on windows the caller must
/// pick. Upstream `symlink_auto` resolves the target's type — here we only need
/// the file variant for compilation parity (tracing-appender uses files).
#[cfg(unix)]
pub fn symlink_auto<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}
#[cfg(windows)]
pub fn symlink_auto<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    // Mirror upstream: default to a file symlink when the target type is unknown.
    std::os::windows::fs::symlink_file(src, dst)
}
#[cfg(not(any(unix, windows)))]
pub fn symlink_auto<P: AsRef<Path>, Q: AsRef<Path>>(_: P, _: Q) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(any(unix, windows))]
pub fn remove_symlink_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    std::fs::remove_file(path)
}
#[cfg(not(any(unix, windows)))]
pub fn remove_symlink_file<P: AsRef<Path>>(_: P) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(unix)]
pub fn remove_symlink_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    std::fs::remove_file(path)
}
#[cfg(windows)]
pub fn remove_symlink_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    std::fs::remove_dir(path)
}
#[cfg(not(any(unix, windows)))]
pub fn remove_symlink_dir<P: AsRef<Path>>(_: P) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(any(unix, windows))]
pub fn remove_symlink_auto<P: AsRef<Path>>(path: P) -> io::Result<()> {
    std::fs::remove_file(path)
}
#[cfg(not(any(unix, windows)))]
pub fn remove_symlink_auto<P: AsRef<Path>>(_: P) -> io::Result<()> {
    Err(unsupported())
}
