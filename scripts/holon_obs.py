#!/usr/bin/env python3
"""Holon observability collector + publisher.

Every keystone / GPUI PBT run already writes a `<run_id>.result.json` into
`.observability/runs/` as a side effect of the run itself (see
`crates/holon-integration-tests/src/pbt/run_result.rs`). This script does the
two jobs that are NOT part of the run:

  collect   — convert each `.result.json` into canonical `.yaml`, and rebuild
              the cumulative `runs.json` summary index.
  push      — collect, then upload `runs.json` (--clobber) plus any new
              `runs/<id>.yaml` into the GitHub `observability` release.

Data flow (declarative files, no server, no database):

  Rust harness ──► .result.json ──► .yaml (canonical) ──► runs.json (index)
                   (local only)     (local + release)     (local + release)

The static report JS fetches `runs.json`, then lazy-loads `runs/<id>.yaml` per
run. Local runs are a first-class source: the report reads `.observability/`
when served locally, and the `observability` release when served from Pages.

Usage:
    python3 scripts/holon_obs.py collect [--dir .observability/runs]
    python3 scripts/holon_obs.py push    [--dir .observability/runs] [--dry-run]
    python3 scripts/holon_obs.py serve   [--port 8000]      # local report viewing

Idempotency: `collect` regenerates deterministically (same JSON -> same YAML);
`push` only uploads `.yaml` files it has not already marked uploaded, and
`runs.json` is uploaded with `--clobber` (one cumulative asset). The canonical
format is YAML; `runs.json` is a derived mirror the browser can fetch.
"""

import argparse
import datetime
import json
import os
import socket
import subprocess
import sys
from pathlib import Path

import yaml


def repo_root(start=None):
    """Walk up from `start` (default cwd) to the directory containing `.git`."""
    cur = Path(start or os.getcwd()).resolve()
    for d in [cur, *cur.parents]:
        if (d / ".git").exists():
            return d
    return cur


def base_dir(repodir, dir_override=None):
    """`.observability/` in the repo root, unless --dir overrides the runs dir."""
    if dir_override:
        return Path(dir_override).resolve()
    return repodir / ".observability"


def iso_from_ms(ms):
    try:
        dt = datetime.datetime.fromtimestamp(ms / 1000.0, tz=datetime.timezone.utc)
        return dt.strftime("%Y-%m-%dT%H:%M:%SZ")
    except (TypeError, ValueError, OSError):
        return None


def now_iso():
    """Current UTC wall clock as an ISO-8601 string ('' on failure)."""
    try:
        ms = int(datetime.datetime.now(tz=datetime.timezone.utc).timestamp() * 1000)
    except (OSError, OverflowError, ValueError):
        ms = 0
    return iso_from_ms(ms) or ""


def canonical(record, git_rev, host):
    """Convert a harness `.result.json` dict into the canonical YAML dict."""
    started = record.get("started_at_ms", 0)
    panics = record.get("panics", [])
    return {
        "schema": 1,
        "run_id": record.get("run_id", ""),
        "kind": record.get("kind", ""),
        "git_rev": record.get("git_rev") or git_rev,
        "host": host,
        "started_at": iso_from_ms(started) or "",
        "duration_ms": record.get("duration_ms", 0),
        "cases": record.get("cases", 0),
        "verdict": record.get("verdict", "red"),
        "panics": [
            {"message": p.get("message", ""), "location": p.get("location", "")}
            for p in panics
        ],
    }


def index_entry(y):
    """The report's index entry: the full canonical record plus a convenience
    `panic_count` so the report needs a single fetch (no per-run YAML round-trips)."""
    entry = dict(y)
    entry["panic_count"] = len(y.get("panics", []))
    return entry


def collect(runs_dir, git_rev, host):
    """Convert every `.result.json` -> `.yaml`; return list of (run_id, yaml path)."""
    runs_dir = Path(runs_dir)
    runs_dir.mkdir(parents=True, exist_ok=True)
    converted = []
    for jp in sorted(runs_dir.glob("*.result.json")):
        try:
            record = json.loads(jp.read_text())
        except (json.JSONDecodeError, OSError) as e:
            print(f"[obs] skipping unparseable {jp}: {e}", file=sys.stderr)
            continue
        y = canonical(record, git_rev, host)
        yp = jp.with_name(f"{record.get('run_id', jp.stem[:-7])}.yaml")
        yp.write_text(yaml_dump(y))
        converted.append((record.get("run_id", ""), yp))
    return converted


def yaml_dump(obj):
    """Serialize the canonical record as YAML (deterministic, standard)."""
    return yaml.safe_dump(obj, sort_keys=False, allow_unicode=True, width=100)


def rebuild_index(base, runs_dir, host):
    """Rebuild `runs.json` from all `*.yaml` in runs_dir, sorted chronologically."""
    runs_dir = Path(runs_dir)
    entries = []
    for yp in sorted(runs_dir.glob("*.yaml")):
        y = parse_yaml(yp.read_text())
        if not y:
            continue
        entries.append(index_entry(y))
    entries.sort(key=lambda e: (e["started_at"], e["run_id"]))
    idx = {
        "schema": 1,
        "updated_at": now_iso(),
        "count": len(entries),
        "runs": entries,
    }
    (Path(base) / "runs.json").write_text(
        json.dumps(idx, indent=2, sort_keys=False) + "\n"
    )
    return idx


