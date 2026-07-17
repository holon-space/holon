#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["igraph"]
# ///
"""Generate the Holon app icon as an SVG.

Self-contained uv script — `uv run` pulls the one dependency (igraph) into an
ephemeral env, so no manual install is needed:

    uv run scripts/gen_holon_icon.py                 # rounded squircle → holon-icon.svg
    uv run scripts/gen_holon_icon.py out.svg         # custom output path
    uv run scripts/gen_holon_icon.py fg.svg --transparent   # graph only, no tile (Android fg)
    uv run scripts/gen_holon_icon.py bg.svg --bg-only       # tile only, no graph  (Android bg)
    uv run scripts/gen_holon_icon.py sq.svg --square        # full-bleed, no rounded corners

The node layout is a real radial-tree algorithm (igraph's Reingold-Tilford
circular); everything else lives in the CONFIG block below.

Render a PNG preview (needs librsvg):  rsvg-convert -w 512 -h 512 holon-icon.svg -o preview.png
"""
import math, os, random, sys
import igraph

# ─────────────────────────── CONFIG (tweak me) ───────────────────────────
SEED        = 7            # change to reshuffle the random tree
SIZE        = 1024         # canvas (square)
CORNER      = 230          # rounded-square corner radius
FILL_SCALE  = 1.5          # visual zoom of the graph about the center (also thickens strokes)
FILL_FRACT  = float(os.environ.get("HOLON_FILL_FRACT", "0.88"))  # outermost node reaches this
                           # fraction of the half-tile (auto-fills the tile; env-overridable)

# Tile background gradient (top-left → bottom-right)
TILE_STOPS  = [(0.0, "#2b2c5e"), (0.55, "#22506b"), (1.0, "#166a78")]

# The sun (center = the tree root). Its radius is derived so it is ALWAYS the
# largest node: sun = max(node radius) × SUN_FACTOR (never smaller than SUN_R).
SUN_R       = 30.0          # floor for the sun radius
SUN_FACTOR  = 1.25          # sun is this much bigger than the biggest other node
SUN_STOPS   = [(0.0, "#fffef8"), (0.45, "#fff3cc"), (0.8, "#ffdf9e"), (1.0, "#ffc474")]
CORONA_MULT = 2.2           # corona radius = sun radius × this
CORONA_STOPS= [(0.0, "#ffb457", 0.85), (0.45, "#ff9a3d", 0.40), (1.0, "#ff9a3d", 0.0)]

# ── Tree shape (the layout algorithm handles placement; this is just topology) ──
MAX_DEPTH     = 2
BRANCHING     = {0: 6, 1: 3, 2: 2}   # children by depth; deeper depths use the default
BRANCHING_DEF = 1
PRUNE_PROB    = 0.14                  # chance to drop an individual (non-root) child → irregular

# ── Layout: a force-directed (Fruchterman-Reingold) relaxation, seeded from a
# radial tree. The radial seed keeps branches flowing out from the sun; the force
# pass adds the organic touch (uneven branch lengths, local clusters, off-ring
# nodes). FR_ITER trades the two off: higher = more organic, lower = more radial.
FR_ITER       = 200

# ── Trace mode (--trace): render exact node positions/sizes/opacity extracted
# from a reference image (scripts/detect_nodes.py → JSON) instead of a random
# tree. The layout is taken verbatim; only the vector styling below is applied. ──
TRACE_SRC     = os.environ.get("HOLON_TRACE_SRC", "/tmp/nodes.json")
RING_W_FRAC   = 0.15    # ring stroke width as a fraction of the node's outer radius
# Trace opacity is taken verbatim from the JSON `op` field (0 = use it as-is).
# The curated nodes.json already holds the final per-node opacity, including the
# node-6 subtree fade. Raise this to lift all rings into a floor band if desired.
TRACE_OP_MIN  = 0.0

