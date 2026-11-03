//! What the process table says about one process or one process group.
//!
//! [`start`] allocates nothing and makes only async-signal-safe calls, so a
//! child may call it between fork and exec.

use std::io;

use super::Pgid;

/// One process, by pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Process {
    /// No process has the pid.
    Gone,
    /// The process started at this time, in the clock of [`start`]; a zombie
    /// still has it.
    Started(u64),
    /// A process has the pid, and this user may not inspect it: only where
    /// the process table hides other users' processes.
    #[cfg(target_os = "linux")]
    Foreign,
}

/// Reads `kinfo_proc`, which any user may read for any process, a zombie
/// included; the libc crate lacks the struct, whose first field is the start
/// as a `timeval`.
#[cfg(target_os = "macos")]
pub(crate) fn start(pid: i32) -> io::Result<Process> {
    const KINFO_PROC: usize = 648;
    let mut info = [0u64; KINFO_PROC / 8];
    let mut len = KINFO_PROC;
    let mut name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    // SAFETY: `info` holds `len` bytes.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            info.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return Err(io::Error::last_os_error());
    }
    match len {
        0 => Ok(Process::Gone),
        KINFO_PROC => Ok(Process::Started(
            info[0] * 1_000_000 + u64::from(info[1] as u32),
        )),
        _ => Err(io::Error::from(io::ErrorKind::InvalidData)),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn start(pid: i32) -> io::Result<Process> {
    let mut path = [0u8; 32];
    let mut at = put(&mut path, 0, b"/proc/");
    at = put_decimal(&mut path, at, pid as u64);
    put(&mut path, at, b"/stat\0");
    // SAFETY: `path` is NUL-terminated.
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOENT | libc::ESRCH) => Ok(Process::Gone),
            Some(libc::EACCES | libc::EPERM) => Ok(Process::Foreign),
            _ => Err(error),
        };
    }
    let mut stat = [0u8; 4096];
    let mut len = 0;
    let read = loop {
        // SAFETY: the range is inside `stat`.
        let n = unsafe { libc::read(fd, stat[len..].as_mut_ptr().cast(), stat.len() - len) };
        match n {
            0 => break Ok(()),
            n if n < 0 => break Err(io::Error::last_os_error()),
            n => len += n as usize,
        }
        if len == stat.len() {
            break Ok(());
        }
    };
    // SAFETY: `fd` is open and owned here.
    unsafe { libc::close(fd) };
    match read {
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return Ok(Process::Gone),
        Err(error) => return Err(error),
        Ok(()) => {}
    }
    let started = stat_field(&stat[..len], 22)
        .and_then(parse_decimal)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    Ok(Process::Started(started))
}

/// The members of a group that are not zombies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Members {
    None,
    /// Every one of them is exiting: macOS refuses to signal them.
    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "only macOS reports exiting members")
    )]
    Exiting,
    Running,
}

/// Reads the group's `kinfo_proc` entries, which name every user's
/// processes.
#[cfg(target_os = "macos")]
pub(crate) fn members(pgid: Pgid) -> io::Result<Members> {
    const KINFO_PROC: usize = 648;
    const P_FLAG: usize = 32;
    const P_WEXIT: i32 = 0x2000;
    const P_STAT: usize = 36;
    const SZOMB: u8 = 5;
    let mut name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PGRP, pgid];
    let mut sysctl = |buf: *mut libc::c_void, len: &mut usize| {
        // SAFETY: `buf` is null or holds `len` bytes.
        let read = unsafe {
            libc::sysctl(
                name.as_mut_ptr(),
                name.len() as libc::c_uint,
                buf,
                len,
                std::ptr::null_mut(),
                0,
            )
        };
        match read {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    };
    let entries = loop {
        let mut len = 0;
        sysctl(std::ptr::null_mut(), &mut len)?;
        // Room for members that join between the two calls.
        len += 8 * KINFO_PROC;
        let mut entries = vec![0u8; len];
        match sysctl(entries.as_mut_ptr().cast(), &mut len) {
            Ok(()) => {
                entries.truncate(len);
                break entries;
            }
            Err(error) if error.raw_os_error() == Some(libc::ENOMEM) => continue,
            Err(error) => return Err(error),
        }
    };
    if entries.len() % KINFO_PROC != 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut members = Members::None;
    for entry in entries.as_chunks::<KINFO_PROC>().0 {
        let flag = i32::from_ne_bytes([
            entry[P_FLAG],
            entry[P_FLAG + 1],
            entry[P_FLAG + 2],
            entry[P_FLAG + 3],
        ]);
        if entry[P_STAT] == SZOMB {
            continue;
        }
        if flag & P_WEXIT == 0 {
            return Ok(Members::Running);
        }
        members = Members::Exiting;
    }
    Ok(members)
}

/// Linux signals an exiting member, so it counts as running.
#[cfg(target_os = "linux")]
pub(crate) fn members(pgid: Pgid) -> io::Result<Members> {
    for entry in std::fs::read_dir("/proc")? {
        let name = entry?.file_name();
        // The other entries of /proc are no processes.
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let stat = match std::fs::read(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        let field =
            |n| stat_field(&stat, n).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData));
        let in_group = parse_decimal(field(5)?) == Some(pgid as u64);
        if in_group && !matches!(field(3)?, b"Z" | b"X" | b"x") {
            return Ok(Members::Running);
        }
    }
    Ok(Members::None)
}

/// Field `n` (1-based, as `proc(5)` counts) of a `/proc/<pid>/stat` line.
/// The command name in field 2 may hold spaces and parentheses; it ends at
/// the last `)`.
#[cfg(target_os = "linux")]
fn stat_field(stat: &[u8], n: usize) -> Option<&[u8]> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    stat[close + 1..]
        .split(|&b| b == b' ' || b == b'\n')
        .filter(|f| !f.is_empty())
        .nth(n - 3)
}

#[cfg(target_os = "linux")]
fn parse_decimal(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |n, &d| {
        d.is_ascii_digit()
            .then(|| n.checked_mul(10)?.checked_add(u64::from(d - b'0')))
            .flatten()
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn start(_: i32) -> io::Result<Process> {
    Err(unsupported())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn members(_: Pgid) -> io::Result<Members> {
    Err(unsupported())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the process table is read only on macOS and Linux",
    )
}

/// Copies `bytes` into `buf` at `at`; the index after them.
pub(crate) fn put(buf: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
    at + bytes.len()
}

/// Writes `n` in decimal into `buf` at `at`; the index after it.
pub(crate) fn put_decimal(buf: &mut [u8], at: usize, n: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut first = digits.len();
    let mut rest = n;
    loop {
        first -= 1;
        digits[first] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    put(buf, at, &digits[first..])
}
