"""A one-crate workspace whose tests t00..t07 act out a plan, for property checks over a real nextest run.

The env var PLAN gives each test's outcome: `P` passes, `K` panics with the wording of a known-red row of
docs/Testing/KeystoneKnownReds.md, `U` panics with a wording no row lists, `X` kills nextest, `S` interrupts it.
`C` and `D` print the lines of the env var CAPTURE first, then fail like `K` (`C`) or pass (`D`).
"""
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
N = 8
TESTS = [f"t{i:02}" for i in range(N)]
UNLISTED = "planted red outside the known-reds list"
STATUS = re.compile(r"^( *)([A-Z][A-Z0-9 +/-]*?) +\[[^]]*\] (?:\([^)]*\) )?\S+ (t\d\d)$")
# nextest indents the output a test captured by this many columns.
CAPTURE_INDENT = 4
# Lines a test can print that imitate nextest's own: the label and the pool of captured lines.
POOL = [("Summary", "Summary [ 0.1s] 1 test run: 1 passed"), ("Cancelling", "Cancelling due to test failure"),
        ("FAIL", "FAIL [ 0.001s] (1/1) other::bin ghost")]


def forges(label, indent):
    """True when a captured line that starts with `label` after `indent` spaces reads as nextest's own line."""
    return indent + CAPTURE_INDENT + len(label) == 12


def captured_line(pool_index, indent):
    return " " * indent + POOL[pool_index][1]


def known_red_wording():
    rows = subprocess.run([sys.executable, str(REPO / "scripts/check-known-red-evidence.py"), "--rows",
                           str(REPO / "docs/Testing/KeystoneKnownReds.md")], capture_output=True, text=True,
                          check=True).stdout
    for row in rows.splitlines():
        key, pattern, site, *_ = row.split("\x1f")
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
    let capture = || std::env::var("CAPTURE").unwrap().lines().for_each(|l| println!("{{l}}"));
    match &plan[i..=i] {{
        "P" => {{}}
        "K" => panic!("{wording}"),
        "C" => {{
            capture();
            panic!("{wording}")
        }}
        "D" => capture(),
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
    """(test, status) per nextest status line of `output`, in order, up to its closing summary.

    A line counts only at nextest's own column, so a test's captured output (indented by CAPTURE_INDENT) cannot
    end the run early or add a test.
    """
    lines = output.splitlines()
    # nextest repeats the red status lines in its closing summary.
    end = next((i for i, l in enumerate(lines) if re.match(r" {5}Summary ", l)), len(lines))
    found = (m.groups() for m in map(STATUS.match, lines[:end]) if m)
    # nextest adds ` + LEAK` to a test's label when the host is loaded enough to delay its output pipe's close.
    return [(t, s.removesuffix(" + LEAK")) for indent, s, t in found if len(indent) + len(s) == 12]
