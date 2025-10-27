#!/usr/bin/env python3
"""Summarize a samply CPU profile (Firefox Profiler format).

samply records sampled, on-CPU stacks. This script aggregates them into the
two numbers that matter for "what is actually burning CPU":

  - SELF time:      samples whose *leaf* frame is a function (the hot code).
  - INCLUSIVE time: samples with the function *anywhere* in the stack.

Unlike `tracing`/chrome-trace spans (which measure wall time and include
idle/await), a sampled profile shows only on-CPU work — so a 150ms settle
`await` is invisible here, while a tight `interpret`/alloc loop stands out.
Use this alongside scripts/analyze-chrome-trace.py: chrome-trace tells you
which phase is slow (incl. waits), this tells you where the CPU goes.

`samply record --save-only -o prof.json.gz` writes an UNsymbolicated profile
(meta.symbolicated == false): leaf frames show as `0x<addr>`. This script
symbolicates on demand via `atos` (macOS) or `addr2line`/`llvm-symbolizer`
(Linux), batched per library, using the lib paths embedded in the profile.
Rust symbols are demangled with `rustfilt` if installed, else a built-in
best-effort demangler.

Usage:
  scripts/analyze-samply-profile.py prof.json.gz [--top N] [--thread SUBSTR]
                                    [--inclusive SUBSTR ...] [--no-symbolicate]

  # Record a profile first, e.g.:
  samply record --save-only -o prof.json.gz -- \
    ./target/debug/deps/general_e2e_pbt-<hash> --exact general_e2e_pbt_sql_only

Notes:
  - Default thread = the busiest one (most samples). Pass --thread to pick
    another by name substring; "ALL" aggregates every thread.
  - Weight is the per-sample CPU delta when present (weightType
    "samples"/"ms"), else 1 sample == 1 unit.
"""

from __future__ import annotations

import argparse
import gzip
import json
import re
import shutil
import subprocess
import sys
from collections import Counter, defaultdict


def load_profile(path: str) -> dict:
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        return json.load(f)


# ── Rust symbol demangling ────────────────────────────────────────────

_HASH_TAIL = re.compile(r"::h[0-9a-f]{16}$")


def _scan_length_prefixed(name: str) -> list[str]:
    """Pull length-prefixed identifiers (`<N><N chars>`) out of a mangled name.

    Honouring the length prefix is what makes this reliable: it consumes
    exactly N characters per identifier, so it doesn't bleed into the
    following base-62 disambiguator (e.g. a crate root `Cs<hash>_5alloc`
    yields `alloc`, not `<hash>_5alloc`). v0 identifiers may carry a `_`-
    terminated byte-count or a unicode `u` tag; we skip those and keep the
    plain idents — enough to read `crate::module::Type::method`.
    """
    out: list[str] = []
    i, n = 0, len(name)
    while i < n:
        if not name[i].isdigit():
            i += 1
            continue
        j = i
        while j < n and name[j].isdigit():
            j += 1
        length = int(name[i:j])
        ident = name[j : j + length]
        i = j + length
        if not ident or not re.fullmatch(r"[A-Za-z_][\w]*", ident):
            continue
        # Drop crate-disambiguator idents that are pure base-62 hashes.
        if re.fullmatch(r"[0-9A-Za-z]{10,}", ident) and not re.search(r"[a-z]{3}", ident):
            continue
        out.append(ident)
    return out


def demangle_builtin(name: str) -> str:
    """Best-effort demangle without external tools.

    Not a faithful demangler — it drops generics/lifetimes — but it turns an
    opaque mangled blob into a readable `crate::module::Type::method`, which
    is all a profile summary needs. Non-Rust names pass through unchanged.
    """
    if name.startswith(("_R", "__R", "_ZN", "__ZN")):
        parts = _scan_length_prefixed(name)
        return "::".join(parts) if parts else name
    return _HASH_TAIL.sub("", name)


class Demangler:
    def __init__(self):
        self.rustfilt = shutil.which("rustfilt")
        self._cache: dict[str, str] = {}

    def __call__(self, name: str) -> str:
        if name in self._cache:
            return self._cache[name]
        out = self._demangle(name)
        self._cache[name] = out
        return out

    def _demangle(self, name: str) -> str:
        if self.rustfilt and (name.startswith("_R") or name.startswith("_ZN")):
            try:
                r = subprocess.run(
                    [self.rustfilt], input=name, capture_output=True, text=True, timeout=5
                )
                if r.returncode == 0 and r.stdout.strip():
                    return r.stdout.strip()
            except (subprocess.SubprocessError, OSError):
                pass
        return demangle_builtin(name)


