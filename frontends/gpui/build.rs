use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

fn main() {
    let sha = git_short_sha();
    println!("cargo:rustc-env=HOLON_BUILD_SHA={sha}");

    let time = match std::env::var("HOLON_BUILD_TIME") {
        Ok(pinned) if !pinned.is_empty() => pinned,
        _ => format_utc(SystemTime::now()),
    };
    println!("cargo:rustc-env=HOLON_BUILD_TIME={time}");

    // Resolve the real .git location robustly: in a jj/git worktree the crate's
    // `.git` may be a file or live at the repo root, so ask git for the common
    // dir instead of hardcoding a relative path. If resolution fails, skip the
    // rerun lines (provenance must never break the build).
    if let Some(git_dir) = git_common_dir() {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
    println!("cargo:rerun-if-env-changed=HOLON_BUILD_TIME");
}

fn git_short_sha() -> String {
    let rev = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output();
    let Ok(rev) = rev else {
        return "no-vcs-info".to_string();
    };
    if !rev.status.success() {
        return "no-vcs-info".to_string();
    }
    let sha = String::from_utf8_lossy(&rev.stdout).trim().to_string();
    if sha.is_empty() {
        return "no-vcs-info".to_string();
    }

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.status.success() && !o.stdout.trim_ascii().is_empty())
        .unwrap_or(false);

    if dirty { format!("{sha}-dirty") } else { sha }
}

fn git_common_dir() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir.is_empty() { None } else { Some(dir) }
}

fn format_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hour, min, sec) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02} UTC")
}

// Howard Hinnant's civil-from-days algorithm (days since 1970-01-01 -> Y/M/D).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}
