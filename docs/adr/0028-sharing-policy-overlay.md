# ADR 0028 — Sharing as Policy Overlay over Aligned Containers

**Status:** Accepted 2026-07-20 (Martin), following adversarial senior review
(ENDORSE-WITH-AMENDMENTS; review artifact: orchestrator session 2026-07-20,
`sharing-synthesis-fable-review.md`).
**Amends:** ADR 0003 (all-in-LoroTree) — formally reopened for the multi-container
shape; casualties named in §7.
**Supersedes (direction):** the extract-prune-mount `share_subtree` mechanism as
the long-term sharing model (it remains in the tree until replaced; near-term
fixes to it must be forward-compatible with this ADR).

## 1. Problem

"Share my entire vault with my phone" and "share this subtree with a colleague"
were served by one destructive mechanism: extract the subtree into its own
LoroDoc, prune the source, leave a mount node. Root-share is therefore
structurally impossible (empirically: silent UI collapse, dogfood 2026-07-20),
live share sync is dead (`fork_at` on shallow-snapshot docs is unimplemented
inside loro 1.11.1), and sharing degrades the owner's own vault (mounts, scars).

## 2. First principles (ratified)

- **Replication-set membership is policy; structure is content. Sharing never
  mutates structure.**
- **Device sync ≠ sharing.** Your own devices are you: same identity, full
  trust, no revocation semantics. Third-party shares carry partial trust.
- Highest-level frame: decentralized information-flow control where policy
  labels derive from a **mutable tree**, over **monotone** replication.
  Three kernels: K1 intensional policy over mutable structure; K2 no un-send
  (revocation is forward-only); K3 filtered causal histories (escapes:
  self-contained containers / leaky placeholders / identity-severing re-encode).

## 3. Decision

A **policy overlay**: a share is an owner-signed policy object
`{selector: stable block id, principals, capabilities, delegation flag, lease}`.
The replication quantum stays the **LoroDoc container**; a background
**boundary-alignment** process migrates blocks between containers **when policy
changes** (rare), so containers always partition content by audience. The
user-visible tree spans containers seamlessly (stitching layer); no mounts on
the owner side.

### Ratified sub-decisions

- **D1 — crossings are deliberate.** Outdent on a *direct page child* becomes a
  forbidden op (today it silently re-parents out of the page). Boundary
  crossings happen via drag-and-drop only; crossings that add a **new
  audience** require explicit confirmation (Drive-style). Device-sync (self)
  never prompts.
- **D2 — metadata-tight boundaries.** Crossings re-encode as
  delete-in-shared-container + create-in-private-container. No placeholders,
  no existence/activity leakage. A concurrent edit stranded by a crossing is
  rejected **loudly** on the editor's side *and returned as a keepable
  divergent copy*.
- **D3 — revocation is forward-only + disclosed best-effort destruction.**
  UI at share time and revoke time states the recipient retains history as of
  revocation; a best-effort destruction request is sent and labeled best-effort
  (legal leverage lives in contracts, not cryptography).