# ── Symbolication (atos / addr2line) ──────────────────────────────────

def text_vmaddr(binary: str) -> int:
    """__TEXT segment vmaddr for a Mach-O binary (atos load base). 0 if n/a."""
    try:
        out = subprocess.run(
            ["otool", "-l", binary], capture_output=True, text=True, timeout=20
        ).stdout
    except (subprocess.SubprocessError, OSError):
        return 0
    in_text = False
    for line in out.splitlines():
        s = line.split()
        if len(s) >= 2 and s[0] == "segname" and s[1] == "__TEXT":
            in_text = True
        elif in_text and s and s[0] == "vmaddr":
            return int(s[1], 16)
    return 0


def symbolicate_macos(binary: str, addrs: list[int]) -> dict[int, str]:
    """Resolve lib-relative addresses with `atos`. Returns {addr: symbol}."""
    base = text_vmaddr(binary)
    arch = "arm64" if "arm64" in (
        subprocess.run(["file", binary], capture_output=True, text=True).stdout
    ) else "x86_64"
    syms: dict[int, str] = {}
    # atos handles many addresses per call; chunk to keep argv sane.
    for i in range(0, len(addrs), 512):
        chunk = addrs[i : i + 512]
        args = ["atos", "-o", binary, "-arch", arch, "-l", hex(base)] + [
            hex(base + a) for a in chunk
        ]
        try:
            out = subprocess.run(args, capture_output=True, text=True, timeout=120).stdout
        except (subprocess.SubprocessError, OSError):
            out = ""
        lines = out.splitlines()
        for a, line in zip(chunk, lines):
            # "symbol (in lib) (file:line)" or bare "0x.." if unresolved.
            sym = line.split(" (in ")[0].strip()
            syms[a] = sym if sym and not sym.startswith("0x") else hex(a)
    return syms


def symbolicate_linux(binary: str, addrs: list[int]) -> dict[int, str]:
    tool = shutil.which("llvm-symbolizer") or shutil.which("addr2line")
    if not tool:
        return {a: hex(a) for a in addrs}
    syms: dict[int, str] = {}
    if "llvm-symbolizer" in tool:
        inp = "\n".join(f"{binary} {hex(a)}" for a in addrs)
        try:
            out = subprocess.run(
                [tool], input=inp, capture_output=True, text=True, timeout=120
            ).stdout
        except (subprocess.SubprocessError, OSError):
            return {a: hex(a) for a in addrs}
        blocks = out.split("\n\n")
        for a, blk in zip(addrs, blocks):
            first = blk.strip().splitlines()
            syms[a] = first[0].strip() if first else hex(a)
    else:  # addr2line
        args = [tool, "-f", "-C", "-e", binary] + [hex(a) for a in addrs]
        try:
            out = subprocess.run(args, capture_output=True, text=True, timeout=120).stdout
        except (subprocess.SubprocessError, OSError):
            return {a: hex(a) for a in addrs}
        names = out.splitlines()[0::2]
        for a, nm in zip(addrs, names):
            syms[a] = nm.strip() or hex(a)
    return syms


def symbolicate(binary: str, addrs: list[int]) -> dict[int, str]:
    if sys.platform == "darwin":
        return symbolicate_macos(binary, addrs)
    return symbolicate_linux(binary, addrs)


# ── Aggregation ───────────────────────────────────────────────────────

def pick_threads(profile: dict, thread_sel: str | None) -> list[dict]:
    threads = profile["threads"]
    if thread_sel and thread_sel.upper() == "ALL":
        return threads
    if thread_sel:
        hits = [t for t in threads if thread_sel.lower() in (t.get("name") or "").lower()]
        if not hits:
            sys.exit(f"no thread name matches {thread_sel!r}; "
                     f"available: {sorted({t.get('name') for t in threads})}")
        return hits
    return [max(threads, key=lambda t: len(t["samples"]["stack"]))]