# ── Node ring geometry (depth-generic: multiplicative, so any depth renders) ──
RING_R1     = 28.0; RING_R_DECAY = 0.66; RING_R_MIN = 3.0   # outer radius by depth
INNER_RATIO = 0.52                                           # inner ring = this · outer
RING_W1     = 4.2;  RING_W_DECAY = 0.72; RING_W_MIN = 1.4    # ring stroke width by depth
EDGE_W1     = 3.6;  EDGE_W_DECAY = 0.72; EDGE_W_MIN = 1.2    # connector width by depth
EDGE_TRIM   = 10.0                                           # extra gap trimmed off both ends

# ── Directional sun lighting: rings are lit FROM the sun. Near the sun they take
# the WARM tones; farther out they fade to cool gray. WARM_SPAN sets how far the
# warmth reaches (fraction of the graph radius); smaller = warm only at the core. ──
LIT_WARM_NEAR     = "#ffe9b4"   # sun-facing edge, near the sun (bright warm)
LIT_WARM_FAR      = "#c99a54"   # shadowed far edge, near the sun
LIT_COOL_NEAR     = "#efe7c6"   # sun-facing edge, far from the sun (pale yellow-gray)
LIT_COOL_FAR      = "#968c69"   # shadowed far edge, far from the sun (muted yellow-gray)
WARM_SPAN         = 0.42        # warmth reaches this fraction of the graph radius
NODE_FILL_COLOR   = None        # None → ring band uses the same sun-lit gradient
NODE_FILL_OPACITY = 0.24        # 0.0 = off · ~0.18 subtle · ~0.4 glowing

# ── Brightness: MONOTONIC — a child is never lighter than its parent ──
WARM         = (244, 230, 200)  # edge tint near the sun
COOL         = (207, 227, 230)  # edge tint at the rim
OPACITY_ROOT = 1.00             # the sun
OPACITY_DECAY= 0.82             # each generation dims by this factor
OPACITY_JIT  = (0.70, 1.00)     # darken-ONLY jitter (≤1) → variety that never brightens
OPACITY_FLOOR= 0.16
# ──────────────────────────────────────────────────────────────────────────

# CLI modes
FLAGS      = set(a for a in sys.argv[1:] if a.startswith("--"))
SQUARE     = bool(FLAGS & {"--square", "--transparent", "--bg-only"})
DRAW_TILE  = "--transparent" not in FLAGS
DRAW_GRAPH = "--bg-only" not in FLAGS

C = SIZE / 2
random.seed(SEED)
R_MAX     = (C * FILL_FRACT) / FILL_SCALE
COOL_DIST = R_MAX

def node_radius(depth):
    return SUN_R if depth == 0 else max(RING_R_MIN, RING_R1 * RING_R_DECAY ** (depth - 1))
def ring_width(depth):
    return max(RING_W_MIN, RING_W1 * RING_W_DECAY ** (depth - 1))
def edge_width(depth):
    return max(EDGE_W_MIN, EDGE_W1 * EDGE_W_DECAY ** depth)
def branching(depth):
    return BRANCHING.get(depth, BRANCHING_DEF)

def warmcool(x, y):
    t = min(1.0, math.hypot(x - C, y - C) / COOL_DIST)
    return "#%02x%02x%02x" % tuple(round(WARM[i] + (COOL[i] - WARM[i]) * t) for i in range(3))

# ── assemble the graph: index 0 is always the sun. Two sources: ──
if "--trace" in FLAGS:
    # verbatim layout traced from a reference image (positions/sizes/opacity)
    import json
    d = json.load(open(TRACE_SRC))
    src = d["nodes"]; sx, sy = d["sun"]
    span = max((math.hypot(n["x"] - sx, n["y"] - sy) for n in src), default=1.0) or 1.0
    k = R_MAX / span                                          # map sun→center, fill the tile
    pos    = [(C, C)] + [(C + (n["x"] - sx) * k, C + (n["y"] - sy) * k) for n in src]
    radii  = [SUN_R] + [max(3.0, n["r"] * k) for n in src]
    opac   = [OPACITY_ROOT] + [TRACE_OP_MIN + (1 - TRACE_OP_MIN) * float(n.get("op", 0.8)) for n in src]
    tree_edges = [((p + 1 if p >= 0 else 0), c + 1) for p, c in d["edges"]]  # -1 = sun → 0
    kids = {}
    for p, c in tree_edges: kids.setdefault(p, []).append(c)
    depths = [0] * len(pos); q = [0]
    while q:
        u = q.pop(0)
        for v in kids.get(u, []): depths[v] = depths[u] + 1; q.append(v)