- **D4 — owner-signed policy; per-share delegation flag** ("invitees may
  invite"). Non-owner grants without delegation become pending requests.
  Membership is **lease-based**: grants expire and renew; revocation =
  non-renewal (H8).

### Amendments from review (all ratified)

- **A7 — v1 forbids nested/overlapping third-party shares.** Nesting fragments
  content into audience-equivalence-class containers and makes ordinary edits
  per-op crossings, destroying the "alignment is rare" economics. (Verdict was
  conditional on this cut.) Device sync (everything-policy) does not count as
  overlap: see C1.
- **H2 — owner-scoped totally-ordered crossing log** (Lamport clock + device
  tiebreak) arbitrates boundary crossings AND policy edits in one primitive;
  losers get the D2 loud-reject + keepable copy. Resolves K1 including the
  owner's own offline devices.
- **H3 — the alignment invariant is directional**: *effective audience never
  over-approximates policy at any observable point.* Widening: create-in-shared
  first. Narrowing: delete-from-shared first (transient invisibility is
  disclosed). Migration is journaled and idempotent/resumable.
- **H4 — narrowing rotates containers.** Baseline: re-encode + owner-signed
  succession pointer (no loro-internals dependency). Planned upgrade behind the
  same succession interface: shallow-fork-at-a-frontier-after-the-deletions,
  contingent on the loro fork/upstream investment (§6). Identity continuity for
  the owner via an **owner-private alias ledger** (recipients see fresh ids —
  metadata-tight preserved; owner's history stitches).
- **H5/C1' — enrollment ceremony precedes the phone fast path.** With sync-path
  overshare made unrepresentable, device enrollment auth is the attack surface;
  current bearer-ticket + `DefaultHasher` peer-id code is insufficient.
  Security-executor work, tightened with PBTs (Martin 2026-07-20).
- **H6 — RESOLVED BY VERIFICATION, not design**: page identity is minted once
  (`PageId::for_path` has creation-time call sites only) and never re-derived;
  rename is an ordinary edit. Selectors bind stable block ids and survive
  renames by construction. Residual: *bare* `[[Label]]` links dangle after
  rename — alias-track concern (see alias option A), not sharing-critical.
- **H7 — recipient side**: accepted shares attach under a dedicated
  **"Shared with me" root** (also the correct home for the 2026-07-20 orphaned
  mount bug N3).

## 4. Non-negotiable conditions

- **C1 — everything-policy fast path.** Device sync compiles to unfiltered
  replicate-all of the container registry: no policy evaluation in the
  self-device hot path; the overshare-bug class is unrepresentable there.
  (Option 1 is thereby a degenerate mode of this design.)
- **C2 — directional alignment invariant as keystone oracle, red-first**, in
  the H3 form (never over-approximate audience), before share code lands.
- **C3 — fail-closed boundary classification.** Every structural op declares
  its boundary behavior; unclassified ops cannot ship (precedent: mandatory
  `MenuExposure`).
- **C4 — two-peer offline move-storm keystone, red-first**, gating the device
  sync fast path itself. Includes undo-across-a-crossing-then-merge cases.
  Sharpest justification: prod has cycle prevention + detection but **no repair
  path** — LoroTree's convergent merge is the only safety net, and device sync
  makes concurrent moves routine. Ruling: "test the hell out of this."

## 5. Substrate (S6 ruling)

**Keep LoroTree** (it is already the substrate — single global tree, ADR 0003;
fractional-index ordering; native cyclic-move rejection). Per-container trees +
our stitching layer replace the single global tree. LoroTree moves never span
containers (crossings are delete+create pairs bracketed by the H2 log), so
doc-scoped TreeIDs are harmless. Maps-LWW would require reimplementing
convergent moves badly. Tonight's share breakage lives in loro's *fork*
machinery, which this design removes from the critical path.

## 6. Loro fork/upstream decision (W2)

`fork_at` on shallow-snapshot docs is a loro-internal gap (loro 1.11.1,
crates.io). Ruling: check upstream issues/PRs/releases first — if unfixed,
**fork and contribute upstream** (cleaner than a private fork; precedent:
turso). The same investment reopens live share sync
(`loro_share_backend.rs:362`) and upgrades H4 rotation. Research artifact:
`loro-fork-shallow-research.md` (orchestrator session 2026-07-20).

## 7. Costs accepted (eyes open)

- **Vault-wide undo dies at container boundaries** (Loro undo is per-doc).
  Ruling: within-container undo is the 90% case; get cross-container undo to
  the best feasible state via inverse-crossings through the H2 log (C4 tests
  it). ADR 0003's unified-undo benefit is knowingly narrowed.
- Per-container base/epoch multiplication under invariant 10, bounded by A7.
- A permanent verification tax: C3 classification on every future structural
  op; alignment machinery correctness guarded by C2 forever.

## 8. Explicitly rejected

- Extract-prune-mount as the long-term model (destructive; empirically broken).
- Root-share via mounts (mount inversion collapses the vault).
- Placeholder-based boundaries (metadata leakage: existence/counts/rhythms).
- Symmetric CRDT-merged policy (security by tiebreak).
- Retroactive-removal promises of any kind.
