---
id: 2026-09-02-a-capability-is-checked-against-a-sender-chosen-selector
date: 2026-09-02
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `admit` verified membership over the sender-chosen `auth.selector` and never
  bound it to the container the envelope rode on, so a peer holding a genuine
  owner-issued read-write cert for ONE container had its delta admitted on the
  log of every other container the victim replicates.
---

## Bug

Found by adversarial verification of lane `pair-inc5` (own-device pair,
Increment 5), by a fresh-context verifier reading the acceptor while checking
that lane's `Capability::Write` enforcement. Not in the lane's residual list,
not in the plan, and not recorded anywhere else.

A capability is granted OVER an object. The acceptor checked the capability
against an object the SENDER named, then imported the payload into a different
object entirely — the container the envelope was pulled from. Nothing tied the
two.

Reproduced against an unmodified copy of the lane tree:

```
CONTAINER-SCOPE DECISION container=private-journal selector=holon_tree
    -> Import { capabilities: Capabilities({Read, Write}) }
    PASS a_write_cert_for_one_container_is_admitted_on_another_containers_log
```

This needs no forgery, no tampering and no relay access. An honest-but-greedy
peer, legitimately shared into one container and holding a valid owner-issued
`[Read, Write]` cert for it, reaches every other container the victim
replicates. Transport-level peer authentication does **not** mitigate it, which
is what separates it from
`2026-09-02-blobsig-is-unkeyed-so-a-forged-sender-can-replay-a-chain`.

Severity: high. It predates the lane — the old code verified over the same
sender-chosen selector — but per-container scoping is the entire point of the
share model, and a capability scoped to the wrong object is not enforcement.

## Root cause

`admit` (`crates/holon-sharing/src/acceptor.rs`) built the selector straight
from the wire and passed it to `verify_membership`:

```rust
let selector = BlockId(env.auth.selector.clone());
...
match verify_membership(&chain, &subject, &selector, ctx.clock, ctx.verifier) {
```

`verify_membership` (`crates/holon-sharing/src/lease.rs:248`) checks only
`cert.selector == selector` — that the cert matches the string the sender
supplied. Meanwhile `pull_once` (`crates/holon-sharing/src/sync.rs`) pulled the
envelope from log L and imported the payload into container L. `env.container`
and `env.auth.selector` were never compared.

## Missing piece

The two-instance slice replicates exactly ONE container. `replication_set`
(`crates/holon-loro/src/container_registry.rs:127`) returns the root container
plus registered extras, and the only caller of `register_container` outside the
registry's own tests is a unit test in `registry_binding.rs`. So no generated
sequence can reach a state with two containers, let alone one where a cert for
the first is presented on the second. That is the COVERAGE gap, and it is why
this survived a boundary that was otherwise well unit-tested.

Secondarily ORACLE: no invariant states that an admitted envelope's capability
was proved over the container it lands in. Adding a second container to the
slice without that invariant would still not have caught it.

## Keystone repro

The keystone `general_e2e_composed_pbt` cannot reproduce it — single instance,
no `SyncTransport`. Closing the generation gap properly means registering a
second container in the two-instance slice and drawing envelopes across the
pair, which is a slice increment of its own. The defect itself is pinned by two
unit tests at the boundary (below).

## Remedy

FIXED in lane `pair-inc5`. The selector and the container are now required to be
equal, checked ONCE in `admit` before the chain is parsed; a mismatch returns
the typed `AdmitDecision::RefuseContainer { container, selector }`, which lands
in `SyncReport.refusals` like every other refusal.

Red-first, `cargo nextest run -p holon-sharing --lib`
(`lane-logs/red-container-scope-2026-09-02.log`):

```
FAIL acceptor::tests::a_write_cert_for_one_container_is_refused_on_another_containers_log
   left: Import { capabilities: Capabilities({Read, Write}) }
  right: RefuseContainer { container: ContainerLogId("private-journal"), selector: BlockId("holon_tree") }
Summary: 19 tests run: 18 passed, 1 failed
```

Green after the binding: 19 of 19. Pinned by
`a_write_cert_for_one_container_is_refused_on_another_containers_log` and its
positive twin `a_cert_for_the_containers_own_selector_is_admitted`, so the
binding cannot pass by refusing everything.

Stronger form left open: drop `MembershipProof::selector` from the wire and
derive the selector from `env.container`, making disagreement unrepresentable
rather than checked. Deferred because it changes the wire type, and container
ids and cert selectors are only known to coincide for the shapes present today
(`ROOT_LOG_ID` == `ROOT_CONTAINER_ID` == `holon_tree`). Worth doing when subtree
shares gain a production registration path.
