# Adversarial verification — lane `hygiene`

Verifier workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/hygiene`
Base asserted: `jj log -r @- ... commit_id.short()` = `ed38a4dae833` (matches).
Toolchain in-lane: `nightly-2026-08-16-aarch64-apple-darwin` (overridden by the
lane's own `rust-toolchain.toml`) — `lane-logs/verify/toolchain.log`.
No jj/git write commands were run. All probe edits were cp-aside + write-back
with sha256 proof; `jj status` after every probe shows the same 6 lane files and
nothing else. My own scripts/logs live under `lane-logs/verify/` (gitignored),
so the change under test is unpolluted.

---

## Claim 1 — Tree identity — CONFIRMED (the brief's probe is ill-formed)

The literal probe fails, but not because of the tree:

```
$ grep -n "query_positional" $(grep -rl "fn consume" crates/holon/src/core/pantry_operations.rs)
exit=1
$ grep -n "fn consume" crates/holon/src/core/pantry_operations.rs   -> rc=1 (no match)
```

`pantry_operations.rs` has no `fn consume`; the operation is dispatched inside
`execute_operation` (`op_name != "consume"` at line 125). So the inner
`grep -rl` prints nothing, the outer grep reads a closed stdin and exits 1.
This is a defect in the probe, not `wrong-tree`.

Substantively the tree IS right:
`crates/holon/src/core/pantry_operations.rs:42` —
`.query_positional(&sql, vec![turso::Value::Text(id.to_string())])`.

## Claim 2 — Pantry `consume` parameterization is an idiom refactor — CONFIRMED

**Both statements bind.** `read_stock` (line 39/42):
`SELECT quantity, unit FROM {TABLE} WHERE id = ?` via `query_positional`.
`execute_operation` (line 172-180):
`UPDATE {TABLE} SET quantity = ? WHERE id = ?` via
`execute(&sql, vec![Value::Real(remaining), Value::Text(id.clone())])`.
`DbHandle::query_positional` (`crates/holon-turso/src/turso.rs:808`) and
`DbHandle::execute` (`:840`) both take positional binds on the same actor path.

**No user value is interpolated into SQL any more.** The only `format!`
substitutions left in the two SQL strings are the `TABLE` const. The remaining
`{id}` occurrences (lines 44, 48, 52, 127, 152, 164, 182) are all error-message
text, not SQL.

**The old escaping was correct, so this closed no hole — independently
reproduced.** I restored the base `pantry_operations.rs` (hand-rolled
`id.replace('\'', "''")`) while keeping the lane's NEW test, and ran it:

- `lane-logs/verify/pantry-base.log:68`
  `PASS [ 1.126s] (2/8) holon::kitchen_cookable_now_e2e consume_carries_sql_metacharacters_in_the_item_id`
- `lane-logs/verify/pantry-base.log:76`
  `Summary [ 11.139s] 8 tests run: 8 passed (2 leaky), 0 skipped`

Restore proof: sha256 before and after the probe both
`2d82aae88e5328ab0219e1a8b03172c58ad11a922d2d8d733389d7777157ef82`
(`lane-logs/verify/pantry-sha-before.txt`, `pantry-sha-after.txt`).

The `'`/`;`/`--` test exists (`crates/holon/tests/kitchen_cookable_now_e2e.rs`,
id `p-o'brien';drop--table`) and passes against the lane's code:
`lane-logs/verify/pantry-new.log:76` PASS, `:83`
`Summary [ 11.203s] 8 tests run: 8 passed, 0 skipped`.