else:
    # a random tree, laid out by radial-tree → Fruchterman-Reingold relaxation
    depths, opac, tree_edges = [0], [OPACITY_ROOT], []
    def grow(pid, depth):
        if depth >= MAX_DEPTH:
            return
        for _ in range(branching(depth)):
            if depth > 0 and random.random() < PRUNE_PROB:
                continue
            pop = opac[pid]
            cop = max(OPACITY_FLOOR, min(pop, pop * OPACITY_DECAY * random.uniform(*OPACITY_JIT)))
            cid = len(depths); depths.append(depth + 1); opac.append(cop)
            tree_edges.append((pid, cid))
            grow(cid, depth + 1)
    grow(0, 0)
    try:
        igraph.set_random_number_generator(random)            # reproducible force pass
    except Exception:
        pass
    g = igraph.Graph(n=len(depths), edges=tree_edges, directed=False)
    rt = g.layout_reingold_tilford_circular(root=[0])         # radial seed
    random.seed(SEED)
    lay = g.layout_fruchterman_reingold(niter=FR_ITER, seed=rt)
    cx, cy = lay[0]
    span = max(math.hypot(px - cx, py - cy) for px, py in lay) or 1.0
    k = R_MAX / span
    pos = [(C + (px - cx) * k, C + (py - cy) * k) for px, py in lay]
    radii = [node_radius(depths[i]) for i in range(len(depths))]

# Sun is always the largest node: size it from the biggest other node.
sun_r = max(max(radii[1:], default=SUN_R) * SUN_FACTOR, SUN_R)
radii[0] = sun_r
corona_r = sun_r * CORONA_MULT

def stops(items):
    out = []
    for s in items:
        op = f' stop-opacity="{s[2]}"' if len(s) > 2 else ""
        out.append(f'<stop offset="{s[0]}" stop-color="{s[1]}"{op}/>')
    return "".join(out)

def _rgb(h): h = h.lstrip("#"); return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))
def _mix(a, b, t): return "#%02x%02x%02x" % tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))
_WN, _WF = _rgb(LIT_WARM_NEAR), _rgb(LIT_WARM_FAR)
_CN, _CF = _rgb(LIT_COOL_NEAR), _rgb(LIT_COOL_FAR)

grad_defs = []
def sun_gradient(idx, x, y, Ro):
    dx, dy = C - x, C - y; L = math.hypot(dx, dy) or 1.0
    ux, uy = dx / L, dy / L
    t = min(1.0, math.hypot(x - C, y - C) / (WARM_SPAN * R_MAX))   # 0 at sun → 1 cool
    near, far = _mix(_WN, _CN, t), _mix(_WF, _CF, t)
    gid = f"lit{idx}"
    grad_defs.append(
        f'<linearGradient id="{gid}" gradientUnits="userSpaceOnUse" '
        f'x1="{x + ux * Ro:.1f}" y1="{y + uy * Ro:.1f}" '
        f'x2="{x - ux * Ro:.1f}" y2="{y - uy * Ro:.1f}">'
        f'<stop offset="0" stop-color="{near}"/>'
        f'<stop offset="1" stop-color="{far}"/></linearGradient>')
    return gid

