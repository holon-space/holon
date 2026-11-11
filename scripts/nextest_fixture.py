"""A one-crate workspace whose tests t00..t07 act out a plan, for property checks over a real nextest run.

The env var PLAN gives each test's outcome: `P` passes, `K` panics with the wording of a known-red row of
docs/Testing/KeystoneKnownReds.md, `U` panics with a wording no row lists, `X` kills nextest, `S` interrupts it.
"""
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
N = 8
TESTS = [f"t{i:02}" for i in range(N)]
UNLISTED = "planted red outside the known-reds list"
STATUS = re.compile(r"^ *([A-Z][A-Z0-9 +/-]*?) +\[[^]]*\] (?:\([^)]*\) )?\S+ (t\d\d)$")


def known_red_wording():
    rows = subprocess.run([sys.executable, str(REPO / "scripts/check-known-red-evidence.py"), "--rows",
                           str(REPO / "docs/Testing/KeystoneKnownReds.md")], capture_output=True, text=True,
                          check=True).stdout
    for row in rows.splitlines():
        key, pattern, site, _ = row.split("\x1f")
        if not site and re.fullmatch(r"[A-Za-z0-9 _'-]+", pattern) and not pattern.startswith("not classifier"):
            return pattern
    raise AssertionError("no unpinned known-red row with a literal pattern to plant")


def fixture(root, wording):
    (root / "tests").mkdir()
    (root / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.0.0"\nedition = "2021"\n\n'
                                     '[workspace]\n')
    (root / "src").mkdir()
    (root / "src/lib.rs").write_text("")
    tests = "".join(f"#[test]\nfn {t}() {{\n    act({i});\n}}\n\n" for i, t in enumerate(TESTS))
    (root / "tests/main.rs").write_text(f"""fn act(i: usize) {{
    let plan = std::env::var("PLAN").unwrap();
    if plan[..i].contains('U') {{
        let secs = std::env::var("SLEEP_AFTER_U").map_or(0, |s| s.parse().unwrap());
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }}
    match &plan[i..=i] {{
        "P" => {{}}
        "K" => panic!("{wording}"),
        "U" => panic!("{UNLISTED}"),
        "X" => {{
            let nextest = std::os::unix::process::parent_id().to_string();
            std::process::Command::new("kill").args(["-9", &nextest]).status().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(5));
        }}
        "S" => {{
            extern "C" {{ fn signal(sig: i32, handler: usize) -> usize; }}
            unsafe {{ signal(2, 1); }}
            let nextest = std::os::unix::process::parent_id().to_string();
            std::process::Command::new("kill").args(["-INT", &nextest]).status().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
        }}
        o => unreachable!("{{o}}"),
    }}
}}

{tests}""")


def ran_tests(output):
    """(test, status) per nextest status line of `output`, in order, up to its closing summary."""
    lines = output.splitlines()
    # nextest repeats the red status lines in its closing summary.
    end = next((i for i, l in enumerate(lines) if re.match(r"^ *(Summary|Cancelling|Canceling) ", l)), len(lines))
    return [(t, s) for t, s in (m.groups()[::-1] for m in map(STATUS.match, lines[:end]) if m)]
