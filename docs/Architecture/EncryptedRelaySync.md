# Encrypted Relay Sync — design note (PARKED)

Status: design discussion captured 2026-07-20 (Martin + orchestrator session).
Not scheduled. Resurface when ADR 0028's crossing log + lease machinery land.
Vault pointer: `holon-pkm/Projects/Holon/Encrypted Sync Relay.org`
(slug `encrypted-relay-resurface`).

## Problem

Devices should sync without being online simultaneously. That wants an
always-online central instance — but the vault must not exist in cleartext
outside the owner's devices. Question: is E2E encryption compatible with CRDT
sync?

## Verdict: yes, directly — if the server never merges

CRDT merge is commutative, associative, and idempotent, so sync tolerates
arbitrary delay, reordering, and duplication. That is exactly the property
that makes a **blind relay** sufficient: a durable store-and-forward ciphertext
mailbox. Devices export Loro update blobs, encrypt client-side with a
per-container key, and push opaque bytes; any device later pulls "all blobs
since my cursor", decrypts, imports (Loro import is idempotent; arrival order
is irrelevant). The only operation that needs plaintext is merge, and merge
only ever happens on enrolled devices.

The relay is therefore **not a Holon instance** and must never be an enrolled
device in the ADR 0028 H5/C1' sense (the everything-policy fast path would
hand it the vault in cleartext). It is a third kind of participant: dumb,
durable, blind, outside the trust boundary.

## Relay contract (sketch)

- One **encrypted append-only log per container**; server assigns sequence
  numbers (CRDT semantics make server ordering harmless).
- Clients hold **cursors**; "give me everything since cursor N" is the whole
  read API. No server-side version-vector computation needed.
- Every blob **signed/MAC'd with the container key** → relay cannot inject.
- **Compaction is client-side**: a device periodically uploads a re-encrypted
  consolidated snapshot + truncation point, authorized by an **owner-signed
  compaction record** (same authority shape as D4 policy objects). Loro
  shallow snapshots (the W2 `fork_at`-on-shallow work) are the natural
  primitive: compacted state without full history.

## What is given up

| Capability | Status | Workaround |
|---|---|---|
| Server-side merge/compaction | gone by construction | client-side compaction + owner-signed truncation |
| Smart delta serving (server computes what a client lacks) | gone if VVs are encrypted | append log + client cursors; slightly more transfer |
| Thin clients / server-side query | gone | every reader is a keyed device (already Holon's model) |
| Metadata privacy | partial | relay still sees blob sizes/timing/count and container topology — activity patterns, not content. Padding/batching mitigates; disclose honestly |

## Threats encryption does not cover

- **Injection**: covered by blob signing (above).
- **Withholding / fork attacks**: a malicious relay can serve device A and
  device B different log suffixes, or freeze a device on a stale view. CRDTs
  make this survivable (any later direct/indirect contact heals), but
  *detection* needs hash-chained logs or signed heads plus occasional
  device-to-device gossip as cross-check. Reference: Kleppmann's
  Byzantine-fault-tolerant CRDT work.

## Rejected middle ground

Encrypting op payloads while leaving causal metadata (op ids, dependency
edges) cleartext would let the server dedup/order/GC — but for a tree CRDT the
op graph's shape approximates the outline structure. That trade leaks too much
for Holon; fully-blind with client-side compaction is the chosen point.

## Why ADR 0028 makes this nearly free

- **Per-container docs (S6)** → the encryption unit. One key + one relay log
  per container.
- **H4 narrowing = re-encode into a fresh container** → that *is* key
  rotation; structurally the MLS epoch model. Revocation composes: revoked
  peer never receives the next epoch's key.
- **D4 leases + owner-signed policy** → key distribution: a valid lease is
  what entitles a peer to the current container key; revocation =
  non-renewal.
- **H5 enrollment ceremony (W4 lane)** → the key-exchange bootstrap.
- **iroh** (already in-tree) has blob-store + gossip layers; a self-hosted
  blind relay is a small service, not a platform.

## Prior art

- Ink & Switch **Keyhive/Beehive** — E2EE + capability-based access control
  for Automerge; closest existing system to this design.
- **MLS** (Messaging Layer Security) — group key agreement + epoch rotation;
  the pattern H4/D4 already mirror.
- **Kleppmann, "Making CRDTs Byzantine fault tolerant"** — the
  withholding/fork detection story.

## Open questions (decide at resurface time)

1. Relay discovery/transport: iroh-native vs plain HTTPS blob API.
2. Per-container key derivation: independent random keys vs derived from an
   owner root key (recovery story vs blast radius).
3. Fork detection cadence: signed heads on every push vs periodic
   device-gossip audit.
4. Whether the everything-policy device-sync fast path (C1) and the relay
   share one code path (relay = "device that stores but cannot read") or stay
   distinct — leaning distinct: blurring the enrolled/blind line is exactly
   the confusion H5 exists to prevent.

## Groundwork already banked

- `relay-seam-inventory` scout report (session 2026-07-20, job 00b6f50c):
  inventory of every Loro byte-boundary site + the narrowest encrypt/decrypt
  seam(s), plus the full-reseed fallback-trigger classification that governs
  relay economics (blob-log relays are only efficient when sync is
  incremental-dominant).
