# Verify: lane `admit-fuzz` — CONFIRMED

Independent fresh-context verification. Every result below was produced in this
session; nothing is quoted from the lane's own logs.

## 1. Tree identity
`pwd` = the lane WS. `jj diff --stat` = **one file**,
`crates/holon-sharing/tests/admit_hostile_envelope_pbt.rs`, 778 insertions.
No production source is modified. Final sha256, matching the lane's claim:
```
2633682dc2e80d92fcffd563978e379580db8c3e54a3480b1da1c356668dde99  crates/holon-sharing/src/acceptor.rs
ec54b0a4ae936789426f159e8b99b5a96e53aef47f9fb7774bdaa8824896c1cf  crates/holon-sharing/src/lease.rs
```

## 2. Cold run reproduced (`lane-logs/verify-a-*.log`)
No `proptest-regressions` directory exists for `holon-sharing`, so the run was
cold by construction. Toolchain `nightly-2026-08-16` from the WS `rust-toolchain.toml`.
```
=== COLD 512 ===   Summary [0.777s] 8 tests run: 8 passed, 0 skipped
=== CI=true rep === Summary [2.149s] 8 tests run: 8 passed, 0 skipped
=== reach ===      Summary [2.124s] 1 test run: 1 passed, 7 skipped
=== -p holon-sharing === Summary [1.562s] 75 tests run: 75 passed, 0 skipped
```

## 3. Oracle independence — MOSTLY INDEPENDENT, one shared surface named
`model_admits` does **not** call `admit`, `parse_chain`, `verify_membership`,
`blob_canonical_bytes` or `MembershipCert::signature_valid`. It re-derives:
- `model_blob_bytes` — its own `blake3::Hasher` field sequence, not the SUT's
  `blob_canonical_bytes`. A field dropped from the production tuple diverges.
- `model_cert_signature_holds` — its own `serde_json` cert-body tuple + blake3,
  not the SUT verifier.
- `required_capability` — duplicated in the test (line 328), not imported.

**Shared functions (the honest caveat):** the model calls the SUT crate's own
`Capabilities::intersect`, `Capabilities::contains` and `Capabilities::is_subset_of`,
and decodes the chain with the same `serde_json::from_slice::<Vec<MembershipCert>>`.
A defect in the capability set algebra, or in `MembershipCert`'s serde impl, would
be mirrored on both sides and is invisible to this PBT. The decision *predicate* is
independent; the capability *lattice* underneath it is not. This does not weaken
any of the six claimed properties, but it bounds what "no defect found" covers.

## 4. Teeth — my own two guards, my own inversion points
I deliberately avoided the lane's first-shown inversion (I1, the `parse_chain`
panic) and used different inversion points than the lane for both guards.

| Guard | My inversion (not the lane's) | Result |
|---|---|---|
| P2 container binding | `if selector.0 != env.container.0` → `if selector.0 != selector.0` (lane deleted the block) | **RED**: `FAIL a_proof_for_one_container_is_refused_on_another_containers_log`, `T1_EXIT=100` |
| P4 capability requirement | `Ok(capabilities) if !capabilities.contains(required)` → `… && false` (lane inverted `required_capability` instead) | **RED**: `FAIL a_third_partys_chain_writes_into_my_replica_only_with_write`, `T2_EXIT=100` |

Each was restored by `cp` from a pristine copy taken before any edit; sha256 after
each restore is the baseline value above. Re-run after restore:
`Summary [0.975s] 8 tests run: 8 passed, 0 skipped`.
Evidence: `lane-logs/verify-b-*.log`.

## 5. Reach — no generator hole
`the_corpus_reaches_every_decision_variant` asserts `imports >= 40 && sig > 0 &&
malformed > 0 && audience > 0 && container > 0 && lease > 0 && capability > 0`,
i.e. every one of the seven `AdmitDecision` variants. Measured by me over 4000 draws:
```
[1.225, 30.15, 8.2, 21.975, 13.2, 22.375, 2.875] percent
```
All seven non-zero. `Import` is the thinnest at 1.225% (49 draws) and
`RefuseCapability` at 2.875% (115) — thin but real, and the four output-directed
baseline properties cover the import path directly. No variant at 0.

## 6. My own hostile shapes (scratch test, since deleted)
A self-contained scratch test at `crates/holon-sharing/tests/vscratch_hostile.rs`,
run and then removed (`lane-logs/verify-c-*.log`). All four refuse-or-import,
none panicked:
- duplicated cert → `RefuseLease{BrokenLink{index:1, expected:"peer-a"}}`
- terminal cert naming a different subject (owner→peer-a→stranger, receiver peer-a)
  → `Import{Read,Write}` — **correct**, not a defect: subject != receiver, so
  `required = Write`, and the chain confers Write. Matches the model.
- lease expired by 1 s → `RefuseLease{LeaseInactive{index:0}}`
- 100 KB of random bytes as the chain → `RefuseMalformedProof`

## Verdict: **CONFIRMED**
Every claim checked against evidence produced in this session. No defect found in
`admit`. The lane's three security review findings (unkeyed verifier, unauthenticated
`BlobSig` replay, unchecked `epoch`/`kind`/`seq`/`head`) are design facts outside
what this layer can falsify and are correctly reported as such — they remain the
real risk, not the PBT.
