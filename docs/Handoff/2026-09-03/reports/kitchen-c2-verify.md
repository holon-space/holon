# Adversarial verification — lane kitchen-c2

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/kitchen-c2`
Change under test: jj `@` = `611294c43214` (uncommitted WIP), base `@-` = `ed38a4dae833` (= `main` = `main@origin`).
All evidence below was produced in this session. Logs under `lane-logs/V.*.log`.

**Overall verdict: REFUTED on two claims (3 and 4), CONFIRMED on the rest.**
Neither refutation is a build break — the lane's gates are genuinely green. Claim 3 is an
overstated security escalation; claim 4 is a real, reproducible data defect in `sync_once`.

---

## Claim 1 — tree identity: CONFIRMED

`grep -n "pickedItems" crates/holon-kitchen/src/shopping.rs` → 11 hits (lines 11, 12, 216, 231,
269, 271, 285, 287, 383, 463, 522). `jj log -r @-` → `ed38a4dae833`. Not `wrong-tree`.

## Claim 2 — "token is STABLE per list; no refresh machinery exists": CONFIRMED with a caveat

**Code — confirmed.** `grep -rniE "rotat|mint_|refresh_token|refresh\("` over
`crates/holon-kitchen/src`, `crates/holon-kitchen/tests`,
`crates/holon-app/src/shopping_operations.rs`, `assets/integrations/shopping.yaml` → **zero hits**.
No mint/rotate/refresh path exists for the shopping token. (`rest_transport.rs` has an OAuth2
`force_refresh`, which is pre-existing and unrelated to this credential.)

**Docs — the primary statements are corrected**, in both files:
`docs/Plans/ThatShoppingList-API-2026-09-01.md:18-21`, `docs/Plans/Kitchen.md:59` (P36d),
`:197`, `:267` (R10 dissolved).

**Caveat — three residual, unqualified "rotating" claims survive in the tracked tree**, and one
of them is now factually wrong:
- `docs/Plans/Kitchen.md:191` — "the ROTATING, never-registered `!<token>` … **(the shape the
  production peer actually uses)**". That parenthetical directly contradicts P36d. It is the
  exact sentence a future agent copies.
- `docs/Plans/Kitchen.md:288` — staleness-guard row titled "P26 layer 2 (rotating token)".
- Two live test names still assert the retired premise:
  `crates/holon-mcp-client/tests/rest_transport_redaction.rs:496` and `:512`
  (`a_rotating_unregistered_bearer_segment_…`), plus the anchor name cited at `Kitchen.md:62`.

`Kitchen.md:62` *is* properly qualified ("With P36d corrected (the token is stable per list…)"),
so this is incomplete cleanup, not a missed correction.

## Claim 3 — "Live credentials are still in git history": REFUTED

I enumerated **every** commit touching `docs/Plans/ThatShoppingList-API-2026-09-01.md` across all
refs (`git -C <primary repo> log --all -- <file>`, 115 commits — jj keep-refs included), and for
each one classified origin-reachability with
`git merge-base --is-ancestor <c> refs/heads/main` and counted distinct `![A-Za-z0-9_-]{8,}`
segments in that version of the file.

| Set | Commits | Distinct token-shaped segments |
|---|---|---|
| **ORIGIN-REACHABLE** (ancestors of `main` = `main@origin` = `ed38a4dae8`) | **1** | **0** |
| local-only (jj keep-refs / abandoned revs, never pushed) | 114 | 47 of them carry **2**; the other 67 carry **0** |

The file was **created** in `ed38a4dae8` (C1) already scrubbed — `git log main -- <file>` returns
exactly that one commit, and `git show main:<file> | grep -coE '![A-Za-z0-9_-]{8,}'` → **0**.
The current WIP version (`611294c432`) is also **0**.

**Why this refutes the claim as written.** The lane report states (§1, §7.3) that the tokens are
"**still live credentials in git history**" and that "**scrubbing that history**, or re-sharing the
list … is the only thing that closes it". Nothing token-shaped is reachable from `main@origin`,
so there is nothing to scrub in shared history and no history rewrite is warranted. The true
exposure is materially smaller and differently shaped: the 2 token-shaped segments exist **only
in local, unpushed jj operation-log/keep refs on this machine**. That is a local-disk exposure
(`jj util gc` territory), not a published one. Escalating it to Martin as published-history
leakage would misdirect a rewrite of shared history.

*(The 2 segments were counted, never read or reproduced.)*

## Claim 4 — "commit, then re-pull and re-reconcile" replaces the version signal: REFUTED

### 4a. There is no generator. The mock "PBT" is a hand-authored example suite.

`crates/holon-kitchen/tests/shopping_pull_mock.rs` contains **no `proptest!`, no generator, no
strategy** — it is 13 `#[tokio::test]` examples over a hand-written mock. The single staged
interleaving is `Mode::StaleFirstCommit` (`:73-76`, `:217-221`): the peer rejects commit #1,
applies nothing, bumps the version and inserts a foreign item. That write does land *between*
commit and re-pull, so one concurrent interleaving is covered — but the interleaving space is
enumerated by hand, not generated, and the report's framing as a PBT overstates coverage.

