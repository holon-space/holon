---
id: 2026-09-02-sharing-a-page-drops-its-page-tag-and-it-vanishes-from-both-sidebars
date: 2026-09-02
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  The mount block that replaces a shared subtree carries no tags, so a shared
  page loses its `Page` tag and disappears from the sidebar on the sharer and
  never appears on the accepter.
---

## Bug

Found by the `double-dogfood` lane on 2026-09-02, macOS desktop paired with the
Android app over a real Iroh share.

Sharing a page makes it disappear from the UI on **both** peers. The content is
still in the database and still syncs; it is simply unreachable by navigation.

Reproduction:

1. Create `block:dd-trip` "Trip planning" under `block:__default__`, `add_tag`
   it `Page`, give it three children. It appears in the left sidebar.
2. `share_subtree` it with `retention: "none"`.
3. Look at the sidebar on the sharer. The page is gone.
4. `accept_shared_subtree` on the other peer. The page never appears there
   either.

Measured after the exchange, on both sides:

```
=== MAC  === Trip planning, block:8b0ee6d6-…, tags []
=== DROID === Trip planning, block:ed458917-…, tags []
```

Both sidebars render only `Journals`, `2026-09-02` and `Integrations`. The
shared page is in neither, while `execute_raw_sql` finds the content on both.

For the accepter this is the whole feature failing in the only place a user
looks: accept a share, see nothing, with no error to explain it. For the sharer
it reads as data loss — the page was there a moment ago.

## Root cause

The sidebar is driven by the query at `block:left_sidebar::src::0`:

```sql
SELECT b.* FROM block b JOIN block_tags bt ON bt.block_id = b.id
WHERE bt.tag = 'Page' AND b.id != 'block:__default__' ORDER BY b.content ASC
```

Membership is `block_tags`, nothing else.

`share_subtree` replaces the shared subtree with a freshly minted mount node
(`crates/holon-loro/src/loro_share_backend.rs`, `create_mount_node` /
`commit_share_prune`). The mount carries the three metadata keys the share
machinery needs — `mount_kind`, `shared_tree_id`, `shared_root` — and copies the
content string, but it does not carry the original block's tags. So the row that
takes the page's place in the tree is not a `Page` and the sidebar query cannot
see it. `accept_shared_subtree` mints its own mount the same way, with the same
omission, which is why the accepter never sees it either.

The share code already knows this class of problem exists: an amendment in
`share_subtree` bubbles the mount up to the nearest page ancestor so "the mount
is a Page (Inc 2), so it must sit under a Page". The mount's *placement* was made
page-aware; its *identity* as a page was not.

Related, same run, same cause: the mount is a new id, so the page's stable id
does not survive sharing. `block:dd-trip` no longer exists in SQL afterwards
while the document alias still points `block:dd-trip` at
`vault/__default__/Trip planning.org`. Any `[[link]]` to a page would dangle the
moment it is shared.

## Missing piece

**ENVIRONMENT.** Sharing is tested at the storage layer, where the assertion is
that the blocks arrive. Nothing drives share and accept through a rendering
frontend, so "the blocks arrived" and "the user can reach them" were never the
same check, and only the first was ever made.

**ORACLE.** There is no invariant that the set of navigable pages is preserved
by sharing. Something of the form "a block that was a `Page` before an operation
is still reachable as a `Page` after it" would have gone red here, and would
also have caught the dangling-alias half.

## Remedy

Not fixed here; this lane is exploratory.

The fix is to carry the shared root's tags onto the mount node on both the share
and the accept path, so a shared page stays a page. The narrow version copies
`Page`; the honest version copies the tag set, since any tag-driven query has
the same blind spot, not just the sidebar.

Red-first ordering: add the reachability invariant, watch it go red on the
share, then carry the tags, then re-run the two-instance dogfood.

Worth deciding at the same time: whether the mount should reuse the shared
root's stable id instead of minting a new one. That would fix the dangling
alias and the vanishing page together, and would remove a whole class of
"the id changed underneath me" problems. It is a bigger change than tag-copying
and it interacts with the id-collision guard described in
`docs/Reference/SUBTREE_SHARING.md`, so it is a design call rather than a patch.

See also
[2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer](2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer.md)
from the same session.
