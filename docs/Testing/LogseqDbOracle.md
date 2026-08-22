# The LogSeq DB oracle

`crates/holon-logseq-db/tests/kvs_round_trip.rs` checks a graph Holon wrote
against **LogSeq's own** validator and graph diff. Those two legs need a LogSeq
checkout with its JS toolchain, so both are `#[ignore]`d and `just lsqdb-oracle`
is the only thing that runs them. This file is how to get them running.

## The gate

**A W-lane touching `holon-logseq-db` is not green until `just lsqdb-oracle`
passes.** `cargo test` alone is not sufficient: it reports the two oracle legs
as *ignored*, never as passing.

```sh
cargo test -p holon-logseq-db --all-targets   # legs 1 and 2 show as ignored
HOLON_LOGSEQ_ORACLE=/path/to/logseq-oracle just lsqdb-oracle   # runs all 5
```

There is no skip-when-absent path. If the checkout is missing, incomplete, or
the restored scripts are gone, the tests **fail** with the reason and a pointer
back here. An oracle leg that passes by not running would be worse than no
oracle at all, which is the whole reason for the `#[ignore]` + recipe split.

### Why leg 2 in particular is not optional

LogSeq replays the transaction tail through `db-with-tail-datoms`, which wraps
its transact in `(catch :default _ db)`. A tail transaction LogSeq cannot
replay is therefore **dropped in silence** — the graph loads as though the edit
never happened, with no error on LogSeq's side and none on Holon's.

Holon refuses any tail entry not shaped `[e a v ±tx]` before writing it
(`RowError::MalformedTailDatom`), which catches malformed output. It cannot
catch output that is well-formed but wrong — a correct-looking datom naming the
wrong entity, attribute, or transaction. The only check that does is
`diff_graphs` asserting the delta is EXACTLY the intended change, which is leg
2. Any increment that writes runs it.

## The rev the oracle must be at

The oracle has to be the schema version the graph under test was written by,
or its verdict is about the version gap rather than about Holon.

- Fixture `crates/holon-logseq-db/tests/fixtures/logseq-db/holontest.sqlite`
  carries `:logseq.kv/schema-version {:major 65 :minor 33}`.
- `logseq/logseq@fab27740975dcda1e93dbca718d1f620eda543c7` (2026-08-18) has
  `(def version (parse-schema-version "65.33"))` in
  `deps/db/src/logseq/db/frontend/schema.cljs`. That is the oracle rev.
- Its DataScript fork pin is
  `github.com/logseq/datascript@3f141af97b70e1f14c65eaa119acd822ebece37e`
  (`deps/db/deps.edn`). Nothing in a graph file records which fork rev wrote
  it, so this pin is the one coupling the file cannot tell you about.

**Always baseline on the pristine fixture first.** If the untouched fixture
does not validate, the oracle is skewed and no verdict about Holon's bytes
means anything. At `fab27740` it reports 213 entities / 2609 datoms / `Valid!`.

## The scripts are no longer shipped

`deps/db/script/validate_db.cljs` was deleted in `ef96a8d0` (2025-12-12,
"remove db scripts made redundant by CLI") and `diff_graphs.cljs` in
`f28e001b` (2026-02-18). LogSeq now validates through a compiled CLI whose
build needs OCaml/opam, the Clojure CLI and shadow-cljs.

The namespaces those scripts drove — `logseq.db.frontend.validate`,
`logseq.db.frontend.malli-schema`, `logseq.db.common.sqlite-cli`,
`logseq.db.sqlite.export` — are all still present and nbb-runnable at 65.33.
So the setup below restores LogSeq's own two scripts from its own history and
runs them against 65.33 source. The validation logic, the malli schema and the
non-migrating open path are the current ones; only the CLI shim is older. It is
not the shipped binary, and a verdict from it should be quoted as such.

`sqlite-cli/open-db!` does **not** migrate (there is no `db-migrate` under
`deps/`), which is exactly what makes it a clean oracle: it judges the bytes
Holon wrote instead of silently upgrading them first.

## Setup

Needs `node`, plus `pnpm` and `babashka` — `nbb-logseq` shells out to `bb` to
build its dependency jar. Neither has to be installed globally.