### 4b. Counterexample: `sync_once` reports SUCCESS while duplicating an item on the peer.

I wrote a scratch test (`crates/holon-kitchen/tests/zzz_verify_probe.rs`, **since deleted**)
implementing `ShoppingPeer` / `ShoppingRowReader` directly and driving `sync_once`.

**Setup.** One local row `("Milk","R")` with `last_seen_remote: None` (a pending Add). Peer starts
empty at version 1. The peer applies commits faithfully. **The only perturbation:** the *verifying
re-pull* (pull #2) is served one write stale — a cached GET.

**Expected** (per `shopping_sync.rs:154-163`, and per `CommitCommand.id` being documented at
`shopping_sync.rs:49` as "the peer's idempotency and ordering key"): the peer ends with one `Milk`.

**Actual** (`lane-logs/V.probe.log`):
```
PROBE: peer items = [("Milk", "R"), ("Milk", "R")]
PROBE: command ids = ["1000_0", "1001_0"]
PROBE: verbs = ["Add", "Add"]
PROBE: committed=2 retried=true pulls=3
Summary [0.027s] 1 test run: 0 passed, 1 failed, 0 skipped
```
`sync_once` returned `Ok`. The user's list has a duplicate.

**Where it breaks.** `CommitBatch::from_push_intents` mints
`id: format!("{now_ms}_{seq}")` (`shopping_sync.rs:95`) and `sync_once` passes
`now_ms + attempt` (`shopping_sync.rs:202`). The retry therefore re-sends **the same logical
command under a different id**, so the field documented as the idempotency key cannot deduplicate
the one case it exists for. The re-pull rule at `:154-163` is sound only if the re-pull is
authoritative; when it is not, the rule converts a *false* "it did not land" into a **second
apply**, and the local parser folds duplicates by `ItemKey` (`shopping.rs:275-292`, `fold`) so
Holon can never observe the damage it caused. The failure is silent by construction.

### 4c. The stale re-pull is not a hypothetical — the shipped sidecar invites it.

`docs/Plans/Kitchen.md:59` records the authoritative captured request as
`GET /!<token>/api/list/<listId>?**oldVersion=&version=&_nocache=**`. The shipped call in
`assets/integrations/shopping.yaml:38-40` is:
```yaml
      list-items:
        method: GET
        path: /api/list/{listId}
```
— **no `_nocache`, no `version`, no `oldVersion`**. The read leg drops the very cache-buster the
captured contract carries, on the one request the whole retry rule depends on being fresh. That is
an independent defect: the implementation diverges from the plan's own authoritative read contract.

### 4d. Probes that did *not* refute (recorded for completeness)

- Concurrent peer **delete** between commit and re-pull of a pending Add: the row keeps
  `last_seen_remote: None`, so it re-pushes rather than being deleted — **no local Add is lost**
  (`shopping.rs:638-648`). It does silently resurrect the other party's delete; undocumented, minor.
- Local rows are read once (`shopping_sync.rs:173`) and re-reconciled against each new snapshot.
  Correct: no local write happens until `sync_once` returns.
- No interleaving found where an item is **deleted instead of checked** through this code path.

## Claim 5 — "a local check is never pushed; no un-check counterpart": CONFIRMED

- `CommandVerb` is exactly `{ Add, Del }` (`shopping_sync.rs:29-33`), `as_wire` → `"add"`/`"del"`
  (`:36-41`). `PushIntent` is exactly `{ Add, Remove }` (`shopping.rs:527-531`). No third command
  shape is representable, so no guessed check encoding can be emitted. My probe's mock panics on
  any unknown verb and only ever saw `Add`.
- `LocalIntent::Check { id }` (`shopping.rs:514-516`) has no un-check sibling.
- **What happens to a locally-checked item on the next pull:** `CompleteSnapshot::items()` yields
  the union of `items` and `pickedItems` (`shopping.rs:275-292`), so a checked item is still
  *present* and is never read as a deletion. The reconciler emits `Check` only on
  `remote.checked && !row.checked` (`shopping.rs:620-624`) and **never** clears. So a locally
  checked item stays checked locally forever and emits only `TouchLastSeenRemote`.
  **There is no ping-pong.** The honest cost is permanent one-way divergence: the peer shows the
  item unchecked forever and Holon never tells it otherwise. That matches the documented §4 rule.

## Claim 6 — one writer + redaction on the non-GET path: CONFIRMED

- **No second writer.** `grep -rn "SqlBlockOperations|execute_raw|BlockOperations|Connection\b"`
  over `crates/holon-kitchen/src` and `crates/holon-app/src/shopping_operations.rs` → **zero hits**.
  The apply leg is `OperationResult::declared_irreversible(...).with_follow_ups(follow_ups)`
  (`shopping_operations.rs:214-219`), and `local_intent_operation` (`shopping_sync.rs:222-256`)
  emits only generic `create` / `set_field` / `delete` against `SHOPPING_ITEM_ENTITY`.
- **Redaction on the non-GET path.** Every error the transport emits leaves through
  `RestCallSurface::err` → `safe` → `redactor.redact` (`rest_transport.rs:281-292`). This covers
  the write path specifically: `send_request` transport/read errors (`:447-461`), non-2xx **with
  the echoed body preview** (`:501-507`), the post-401-refresh failure incl. body preview
  (`:491-498`), non-JSON body incl. preview (`:344-351`), and `attach_response_version`'s
  non-object body which embeds the raw body `{other}` (`:398-403`) — all redacted.
  Re-ran: `lane-logs/V.redaction.log` → `Summary [ 0.195s] 10 tests run: 10 passed, 0 skipped`.
- **Minor defect, same file.** Two error strings carry a mangled line-wrap and print ~20 stray
  spaces mid-sentence: `rest_transport.rs:400` and `:409` (`"...response_version_path '{path}', but
  <20 spaces> {} answered..."`). `cargo fmt` does not touch string literals, so it passes clean.
- **Un-redacted (but secret-free) error sites**, for the record: `rest_transport.rs:297-301`
  (unknown call name), `:305`, `:315`, `:332-334` (placeholder/body-fill failures). These carry
  call names and tool arguments, never the base URL. No leak found.

## Claim 7 — gates, re-run independently: CONFIRMED

Script `lane-logs/verify-gate.sh` (asserts the tree, asserts the probe file is gone), run under
`sem --id holon-build -j4 --fg`; `rustup show active-toolchain` → `nightly-2026-08-16-aarch64-apple-darwin`
(from the lane's own `rust-toolchain.toml`).

| Gate | Log | Summary |
|---|---|---|
| `cargo fmt --all -- --check` | `lane-logs/V.fmt.log` | empty, 0 bytes (clean) |
| `cargo nextest run -p holon-kitchen -p holon-mcp-client` | `lane-logs/V.tests.log` | `Summary [   8.154s] 411 tests run: 411 passed, 0 skipped` |
| `cargo nextest run -p holon-mcp-client --test rest_transport_redaction` | `lane-logs/V.redaction.log` | `Summary [   0.195s] 10 tests run: 10 passed, 0 skipped` |
| `cargo check -p holon-gpui -p holon-app` | `lane-logs/V.check.log` | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1.62s` |
| `just keystone-smoke` | `lane-logs/V.keystone.log` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.28s` |

Test counts non-zero throughout. `grep -lE "^error:|usage:"` over all five logs → **none**.
No known pre-existing red was hit: the five `e2e_backend_engine_test` matview reds and the three
named flakes live outside these crates. The lane report's numbers reproduce exactly (411/411, 4/4).

## Claim 8 — docs describe current state; Kitchen.md still truthful: CONFIRMED, one caveat

- The C2 subsection (`docs/Plans/Kitchen.md:195-205`) is written in the present tense and
  describes state, not history. The one past-tense phrase — "the compiled-in aisle enum C1
  shipped is DELETED" — is a statement about the current tree, and the "Red-first" bullet is
  required by the project's own `holon-feature` convention.
- **"No timer" is true.** `grep -rn "interval|spawn|tokio::time|poll_interval"` over
  `shopping_sync.rs`, `shopping_rest.rs`, `shopping_operations.rs` → **zero hits**.
- **`shopping_sync` op is real and registered.** `shopping_operations.rs:149` (`name:
  "shopping_sync"`), dispatch guard at `:182`, registered into the shared provider at
  `mcp_integrations.rs:798-813`.
- **"nothing CALLS `shopping_sync` yet" is true.** The only non-test reference to
  `holon_kitchen::shopping_sync::sync_once` is `shopping_operations.rs:30` / `:204`. (The other
  `sync_once` hits in the tree are `holon-sharing`'s unrelated function of the same name.)
- **Caveat.** `Kitchen.md:195` reads "**LANDED 2026-09-01**" and `:30` reads "**CLOSED at C2**",
  but this change is uncommitted WIP on `@` and is not reachable from `main`. Consistent with the
  repo's convention of authoring the doc inside the landing commit, so I record it as a fact to be
  aware of rather than a defect — it is only true once the change lands.

---

## Cleanup proof

The scratch probe `crates/holon-kitchen/tests/zzz_verify_probe.rs` was **created and removed**;
`test ! -e` passes and it appears nowhere in `jj status`. No other file in the lane was modified.
No jj/git write command was run. Files I added are confined to `lane-logs/` and this report.

## Defects for the orchestrator to route (no remedies prescribed)

1. **[data loss, silent]** `sync_once` re-sends a retried command under a new id
   (`shopping_sync.rs:95` + `:202`), duplicating the item on the peer whenever the verifying
   re-pull is stale, and returns `Ok`. Reproduced.
2. **[contract divergence, raises the odds of 1]** `assets/integrations/shopping.yaml:38-40` drops
   the `_nocache` / `version` / `oldVersion` query parameters that `Kitchen.md:59` records as the
   authoritative read request.
3. **[report accuracy / security triage]** The "live credentials still in git history" escalation
   is not supported: 0 token-shaped segments are origin-reachable; the exposure is local-only.
4. **[doc rot]** Three unqualified "rotating token" claims survive — `Kitchen.md:191` (with a
   parenthetical that is now false), `Kitchen.md:288`, and two test names at
   `rest_transport_redaction.rs:496,512`.
5. **[cosmetic]** Mangled error strings at `rest_transport.rs:400` and `:409`.
6. **[coverage framing]** `shopping_pull_mock.rs` is an example suite, not a PBT; one concurrent
   interleaving is staged by hand.

---

# Re-verification — 2026-09-01 (delta §D1–D6)

Same workspace, same rules. Change `lsnlmpqn` at its new state: jj `@` = `9c5f...` WIP,
`@-` = `ed38a4dae833` unchanged. Diff is now 23 files / +3451 −517 (was 20 / +2487 −507); new
files `crates/holon-kitchen/tests/shopping_sync_pbt.rs` and the bugfunnel entry.
Evidence: `lane-logs/W.*.log`, produced in this session.

**Overall: all six delta claims CONFIRMED.** Both original refutations are genuinely closed.
Three residual gaps are recorded at the end; none blocks the delta.

## R1 — `command_id` derived from the command + one `round_ms`: CONFIRMED

`command_id(round_ms, verb, key)` (`shopping_sync.rs:129-134`) hashes `verb.as_wire()` and the
`ItemKey`, prefixed by `round_ms`; `sync_once` passes `now_ms` unchanged on every attempt
(`:225-226`, with the `now_ms + attempt` that caused the defect gone).

**The original counterexample, replayed** with the same fake peer — and this time a peer that
does **not** deduplicate on the id, so a genuine re-send is visible (`lane-logs/W.probe2.log`):
```
P1a: applied=[("Milk", "R")] ids=["1000_b44b53b862a5b918"] pulls=3 commits=1 committed=1 retried=false
```
**One Add at the peer, one id, one commit.** Previously: two Adds, ids `1000_0` / `1001_0`.
The defect is closed, and it is closed by the freshness floor rather than only made harmless —
`commits=1` shows the second commit never happened at all.

**Two identical logical Adds in one round — NOT wrongly collapsed.** Two local rows with distinct
row ids but the same `(name, cat)` are refused before any commit is built:
```
P1b: Err = two local shopping rows share the key ItemKey { name: "Milk", cat: "R" }; the (name, cat)
key is the row identity, so a duplicate means the table was written past the reconciler
```
(`shopping.rs:570-577`.) That is the correct outcome, not a collapse: `(name, cat)` **is** row
identity on this peer (P36 / `shopping.rs:184-188` — the phone API itself folds duplicate names in
one category), so "the user added Milk twice" is one row with `count: 2`, never two rows. Two Add
intents sharing a key are therefore unreachable, and the only other way to collide would be an Add
and a Remove on one key — mutually exclusive branches (`shopping.rs:580-650`), and the verb is
hashed anyway. **No wrong collapse exists.**

*Note, not a defect:* `command_id` uses `DefaultHasher`, whose output is not guaranteed stable
across Rust releases. Harmless here — ids only need to agree within one round, i.e. one process.

## R2 — `pull_at_least` on a peer that NEVER serves a fresh snapshot: CONFIRMED, terminates loudly

`pull_at_least` (`shopping_sync.rs:250-267`) is a bounded `for _ in 0..3` with an
`anyhow::bail!` after it — structurally incapable of spinning. Exercised against a peer frozen
one write behind forever, under a 10s `tokio::time::timeout` that would have caught a spin:
```
P2: err = shopping sync: after a commit the peer answered version 2, but 3 reads still returned
version 1; the list this round would decide against is older than a write it just made, and acting
on it would re-send that write
P2: pulls=4 commits=1 applied=[("Milk", "R")]
```
Exactly 1 initial pull + 3 bounded re-reads, **one** commit, **one** applied command, then a loud
`Err`. It does not spin, and it does not re-commit against the stale read.

## R3 — sidecar GET carries `oldVersion`/`version`/`_nocache`: CONFIRMED (names); values unverifiable from the cited source

`assets/integrations/shopping.yaml:47-56` declares all three on `list-items`, and the values are
supplied generically from `shopping_rest.rs:87-94`: `oldVersion` and `version` = the newest version
this peer has observed (`last_version`, `fetch_max`), `_nocache` = `epoch_ms()` — a **fresh value
per request**, so it genuinely busts a cache, which is the property `pull_at_least` depends on.

**Correction to the claim as put to me:** `Kitchen.md:59` records the request as
`GET /!<token>/api/list/<listId>?oldVersion=&version=&_nocache=` — the values are **elided** in
the capture. So the parameter *names* match exactly and on the right call, but the doc specifies
no values and cannot confirm or deny them. I verified what I could; I cannot certify "the values
are correct per Kitchen.md:59" from that source.

*Named risk, bounded — not a defect.* `observe()` is called on the commit ack
(`shopping_rest.rs:134`), so the verifying re-pull asks `oldVersion=<the version it is waiting
for>`. If the peer treats `oldVersion` as a delta cursor (semantics unobserved — D6 item 2), that
read could answer a partial body. The blast radius is limited by the type gate: a response missing
`items` fails loudly in `CompleteSnapshot::from_response` (`required_array`), so the only bad shape
is an explicit `items: []`, which would read as mass deletion. Worth an entry when `mode` /
`oldVersion` semantics are finally captured.

## R4 — `shopping_sync_pbt.rs` is a real proptest state machine: CONFIRMED

**Generator** (`shopping_sync_pbt.rs:65-77`): a weighted `prop_oneof!` over six step kinds —
`PeerAdd`(2) `PeerDel`(1) `PeerCheck`(1) `LocalAdd`(2) `LocalDelete`(1) and `Sync`(3) — drawn into
`proptest::collection::vec(step(), 1..14)` (`:292`). Both perturbations are **generated, not
staged**: `Sync { stale_repull: any::<bool>(), concurrent_write: option::of(item()) }` (`:72-74`).

**Both are really wired into the window that matters.** In `MockPeer::commit`
(`:164-217`): `pre_commit` is snapshotted *before* the commands apply; the concurrent write is
injected *after* they apply and *before* the version bump (`:200-205`) — i.e. squarely between
commit and re-pull; and `pending_stale = Some(pre_commit)` (`:208-211`) is what the next `pull`
serves (`:155-162`). So the exact interleaving that produced my original counterexample is inside
the generated space, independently combinable with a concurrent writer.

The mock **deliberately permits duplicate `(name, cat)` entries** (`:118` `duplicate_keys`) — it
does not hide the damage — while honouring the documented id key (`applied_ids`, `:189`).

**Run at 1000 cases** (`lane-logs/W.pbt1000.log`, `PROPTEST_CASES=1000`; the config reads that env
var at `:283-286`, default 256):
```
Summary [   0.422s] 3 tests run: 3 passed, 0 skipped
```
(the PBT plus the two focused defence tests in the same binary). Timing is consistent with the
lane's 4000-case run — ~2500 cases/s in both — so the env var is genuinely honoured, not ignored.

*Coverage limit, stated:* the mock's `pending_stale` is a `take()`, so a generated `stale_repull`
serves **one** stale read. Two or three consecutive stale reads — the `MAX_PULL_ATTEMPTS = 3`
boundary — are never generated; that boundary is pinned only by the hand-written
`a_permanently_stale_read_fails_the_round_instead_of_re_committing` (`:487`), which is exactly
what my R2 probe re-derives independently.

## R5 — D62.a share-link base URL via a generic empty-path rule: CONFIRMED, with a test gap

**No shopping-specific Rust parsing exists.** `grep -rn "listId|list_id|split.*'!'|token"` over
`crates/holon-kitchen/src/shopping_rest.rs` and `crates/holon-app/src/shopping_operations.rs` →
**zero hits**. Nothing splits the token or the list id out of the URL; the sidecar declares
`path: ""` and `path: /commit` (`shopping.yaml:51`, `:57`) and the `listId` operation parameter is
gone.

**The engine change is genuinely generic** — five lines in `do_call`
(`rest_transport.rs:306-315`): an empty relative path resolves to the trimmed base URL instead of
appending a bare separator. No mention of shopping, no special case.

**Other sidecars still behave:** `cargo nextest run -p holon-mcp-client --test rest_transport_mock`
→ `Summary [   0.999s] 10 tests run: 10 passed, 0 skipped` (`lane-logs/W.restmock.log`). Every
non-empty path is unaffected.

**Gap:** `grep -rn 'path: ""'` across `crates` and `assets` → **exactly one hit**, the shopping
sidecar. There is **no test in `holon-mcp-client`** — the crate that owns the rule — exercising an
empty path at all. The 10 green mock tests are a regression check on the *unchanged* branch; the
new branch is pinned only indirectly, through the shopping sidecar's HTTP sibling. An engine change
justified by "any connector benefits" is untested for any connector but one.

## R6 — gate list re-run independently: CONFIRMED

Script `lane-logs/verify2-gate.sh` (asserts the tree, asserts the scratch probe is gone), under
`sem --id holon-build -j4 --fg`. Toolchain `nightly-2026-08-16-aarch64-apple-darwin`.

| Gate | Log | Summary |
|---|---|---|
| `cargo fmt --all -- --check` | `lane-logs/W.fmt.log` | empty, 0 bytes (clean) |
| `cargo nextest run -p holon-kitchen -p holon-mcp-client` | `lane-logs/W.tests.log` | `Summary [   7.861s] 414 tests run: 414 passed, 0 skipped` |
| `cargo check -p holon-gpui -p holon-app` | `lane-logs/W.check.log` | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1.53s` |
| `just keystone-smoke` | `lane-logs/W.keystone.log` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.63s` |
| `--test shopping_sync_pbt` @ `PROPTEST_CASES=1000` | `lane-logs/W.pbt1000.log` | `Summary [   0.422s] 3 tests run: 3 passed, 0 skipped` |
| `--test rest_transport_mock` | `lane-logs/W.restmock.log` | `Summary [   0.999s] 10 tests run: 10 passed, 0 skipped` |
| `/usr/bin/python3 scripts/bugfunnel.py check` | `lane-logs/W.bugfunnel.log` | `585 entries, 0 problems` |

414 tests, up from the 411 I measured before — the sync PBT plus its two focused defence tests, as
the delta states. `grep -lE "^error:|usage:"` over all four gate logs → **none**. No known
pre-existing red was reached. **I did not observe the "1 leaky"** the delta reports: my run says
`414 passed, 0 skipped` with no leak flagged, consistent with it being a flaky handle-detection
artefact rather than a property of the change.

## Residual findings (none blocking)

1. **[test gap]** The generic empty-path rule (`rest_transport.rs:306-315`) has no test in its
   owning crate; `path: ""` occurs once repo-wide.
2. **[coverage limit]** The PBT generates at most one consecutive stale read, so
   `MAX_PULL_ATTEMPTS = 3` is exercised only by a hand-written test.
3. **[unverifiable-as-claimed]** `Kitchen.md:59` elides the query values, so value correctness
   cannot be certified against it; and the verifying re-pull sends `oldVersion` = the version it is
   waiting for, whose peer semantics are still unobserved (bounded by the type gate to the
   `items: []` case).
4. **[carried over, unfixed by choice]** Mangled error strings at `rest_transport.rs:400`/`:409`;
   the two `a_rotating_…` test names retained deliberately (D4). Both agreed.
5. **[housekeeping]** `kitchen-c2-verify.md` is now inside the change's diff. It is a verification
   artefact, not lane content — worth deciding whether it should land.

## Cleanup proof

Scratch probe `crates/holon-kitchen/tests/zzz_verify_probe2.rs` created and removed; `test ! -e`
passes and nothing matching `zzz` appears in `jj status`. No lane file was modified. No jj or git
write command was run. My additions are confined to `lane-logs/` and this report.