The lane report already states this premise failure plainly (Item 1, "The
brief's premise does NOT hold"). Its self-assessment is accurate.

**Probed and clean:** `Value::Real(remaining)` vs the old bare `{remaining}`
numeric literal is not an affinity change — `pantry_item.yaml:18-19` declares
`quantity` as `sql_type: REAL`.

## Claim 3 — `notify_watcher_delivers_events_after_arm` oracle — CONFIRMED, non-vacuous

Read of `crates/holon-filesystem/src/change_source.rs:864-894`: `subscribe()`
happens before `arm()`, the dir is a fresh empty `tempfile::tempdir()`, and
`a.org` is written 100 ms AFTER arming. No `a.org` event can therefore be
produced by the arm or pre-exist in the channel — the only possible source is
the write. `Path::ends_with` matches whole components, so it is not a substring
match either. **Not vacuous by construction.**

**Not vacuous empirically.** With the oracle's target changed to a file that is
never written (`zzz-never-written.org`), the test goes RED, consuming the full
budget: `lane-logs/verify/p2.log` —
`Summary [ 5.139s] 1 test run: 0 passed, 1 failed, 92 skipped`, panic at
`change_source.rs:892:10` (the `expect("timed out waiting for an fs event for a.org")`).

**10 isolated runs, one script, one log per run** — `lane-logs/verify/n-1.log`
.. `n-10.log`, each:
`Summary [ ~0.13s] 1 test run: 1 passed, 92 skipped` → **10 passed, 0 failed**.

**Full crate suite once** — `lane-logs/verify/fs-suite.log:163`:
`Summary [ 3.251s] 93 tests run: 93 passed (1 leaky), 0 skipped`.

`grep -cE "^error:|^error\[|usage:"` = 0 on all eleven logs.

### DEFECT 3a (minor, non-blocking) — the trailing `assert!` is dead

```rust
if is_target { return seen; }   // loop only returns on a match
...
assert!(seen.last().unwrap().ends_with("a.org"), "seen: {seen:?}");
```
The closure returns only when the last pushed path already ends with `a.org`,
so this assertion can never fail and its `seen: {seen:?}` message can never
print. The real oracle is the `.expect(...)` on the timeout — which is exactly
where probe 2 panicked (`:892`), never at `:893`. The teeth are real; the
assert contributes none.

### DEFECT 3b — the bugfunnel entry's base measurement is not reproducible

The entry (and the lane report) state base behaviour as **"0 passed, 10
failed"** and conclude "every isolated failure lands in ~0.12s". I restored the
exact base file (`jj file show -r @-`, `assert!(change.path.ends_with("a.org"))`
present) and ran it 5× isolated:

- `lane-logs/verify/p1-1.log` `0 passed, 1 failed` (0.128s)
- `lane-logs/verify/p1-2.log` `0 passed, 1 failed` (0.120s)
- `lane-logs/verify/p1-3.log` **`1 passed`** (0.132s)
- `lane-logs/verify/p1-4.log` `0 passed, 1 failed` (0.119s)
- `lane-logs/verify/p1-5.log` **`1 passed`** (0.133s)

→ **2 of 5 PASSED at base.** The old oracle is a genuine race, not the
deterministic 10/10 failure the entry records. The root cause (event ordering)
is unaffected and the fix is still correct; but the entry's `## Bug` section
presents a single sample as a determinate fact, which is what makes the
"contradictory prior measurements (one session 10/10 passing, another 0/5)" it
complains about happen in the first place. Restore proof: sha256
`9278a383547dba8d9f4991e29584ee143a2d2f7f4cc0b21b68b02e504d36f027` before and
after (`lane-logs/verify/sha-before.txt`, `sha-after.txt`).

## Claim 4 — Bugfunnel entry — CONFIRMED on schema, QUALIFIED on triage

`docs/Testing/bugfunnel/entries/2026-09-01-notify-watcher-arm-first-event-oracle.md`
matches the template in `.claude/skills/bug-gap-triage/SKILL.md`: front matter
`id` (= filename stem), `date`, `gap: ORACLE`, `secondary: null`,
`status: FIXED`, `summary: >-` one sentence; sections `## Bug`, `## Root cause`
(with `change_source.rs:880` citation and log paths), `## Missing piece`,
`## Remedy`.

Validator, run by me:

```
$ /usr/bin/python3 scripts/bugfunnel.py check
585 entries, 0 problems
```
(`lane-logs/verify/bugfunnel.log`.)

### DEFECT 4a — the ORACLE classification inverts the gap's definition

The skill defines ORACLE as "The PBT can generate the interaction, but **no
invariant would have flagged the defect**" — a *missing* oracle that let a
production defect escape. Here there was no production defect at all: the
watcher was correct in every run (the entry says so), and the test's own
assertion raised a **false alarm**. That is the opposite failure mode, and the
funnel's stated purpose ("the distribution of escapes decides where test
investment goes", ~21 prod escapes in the 2026-07 baseline) is skewed by
counting a bad-test-assertion as an escape. `bugfunnel.py check` validates
field syntax only, so it cannot catch this.

### DEFECT 4b — skill procedure step 3 not addressed

The skill's step 3 requires an explicit keystone-repro attempt/note; the entry
has none. Defensible here (nothing in the keystone touches this unit test), but
it is unstated rather than dismissed.

## Claim 5 — Known-reds census rows — CONFIRMED on numbers, REFUTED on mechanism

**Every number traces to a log I read.** All twenty `Summary` lines:

- `subtree-share-tmp-leftover` 9/10: FAIL `lane-logs/flakes/subtree-1.log`
  `Summary [ 43.621s] 1 test run: 0 passed, 1 failed, 7 skipped`; PASS ×9
  `subtree-2.log` (309.393s), `-3` (106.232s), `-4` (107.125s), `-5` (148.756s),
  `-6` (154.688s), `-7` (122.858s), `-8` (136.918s), `-9` (178.844s),
  `-10` (166.141s), each `1 test run: 1 passed (1 slow), 7 skipped`. The doc's
  "43s–309s, the run that failed is also the shortest" is exactly right.
- `loro-backend-change-count` 10/10 pass: `lane-logs/flakes/loro-1.log` ..
  `loro-10.log`, each `1 test run: 1 passed (1 slow), 5 skipped`, 43.225s–66.223s
  — matches the doc's "43s–66s".

**The `subtree-share-tmp-leftover` row DOES state the isolated rate**: its
Evidence column reads "2026-09-01, 10 isolated runs: **9 passed, 1 failed**",
plus the explicit "One failure in ten is one sample; do not read it as a 10%
rate." It is not recorded as "load-only".

### DEFECT 5a (blocking-ish) — the row's own disclaimer is FALSE, and it silently arms the nightly classifier

The new section states:

> `scripts/keystone-known-reds.sh` classifies composed-nightly logs only, so it
> does not consume these rows automatically — they are a manually-maintained
> baseline

The script parses the registry with a section-blind `awk -F'|' '/^\| *`/'`
(`scripts/keystone-known-reds.sh:50-55`) and takes **every** row whose status is
`known-red`, wherever it sits in the file. Measured:

```
base  docs/Testing/KeystoneKnownReds.md -> 25 known-red keys
lane  docs/Testing/KeystoneKnownReds.md -> 27 known-red keys
added: subtree-share-tmp-leftover, loro-backend-change-count
```

End-to-end proof, running the real script on the lane's own captured failure:

```
$ bash scripts/keystone-known-reds.sh <copy of lane-logs/flakes/subtree-1.log>
PRIMARY: [known-red:subtree-share-tmp-leftover] ... P-NO-TMP-LEFTOVER/B: stale tmp files: [...]
WARN known-red [subtree-share-tmp-leftover] x1
```

So the row does NOT behave as a passive manual baseline: from now on a
composed-nightly `P-NO-TMP-LEFTOVER/B: stale tmp files` panic is auto-demoted to
a WARN pass-with-note. For a signature the lane's own report calls "a real
atomic-publish race worth its own lane" (Open question 2), that is masking a
live bug, and the doc tells the next reader it cannot happen.

The same applies to `loro-backend-change-count`, where auto-classification is
presumably intended — but its Match pattern is registered on a signature that
never fired in 10 runs and was transcribed from an assertion format string, so
the pattern itself is unverified against any real payload.

## Claim 6 — Redaction residuals module doc — REFUTED (dangling label scheme)

Wording quality is fine: `crates/holon-mcp-client/tests/rest_transport_redaction.rs:9-26`
is present-tense current-state, no dates, no "was previously", and each of the
four items carries at most two reasons (N1: byte-match / decoding would redact
a different string; N2: indistinguishable / would blank prose; (f): convention
places it in the path / erasing the authority costs the diagnostic;
(g): length floor keeps `Error!` readable / cannot tell word from token).

### DEFECT 6a — two incompatible label schemes, one of them unresolvable

The list is labelled `N1`, `N2`, `(f)`, `(g)`. There is no `(a)`–`(e)` anywhere
in the tree:

```
$ grep -rn "host position" . --exclude-dir=lane-logs --exclude-dir=target --exclude-dir=.jj --exclude-dir=.git
lane-report-hygiene.md:118:...**(f)** token in host position, **(g)** benign
crates/holon-mcp-client/tests/rest_transport_redaction.rs:20://! - **(f) A token in host position.** ...
```

The only other hit is the lane's own report restating the same letters. `(f)`
and `(g)` are ordinals inherited from an enumeration that exists only in a
prior lane's prompt or notes — precisely the "cite by position into a list that
is not in the tree" failure the bug-gap-triage skill warns about ("Do NOT cite
it by position: ... those numbers are not recoverable"). A reader cannot resolve
what `(a)`–`(e)` were, and cannot tell whether `N1`/`N2` and `(f)`/`(g)` are one
list or two.

## Claim 7 — Gates — CONFIRMED

All run by me from script files under the `holon-build` semaphore, foreground,
tree asserted first (`test -f` + `grep -q query_positional` +
`grep -q "Deliberately not covered"`); scripts `lane-logs/verify/vA.sh`,
`vB.sh`, `vProbe.sh`.

| Gate | Log | Summary |
|---|---|---|
| `cargo fmt --all --check` | `lane-logs/verify/fmt.log` | 0 bytes = clean |
| `cargo nextest run -p holon-filesystem` | `lane-logs/verify/fs-suite.log:163` | `Summary [ 3.251s] 93 tests run: 93 passed (1 leaky), 0 skipped` |
| notify test ×10 isolated | `lane-logs/verify/n-1.log` .. `n-10.log` | each `1 test run: 1 passed, 92 skipped` |
| `cargo nextest run -p holon --features test-helpers --test kitchen_cookable_now_e2e` | `lane-logs/verify/pantry-new.log:83` | `Summary [ 11.203s] 8 tests run: 8 passed, 0 skipped` |
| `cargo check -p holon-app` | `lane-logs/verify/check-holon-app.log` | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 6.46s` |
| `/usr/bin/python3 scripts/bugfunnel.py check` | `lane-logs/verify/bugfunnel.log` | `585 entries, 0 problems` |

Non-zero test counts everywhere (93, 8, 1×10).
`grep -cE "^error:|^error\[|usage:"` = 0 on fmt, fs-suite, pantry-new,
check-holon-app and all ten notify logs.

None of the lane's four registered pre-existing reds fired in any run of mine.

---

# Overall verdict: REFUTED (two documentation defects), code CONFIRMED

Every **code** claim survives independent reproduction: the pantry statements
bind, no user value is interpolated, the metacharacter test exists and passes,
the notify oracle is non-vacuous and green 10/10 plus a clean 93/93 suite, and
all five gates are green from my own runs. The pantry change is confirmed to be
an idiom refactor and not a security fix — I reproduced the new test passing
against the base escaping.

Two **documentation** claims fail:

1. **DEFECT 5a (fix before landing).** `docs/Testing/KeystoneKnownReds.md`'s new
   section asserts `scripts/keystone-known-reds.sh` does not consume its rows.
   It does — measured 25→27 known-red keys, and the real script classifies the
   lane's own `subtree-1.log` failure as `known-red:subtree-share-tmp-leftover`,
   auto-demoting a suspected real atomic-publish race to a WARN.
2. **DEFECT 6a.** The redaction module doc mixes `N1`/`N2` with `(f)`/`(g)`,
   the latter pointing at an `(a)`–`(e)` enumeration that exists nowhere in the
   repository.

Plus three lesser findings: the dead trailing `assert!` in the notify test
(3a), the bugfunnel entry's non-reproducible "0 passed, 10 failed" base
measurement — I got 2 of 5 passing on the same file (3b), and the ORACLE gap
classification inverting the skill's definition for what was a false-alarm
assertion rather than an escaped defect (4a).

I fixed nothing. Routing is the orchestrator's.

---

# Re-verification — 2026-09-01, change `wrqxvmor` at its NEW state (`a81a5653`)

Same workspace, same rules. Base still `ed38a4dae833`. No jj/git writes; no
probe edits were needed this round. Scripts `lane-logs/verify/vC.sh`, tree
asserted first (`grep -q "no fs event for a.org within the budget"` +
`grep -q "Adding a row here ARMS the nightly classifier"`).

Delta since the first pass: `docs/Testing/KeystoneKnownReds.md` rewritten
(27→26 lines of table, one row dropped), a new bugfunnel entry
`2026-09-01-subtree-share-tmp-leftover-race`, the notify entry corrected
(`gap:` ORACLE→ENVIRONMENT + a new preamble section), the redaction labels
stripped, and the notify test's assertion moved onto the timeout result.

## Re-claim 1 — classifier no longer arms on the share race — CONFIRMED

**Key count 26, not 27.** Extracting known-red keys with the script's own
parser (`awk -F'|' '/^\| *\`/'` then `status == "known-red"`):

```
base  docs/Testing/KeystoneKnownReds.md -> 25 keys
lane  docs/Testing/KeystoneKnownReds.md -> 26 keys
diff: 25a26 > loro-backend-change-count     (only ONE key added)
```

`subtree-share-tmp-leftover` is gone from the registry.

**The classifier now treats the failure as novel.** Running the real script on
the lane's own `lane-logs/flakes/subtree-1.log`:

```
PRIMARY: [novel] fake2.log @ crates/holon/tests/sync_suite/sync_pbt.rs:803:29:
    P-NO-TMP-LEFTOVER/B: stale tmp files: [".../shares/<uuid>.loro.tmp"]
...
[known-reds] FAIL: 3 novel panic(s), 0 known-red panic(s), 0 collateral (ignored).
```

Contrast with the first pass, where the same log produced
`PRIMARY: [known-red:subtree-share-tmp-leftover]` and
`WARN known-red [subtree-share-tmp-leftover] x1`. DEFECT 5a is closed, and the
section now carries the mechanic as a standing warning ("**Adding a row here
ARMS the nightly classifier.** `scripts/keystone-known-reds.sh:50-55` parses
this file with a section-blind `awk` …"), which is a stronger outcome than
merely deleting the false sentence.

## Re-claim 2 — bugfunnel entries — CONFIRMED

```
$ /usr/bin/python3 scripts/bugfunnel.py check
586 entries, 0 problems
```
(`lane-logs/verify/bugfunnel2.log`; 585 → 586 = the one new entry.)

```
$ /usr/bin/python3 scripts/bugfunnel.py list --gap ENVIRONMENT --status OPEN
2026-09-01 ENVIRONMENT OPEN 2026-09-01-subtree-share-tmp-leftover-race — …
```

Schema of the new entry matches the skill template: `id` = filename stem,
`date`, `gap: ENVIRONMENT`, `secondary: null`, `status: OPEN`, one-sentence
`summary`, sections `## Bug` / `## Root cause` / `## Missing piece` /
`## Remedy`, plus a `## Keystone repro` section (the skill's step 3, which
DEFECT 4b said was unstated — now addressed in BOTH entries).

**Every number traces to a log I read:**

| Number in the entry | Log |
|---|---|
| `Summary [ 43.621s] 1 test run: 0 passed, 1 failed, 7 skipped` | `lane-logs/flakes/subtree-1.log` |
| 9 passing, each `1 test run: 1 passed (1 slow), 7 skipped` | `lane-logs/flakes/subtree-2.log` .. `subtree-10.log` |
| wall-time swing 43s–309s, failure is the shortest | same ten (43.621 / 309.393 / 106.232 / 107.125 / 148.756 / 154.688 / 122.858 / 136.918 / 178.844 / 166.141) |
| decoded payload + `minimal failing input` + `successes: 7` | `lane-logs/flakes/subtree-1.log:81` |

The corrected notify entry's new evidence table also checks out:
`lane-logs/notify/base2-1.log` .. `base2-10.log` each read
`1 test run: 0 passed, 1 failed, 92 skipped` (0.120–0.125s) — a second 0/10
block, exactly as tabulated — and it cites my own `lane-logs/verify/p1-1.log`
.. `p1-5.log` (2 passes) and `p2.log` correctly. DEFECT 3b is closed: the entry
now states 25 isolated runs / 2 passes and says plainly that "always fails" was
wrong. DEFECT 4a is closed too — the entry opens by naming that no gap fits, why
ENVIRONMENT is the least-wrong forced choice, and that it should be excluded
from the funnel distribution.

## Re-claim 3 — redaction notes have no dangling labels — CONFIRMED

`crates/holon-mcp-client/tests/rest_transport_redaction.rs:9-25`: the four
bullets now open with their own subject (**A `%21`-percent-encoded `!`
marker.** / **An all-lowercase-alpha piece.** / **A token in host position.** /
**Benign path words of `MIN_SECRET_LEN` (8) bytes or more are
over-redacted**). No `N1`, `N2`, `(f)`, `(g)` remain:

```
$ grep -n "N1 \|N2 \|(f)\|(g)" crates/holon-mcp-client/tests/rest_transport_redaction.rs
(no matches)
```

Wording is still present-tense current-state with at most two reasons each.
DEFECT 6a closed.

## Re-claim 4 — live assertion on the timeout result — CONFIRMED

`crates/holon-filesystem/src/change_source.rs:865-896`:

```rust
let mut seen = Vec::new();
let arrived = tokio::time::timeout(std::time::Duration::from_secs(5), async {
    loop {
        let change = rx.recv().await.expect("channel closed");
        let is_target = change.path.ends_with("a.org");
        seen.push(change.path);
        if is_target { return; }
    }
})
.await;
assert!(
    arrived.is_ok(),
    "no fs event for a.org within the budget; seen: {seen:?}"
);
```

`seen` was hoisted out of the closure so the failure message can name every
path observed, the `.expect("timed out …")` is gone, and the single live
assertion is now the one on `arrived`. The tautological
`assert!(seen.last().unwrap().ends_with("a.org"))` of the previous state is
removed — DEFECT 3a closed. This is strictly better than the first state: the
diagnostic paths now print on the failing path rather than on the impossible one.

**10× isolated, one invocation and one log per run** — `lane-logs/verify/m-1.log`
.. `m-10.log`, each `Summary [ ~0.13s] 1 test run: 1 passed, 92 skipped` →
**10 passed, 0 failed**.

**Full crate suite once** — `lane-logs/verify/fs-suite2.log:160`:
`Summary [ 3.248s] 93 tests run: 93 passed, 0 skipped`.

`grep -cE "^error:|^error\[|usage:"` = 0 on the suite log and all ten run logs.

---

## Re-verification verdict: CONFIRMED

All four re-claims hold against evidence I produced in this session. Both
blocking defects from the first pass are closed, and all three lesser findings
(3a dead assert, 3b non-reproducible base measurement, 4a mis-fit gap) are
closed as well — 4b (missing keystone-repro note) is closed in both entries.
I found no new defect in the delta. Nothing was fixed by me.
