---
id: 2026-10-08-create-page-from-link-bypasses-the-loro-write-authority
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Under Loro authority, a click on a dangling wiki-link created its page only in
  SQL: no Loro node, the unminted sort_key "A0", and no org file.
---

## Bug
Found by agent exploration (recreate lane, 2026-10-08) while probing page
recreation at freed paths. `block.create_page_from_link("Linked later")` with no
earlier history gives a page that `inv-blocks-match-ref/loro` sees only in the
reference, `inv-birth-contract-satisfied` sees with sort_key `A0`, and
`inv-every-page-has-its-own-file` sees as fileless.

## Root cause
The op was served by the bare `SqlOperationProvider` that
`crates/holon-app/src/turso_seams.rs` allowlists under Loro authority. Its
`create` constituents called the SQL provider's own `execute_operation_with_origin`,
so the write authority (`LoroBlockOperations`) never saw them.

## Missing piece
Only `CreatePageAtFreedPath` drives the op in the keystone, and under Loro its
freed paths always collide with the renamed page's derived id, so the op is
refused before it writes. No generated sequence reached a successful create.

## Remedy
FIXED. `create_page_from_link` is an engine-level compound
(`run_create_page_from_link`, crates/holon/src/api/operation_engine.rs), like
`convert_block_to_page`: the read-only `page_chain_plan` plans the chain, each
missing page is a `create` dispatched to the write authority, and
`heal_page_links` re-resolves the `block_links` rows. The reference
(`apply_create_page_at_path`, crates/holon-integration-tests/src/pbt/reference_state.rs)
records the minted page's own file `<name chain>.org`.

Hand-authored row `a-page-created-from-a-dangling-link-lives-in-the-write-authority`:
red before the fix (lane-logs/recreate/red1-red.log), green after
(lane-logs/recreate/red1-green.log). Teeth: the base engine, SQL provider, seam
allowlist and shape-gate sim turn it red on `inv-birth-contract-satisfied` and
`inv-every-page-has-its-own-file` (lane-logs/recreate/red1-teeth.log).

A click at a path a rename vacated now creates a new page beside the renamed one
(`holon_api::page_slot`, PageIdentityDeterminism.md §5.3), so
`CreatePageAtFreedPath` in the random keystone reaches a successful create; the
reference mirrors the same function and the driver no longer tolerates
`IdentityCollision`.

## Known limits
- No rollback mid-chain, as in `run_convert_block_to_page`: a failure while
  creating segment k leaves segments 1..k-1 created, and a failing
  `heal_page_links` leaves the chain created with no history entry. The error
  names the failed segment but not the pages already created.
