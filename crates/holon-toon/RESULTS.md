# TOON vs Org: token measurement & recommendation

Reproduce:

```
cargo run -p holon-toon --example measure --features measure -- \
  "<vault>/<file-A>.org" "<vault>/<file-B>.org" "<vault>/<file-C>.org"
```

Measured on three real (private) vault files of different shapes:

- **File A** — the motivating file: large (249 blocks), drawer-heavy,
  dominated by DONE tasks.
- **File B** — prose-heavy project doc (62 blocks, long bodies).
- **File C** — tiny index file (9 blocks).

```
```

Tokenizer: **`tiktoken-rs` `o200k_base`** (the GPT-4o/o200k BPE). Claude uses a
different tokenizer; o200k is a well-correlated, fully-reproducible public proxy
for English + code. Treat the **ratios** as the finding, not the absolute
counts. Three renderings are produced from the *same* parsed forest, so the
comparison is apples-to-apples:

- **org_full** — canonical org: full `:PROPERTIES:/:ID:/:END:` drawer per block.
- **org_comp** — org with the `:ID:` drawer compressed to a trailing headline
  token (`{#id}`); the 3-line scaffolding is dropped, other drawer keys stay.
- **toon** — the 6-column tabular projection in this crate.

## Results

| dataset | blocks | org_full | org_comp | toon | TOON vs full | TOON vs comp | ids only | round-trip |
|---|--:|--:|--:|--:|--:|--:|--:|:--:|
| File A — drawer-heavy (all) | 249 | 13768 | 11743 | 12002 | **+12.8%** | −2.2% | 5366 | ok |
| File A — drawer-heavy (no DONE) | 137 | 7930 | 6846 | 6954 | **+12.3%** | −1.6% | 2964 | ok |
| File B — prose-heavy (all) | 62 | 9366 | 9007 | 9195 | +1.8% | −2.1% | 1016 | ok |
| File B — prose-heavy (no DONE) | 61 | 9195 | 8837 | 9027 | +1.8% | −2.2% | 1011 | ok |
| File C — tiny (all) | 9 | 950 | 929 | 885 | +6.8% | +4.7% | 42 | ok |
| File C — tiny (no DONE) | 9 | 950 | 929 | 885 | +6.8% | +4.7% | 42 | ok |

`ids only` = tokens spent on the bare block ids alone — the irreducible cost
**every** format pays once per block.

## Verdict: significant savings? **No — not from the container format.**

- **vs naive org:** TOON saves a meaningful **~12–13%** on the drawer-heavy,
  DONE-task-dominated motivating file, but only **~2%** on the prose-heavy
  prose-heavy File B. The win is entirely the collapse of the repeated
  `:PROPERTIES:/:ID:/:END:` scaffolding — so it's large exactly when that
  scaffolding dominates and small otherwise.
- **vs ID-compressed org:** TOON is a **wash to slightly worse** (−1.6% to
  −2.2%) on every file big enough to matter. Once you stop writing the 3-line ID
  drawer, org and TOON encode the same information at the same cost; TOON's
  per-row commas even tip it slightly negative. TOON only wins on the tiny
  File C, where its fixed header amortizes over almost no rows... the reverse
  of what you'd expect, and within noise.
- **The filtered ("no DONE") projection** — the actual agent use case — shows
  the *same* ratios. Filtering changes the block count, not the per-block
  economics.

### Why the container format barely matters

The dominant, irreducible cost is the **per-block UUID**. On the motivating
file, bare ids are **5366 of 12002 TOON tokens — 45% of the entire payload** —
and both org and TOON must carry every id exactly once. TOON's tabular array can
only dedup the *key names* (`:ID:`, `:PROPERTIES:`), a fixed ~5 tokens/block;
it cannot touch the ids, the titles, or the bodies, which are the bulk. That
ceiling is why TOON tops out at ~12% and evaporates against compressed org.

**The real lever is id length, not the serialization format.** A 36-char UUID
tokenizes to ~15–22 tokens (hyphens split it into many pieces). Replacing the
projection's ids with short, dense, agent-local handles (e.g. a 4–6 char base62
alias assigned per query, mapped back on patch) would save on the order of
**4000 tokens on this file alone** — several times more than the entire org→TOON
delta — and it is orthogonal to whether the container is org, TOON, or JSON.

## Recommendation

**Stick with compressed org; do not adopt TOON as the projection container.**
Decisive tradeoff: TOON buys no durable token win over ID-compressed org (it's
net-negative on real files), while it *loses* org's biggest strengths for this
use case — human/agent legibility of multi-line bodies and source blocks (which
TOON must flatten to one `\n`-escaped line) and native tooling (Emacs, the
existing `holon-org-format` parser, the whole vault). The structural win people
expect from TOON's tabular arrays is real only for wide, uniform, value-heavy
rows; a block forest is narrow, deep, and id-dominated, so the win doesn't
materialize.

If token budget on these projections is a real constraint, invest in the
orthogonal, higher-leverage lever instead:

1. **Short per-query id aliases** (biggest win, ~30–45% of payload is ids).
2. **Drop the `:ID:` drawer to a trailing token** in the org projection
   (captures ~15% vs naive org, keeps org legibility and tooling).
3. Optionally omit empty drawers entirely.

A hybrid (org container + aliased ids) dominates TOON on both axes — fewer
tokens *and* more legible — which is why the recommendation is org, not TOON.

## Caveats / threats to validity

- o200k ≠ Claude's tokenizer; absolute counts will shift, ratios much less so.
- The org renderings are reconstructed by this crate's measurement fixture
  (`org_reader.rs`), not the production `holon-org-format` renderer, so the org
  baselines are representative rather than byte-identical to on-disk files
  (source blocks are folded into body text by the fixture). This *understates*
  org's real per-block scaffolding slightly, making TOON look marginally better
  than it is — it does not change the conclusion.
- Only three files sampled, all from one project vault. A vault of wide, uniform,
  property-heavy blocks (few IDs, many shared columns) is the shape where TOON
  could genuinely win; none of Holon's real files look like that.