def hollow(idx, x, y):
    Ro = radii[idx]; Ri = Ro * INNER_RATIO
    gid = sun_gradient(idx, x, y, Ro)
    band_paint = f'url(#{gid})' if NODE_FILL_COLOR is None else NODE_FILL_COLOR
    sw = max(RING_W_MIN, Ro * RING_W_FRAC)
    parts = [f'<g opacity="{opac[idx]:.2f}">']
    if NODE_FILL_OPACITY > 0:
        band = (Ro + Ri) / 2
        parts.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{band:.1f}" fill="none" '
                     f'stroke="{band_paint}" stroke-width="{Ro - Ri:.1f}" opacity="{NODE_FILL_OPACITY}"/>')
    parts.append(f'<g stroke="url(#{gid})" fill="none">'
                 f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{Ro:.1f}" stroke-width="{sw:.1f}"/>'
                 f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{Ri:.1f}" stroke-width="{sw * 0.8:.1f}"/></g></g>')
    return "".join(parts)

graph_svg = ""
if DRAW_GRAPH:
    edge_svg = ""
    for pid, cid in tree_edges:
        x1, y1 = pos[pid]; x2, y2 = pos[cid]
        dx, dy = x2 - x1, y2 - y1; L = math.hypot(dx, dy) or 1.0
        ux, uy = dx / L, dy / L
        ra, rb = radii[pid], radii[cid]
        avail = L - ra - rb
        trim = min(EDGE_TRIM, max(0.0, (avail - 4) / 2))
        if avail - 2 * trim <= 2:
            continue
        a, b = x1 + ux * (ra + trim), y1 + uy * (ra + trim)
        c, d = x2 - ux * (rb + trim), y2 - uy * (rb + trim)
        edge_svg += (f'<line x1="{a:.1f}" y1="{b:.1f}" x2="{c:.1f}" y2="{d:.1f}" stroke-linecap="round" '
                     f'stroke="{warmcool((a + c) / 2, (b + d) / 2)}" stroke-width="{edge_width(depths[pid]):.1f}" '
                     f'opacity="{max(0.12, 0.6 * opac[cid]):.2f}"/>')
    # faint (far) nodes first so brighter, nearer nodes and the sun sit on top
    order = sorted((i for i in range(len(depths)) if depths[i] != 0), key=lambda i: opac[i])
    node_svg = "".join(hollow(i, pos[i][0], pos[i][1]) for i in order)
    graph_svg = (f'<g>{edge_svg}</g><g>{node_svg}</g>'
                 f'<circle cx="{C}" cy="{C}" r="{corona_r:.1f}" fill="url(#corona)"/>'
                 f'<circle cx="{C}" cy="{C}" r="{sun_r:.1f}" fill="url(#sun)"/>')

rx = 0 if SQUARE else CORNER
tile_svg = f'<rect x="0" y="0" width="{SIZE}" height="{SIZE}" fill="url(#tile)"/>' if DRAW_TILE else ""

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}" width="{SIZE}" height="{SIZE}">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="1" y2="1">{stops(TILE_STOPS)}</linearGradient>
    <radialGradient id="corona" cx="0.5" cy="0.5" r="0.5">{stops(CORONA_STOPS)}</radialGradient>
    <radialGradient id="sun" cx="0.5" cy="0.5" r="0.5">{stops(SUN_STOPS)}</radialGradient>
    {"".join(grad_defs)}
    <clipPath id="squircle"><rect x="0" y="0" width="{SIZE}" height="{SIZE}" rx="{rx}" ry="{rx}"/></clipPath>
  </defs>
  <g clip-path="url(#squircle)">
    {tile_svg}
    <g transform="translate({C} {C}) scale({FILL_SCALE}) translate(-{C} -{C})">
      {graph_svg}
    </g>
  </g>
</svg>'''

args = [a for a in sys.argv[1:] if not a.startswith("--")]
out = args[0] if args else "holon-icon.svg"
with open(out, "w") as f:
    f.write(svg)
print(f"wrote {out}  ({len(depths)} nodes, {len(tree_edges)} edges, MAX_DEPTH={MAX_DEPTH})")