def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("profile", help="samply profile (.json or .json.gz)")
    ap.add_argument("--top", type=int, default=30, help="rows to show (default 30)")
    ap.add_argument("--thread", help="thread name substring, or ALL (default: busiest)")
    ap.add_argument("--inclusive", action="append", default=[],
                    help="also report inclusive time for funcs matching SUBSTR (repeatable)")
    ap.add_argument("--no-symbolicate", action="store_true",
                    help="skip atos/addr2line; leave 0x<addr> leaves")
    ap.add_argument("--no-demangle", action="store_true")
    args = ap.parse_args()

    profile = load_profile(args.profile)
    libs = profile["libs"]
    sym = "(symbolicated)" if profile.get("meta", {}).get("symbolicated") else "(raw addrs)"
    threads = pick_threads(profile, args.thread)
    demangle = (lambda s: s) if args.no_demangle else Demangler()

    # Pass 1: collect unsymbolicated addresses per library (so we can batch
    # one atos/addr2line call per binary). No counting here — frames are
    # shared across samples, so we tally in pass 2 once names are known.
    addr_by_lib: dict[str, set[int]] = defaultdict(set)
    frame_addr: dict[int, dict[int, tuple[str, int]]] = {}  # thread -> frame -> (libpath, addr)

    for ti, t in enumerate(threads):
        ft = t["frameTable"]; fn = t["funcTable"]; rt = t["resourceTable"]
        strs = t["stringArray"]
        fr_func = ft["func"]; fr_addr = ft["address"]
        f_res = fn["resource"]; f_name = fn["name"]; res_lib = rt["lib"]
        fmap: dict[int, tuple[str, int]] = {}
        frame_addr[ti] = fmap
        if args.no_symbolicate:
            continue
        for fi in range(ft["length"]):
            if not strs[f_name[fr_func[fi]]].startswith("0x"):
                continue
            res = f_res[fr_func[fi]]
            lib = res_lib[res] if (res is not None and res >= 0) else None
            path = libs[lib].get("path") if lib is not None else None
            addr = fr_addr[fi]
            if path and addr is not None:
                addr_by_lib[path].add(addr)
                fmap[fi] = (path, addr)

    # Pass 1b: symbolicate collected addresses, one batch per library.
    resolved: dict[tuple[str, int], str] = {}
    for path, addrs in addr_by_lib.items():
        alist = sorted(addrs)
        syms = symbolicate(path, alist)
        for a in alist:
            resolved[(path, a)] = syms.get(a, hex(a))

    # Pass 2: tally self + inclusive weight, keyed by final display name.
    self_w: Counter[str] = Counter()
    incl_w: Counter[str] = Counter()
    total = 0.0
    for ti, t in enumerate(threads):
        ft = t["frameTable"]; fn = t["funcTable"]; st = t["stackTable"]
        sm = t["samples"]; strs = t["stringArray"]
        fr_func = ft["func"]; f_name = fn["name"]; sf = st["frame"]; sp = st["prefix"]
        fmap = frame_addr[ti]
        weights = sm.get("weight") or [1] * len(sm["stack"])

        name_cache: dict[int, str] = {}

        def disp(fi: int) -> str:
            if fi in name_cache:
                return name_cache[fi]
            if fi in fmap:
                path, addr = fmap[fi]
                nm = resolved.get((path, addr)) or hex(addr)
                if nm.startswith("0x"):
                    nm = f"{libs_name(path)}+{nm}"
            else:
                nm = strs[f_name[fr_func[fi]]]
            nm = demangle(nm)
            name_cache[fi] = nm
            return nm

        for s, w in zip(sm["stack"], weights):
            if s is None:
                continue
            w = w or 0
            total += w
            self_w[disp(sf[s])] += w
            seen = set(); cur = s
            while cur is not None:
                seen.add(disp(sf[cur])); cur = sp[cur]
            for nm in seen:
                incl_w[nm] += w

    tnames = ", ".join(sorted({t.get("name") or "?" for t in threads}))
    print(f"profile: {args.profile} {sym}")
    print(f"threads: {tnames}  |  total on-CPU weight: {total:.0f}")
    if total == 0:
        return

    print(f"\n=== TOP {args.top} SELF-TIME (on-CPU leaf) ===")
    for nm, w in self_w.most_common(args.top):
        print(f"{100 * w / total:6.2f}%  {nm[:100]}")

    for needle in args.inclusive:
        print(f"\n=== INCLUSIVE time, funcs matching {needle!r} ===")
        hits = sorted(
            ((nm, w) for nm, w in incl_w.items() if needle.lower() in nm.lower()),
            key=lambda x: -x[1],
        )
        if not hits:
            print("  (no match)")
        for nm, w in hits[:args.top]:
            print(f"{100 * w / total:6.2f}%  {nm[:100]}")


def libs_name(path: str) -> str:
    return path.rsplit("/", 1)[-1]


if __name__ == "__main__":
    main()