def collect_captures(base):
    """Index capture dirs under `<base>/captures/` (one `events.jsonl` each).

    Writes `<base>/captures.json`, the report's flipbook manifest: one entry per
    capture session with its frame events, so the static report can render a
    scrubber without any server-side directory listing.
    """
    caps_dir = Path(base) / "captures"
    captures = []
    if caps_dir.is_dir():
        for events_file in sorted(caps_dir.glob("*/events.jsonl")):
            cid = events_file.parent.name
            events = []
            for raw in events_file.read_text().splitlines():
                line = raw.strip()
                if not line:
                    continue
                try:
                    events.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
            captures.append(
                {
                    "id": cid,
                    "dir": f"captures/{cid}",
                    "frames": len(events),
                    "events": events,
                }
            )
    captures.sort(key=lambda c: c["id"])
    idx = {
        "schema": 1,
        "updated_at": now_iso(),
        "count": len(captures),
        "captures": captures,
    }
    (Path(base) / "captures.json").write_text(
        json.dumps(idx, indent=2, sort_keys=False) + "\n"
    )
    return idx


def parse_yaml(text):
    """Parse a canonical record back (inverse of [`yaml_dump`])."""
    return yaml.safe_load(text) or {}


def git_head_rev(repodir):
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repodir,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError, OSError):
        return ""


def uploaded_marker(base):
    return Path(base) / ".uploaded"


def gh_cli_available():
    try:
        subprocess.run(["gh", "--version"], capture_output=True, check=True)
        return True
    except (FileNotFoundError, subprocess.CalledProcessError, OSError):
        return False


def push(base, runs_dir, release, dry_run):
    """Collect, then upload to the GitHub `observability` release via `gh`."""
    repodir = repo_root()
    git_rev = git_head_rev(repodir)
    host = socket.gethostname()

    converted = collect(runs_dir, git_rev, host)
    rebuild_index(base, runs_dir, host)
    index_path = Path(base) / "runs.json"

    marker = uploaded_marker(base)
    done = set()
    if marker.exists():
        done = set(marker.read_text().splitlines())

    to_upload = [p for rid, p in converted if rid not in done]

    def gh(*args):
        if dry_run:
            print("[obs] dry-run: gh", " ".join(args))
            return
        subprocess.run(["gh", *args], check=True)

    if not dry_run and not gh_cli_available():
        print(
            "[obs] error: `gh` CLI not found or not authenticated. Install it "
            "(https://cli.github.com) and run `gh auth login`.",
            file=sys.stderr,
        )
        return 0

    # Ensure the release exists (skip the read in dry-run, which must be offline).
    exists = False
    if not dry_run:
        exists = (
            subprocess.run(
                ["gh", "release", "view", release],
                capture_output=True,
            ).returncode
            == 0
        )
    if not exists:
        gh(
            "release",
            "create",
            release,
            "--title",
            release,
            "--notes",
            "Holon observability run records.",
        )
        if not dry_run:
            print(f"[obs] created release {release!r}")

    # runs.json is the ONE cumulative asset — clobber in place.
    gh("release", "upload", release, str(index_path), "--clobber")
    if not dry_run:
        print(f"[obs] uploaded {index_path} -> {release} (clobber)")

    # Per-run YAMLs are immutable and unique by run_id — never clobbered.
    for p in to_upload:
        gh("release", "upload", release, str(p))
        if not dry_run:
            print(f"[obs] uploaded {p} -> {release}")

    if not dry_run and to_upload:
        try:
            with open(marker, "a") as fh:
                for rid, _ in converted:
                    if rid not in done:
                        fh.write(rid + "\n")
        except OSError as e:
            print(f"[obs] could not write upload marker {marker}: {e}", file=sys.stderr)
    return len(to_upload)


def serve(root, port):
    """Serve the REPO root so both the committed report (`observability/report/`)
    and the gitignored data (`.observability/`) are reachable."""
    report_url = f"http://127.0.0.1:{port}/observability/report/"
    print(f"[obs] serving repo root at http://127.0.0.1:{port}/ (Ctrl-C to stop)")
    print(f"[obs] report: {report_url}")
    subprocess.run(
        [sys.executable, "-m", "http.server", str(port), "--directory", str(root)]
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("command", choices=["collect", "push", "serve"])
    ap.add_argument(
        "--dir",
        default=None,
        help="override the .observability dir (default: <repo>/.observability)",
    )
    ap.add_argument("--release", default="observability", help="GitHub release name")
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument(
        "--dry-run", action="store_true", help="print gh commands, do not run"
    )
    args = ap.parse_args()

    repodir = repo_root()
    base = Path(args.dir) if args.dir else repodir / ".observability"
    runs_dir = base / "runs"

    if args.command == "collect":
        git_rev = git_head_rev(repodir)
        host = socket.gethostname()
        converted = collect(runs_dir, git_rev, host)
        idx = rebuild_index(base, runs_dir, host)
        caps = collect_captures(base)
        print(
            f"[obs] collected {len(converted)} run(s); index has {idx['count']} run(s); "
            f"{caps['count']} capture session(s) -> {base}"
        )
    elif args.command == "push":
        n = push(base, runs_dir, args.release, args.dry_run)
        print(f"[obs] push complete ({n} new yaml file(s) uploaded)")
    elif args.command == "serve":
        serve(repodir, args.port)


if __name__ == "__main__":
    main()
