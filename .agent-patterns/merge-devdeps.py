#!/usr/bin/env python3
"""merge-devdeps.py <target Cargo.toml> <run Cargo.toml> : adds to the target's [dev-dependencies] every dev-dependency line of the run that the target lacks."""
import re, sys

def section(text, name):
    m = re.search(rf'(?ms)^\[{re.escape(name)}\]\n(.*?)(?=^\[|\Z)', text)
    return m

tgt, run = open(sys.argv[1]).read(), open(sys.argv[2]).read()
rs = section(run, 'dev-dependencies')
if rs is None:
    sys.exit(0)
# shortcut: a dependency spelled two ways (`x.workspace = true` vs `x = { workspace = true }`) is added twice and cargo refuses the manifest loudly; normalise the key if that happens.
have = {l.split('=')[0].strip() for l in (section(tgt, 'dev-dependencies') or [None, ''])[1].splitlines() if '=' in l}
add = [l for l in rs.group(1).splitlines() if '=' in l and not l.lstrip().startswith('#') and l.split('=')[0].strip() not in have]
if add:
    ts = section(tgt, 'dev-dependencies')
    tgt = tgt[:ts.end(1)] + '\n'.join(add) + '\n' + tgt[ts.end(1):] if ts else tgt.rstrip('\n') + '\n\n[dev-dependencies]\n' + '\n'.join(add) + '\n'
    open(sys.argv[1], 'w').write(tgt)
print(f'added {len(add)}: ' + ', '.join(l.split("=")[0].strip() for l in add))