```sh
ORACLE=/path/to/logseq-oracle          # a checkout or git worktree
git -C /path/to/logseq fetch upstream  # logseq/logseq, not a fork
git -C /path/to/logseq worktree add "$ORACLE" fab27740975dcda1e93dbca718d1f620eda543c7 --detach

# LogSeq's own scripts, restored from the commits that deleted them
git -C "$ORACLE" show ef96a8d0^:deps/db/script/validate_db.cljs > "$ORACLE/deps/db/script/validate_db.cljs"
git -C "$ORACLE" show f28e001b^:deps/db/script/diff_graphs.cljs  > "$ORACLE/deps/db/script/diff_graphs.cljs"

# Holon's own scripts. apply_edits.cljs drives the head-to-head legs and the
# bisect; without it those tests cannot run at all. The probes back the
# built-in predicate's pins.
cp crates/holon-logseq-db/oracle/*.cljs "$ORACLE/deps/db/script/"

pnpm --dir "$ORACLE/deps/db" install --frozen-lockfile --ignore-workspace

# probe_built_in.cljs reaches into deps/outliner and deps/graph-parser, which
# carry their own node_modules — without these two the run dies with
# "Could not find namespace: logseq.clj-fractional-indexing"
pnpm --dir "$ORACLE/deps/graph-parser" install --ignore-workspace
pnpm --dir "$ORACLE/deps/outliner"     install --ignore-workspace
```

Then run the legs:

```sh
HOLON_LOGSEQ_ORACLE="$ORACLE" just lsqdb-oracle
```

Expect the whole crate green with 0 ignored. `Oracle::find` asserts that
`node_modules` exists before anything runs, so a graph-less setup fails with
the reason rather than passing quietly. It does NOT check that the individual
scripts are present — a missing script surfaces as the nbb invocation failing
inside whichever leg needed it.

## Naming a graph

`->open-db-args` treats an **absolute** path as the db file itself and a
relative one as a graph directory to resolve under `db.sqlite`. The test passes
absolute paths to `<dir>/db.sqlite`; by hand, from `deps/db`, either works:

```sh
./node_modules/.bin/nbb-logseq script/validate_db.cljs /abs/path/db.sqlite --closed-maps --group-errors
./node_modules/.bin/nbb-logseq script/diff_graphs.cljs /abs/a/db.sqlite /abs/b/db.sqlite -T
```

`probe_built_in.cljs` needs more: it loads namespaces from sibling `deps/`, so
it takes an explicit classpath AND `NODE_PATH`, without which nbb resolves its
own dependencies against the wrong `node_modules`. From `deps/db`:

```sh
NODE_PATH=$(pwd)/node_modules ./node_modules/.bin/nbb-logseq \
  -cp src:../outliner/src:../graph-parser/src:../common/src \
  script/probe_built_in.cljs /abs/path/db.sqlite /abs/out/built-in-entities.json
```

`probe_tail_builtin.cljs` and `probe_mirror.cljs` take the same invocation and
need no second argument; they print LogSeq's verdict on a built-in marker that
lives only in the unflushed tail, and on one retracted there. Those two answers
are what `is_built_in` is written to, and both were measured after a
tree-only predicate got them wrong in the direction that writes.

The second argument writes the recording that
`crates/holon-logseq-db/tests/fixtures/logseq-db/built-in-entities.json` holds.
Regenerate it only when LogSeq's version moves — the pin exists to make such a
move visible, so a silently-refreshed recording defeats it. A script placed
outside `deps/db/script/` fails with a namespace-resolution error even with the
classpath set; copy it in rather than pointing at it in place.

`--closed-maps` rejects unknown attributes rather than tolerating them, and
`-T` keeps timestamps in the diff — without it a write that dropped every
`:block/updated-at` still reports "equal".

## When LogSeq's version moves

Re-pin deliberately: find the newest rev whose `schema.cljs` version matches
the graphs Holon writes, redo the worktree, and re-baseline on the pristine
fixture before trusting any verdict. A minor bump is not cosmetic — most
shipped 65.x migrations rewrite user datoms.
