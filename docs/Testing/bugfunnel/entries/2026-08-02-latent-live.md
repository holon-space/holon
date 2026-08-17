---
id: 2026-08-02-latent-live
date: 2026-08-02
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  LATENT, not live
source_line: 784
---

## Bug

Image-path traversal escapes the vault: `resolve_image_path`
(`crates/holon-filesystem/src/file_sync_controller.rs:4627-4655`) has THREE
defects, all verified by reading the guard. (a) The assert is
`resolved.starts_with(&canonical_root) \ | \ |
joined.starts_with(&self.root_dir)` — `Path::starts_with` compares whole
COMPONENTS and does not normalize `..`, so for `relative_path =
"../evil.png"` the value `joined = <root>/../evil.png` literally begins with
the component sequence of `<root>` and the SECOND disjunct is
unconditionally TRUE for every relative input. The disjunction therefore
makes the first (correct, canonicalized) disjunct dead and the whole assert
a no-op for relative traversal. (b) The function returns `joined`, the
UN-normalized path, discarding the `resolved` value it just computed — so
even a correct check would be validating a different path than the one it
hands back. (c) `self.fs.canonicalize(&self.root_dir).unwrap_or_else(\ | _\
| self.root_dir.clone())` silently swallows a canonicalize error, against
the fail-loud rule; a root that cannot be canonicalized degrades to an
unresolved prefix instead of erroring. Blast radius is a WRITE:
`materialize_images` (`:4525-4569`) takes the returned path,
`create_dir_all(parent)` and `fs.write(path, data)` — so image bytes land
wherever the traversal points, outside the vault root. The traversal string
is `block.content` of an image block, i.e. AUTHOR- or CRDT-SYNC-supplied
data, not developer input. SEVERITY CORRECTION (verified during the fix, and
this row originally overstated it): the write was **LATENT, not live**.
`with_image_data` had ZERO call sites at the time of the fix and the only
`provide::<dyn ImageDataProvider>` in the repo is test-only
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:623`),
so `image_data` was always `None` and both `materialize_images` and
`ingest_images` returned early — the traversal write was unreachable in the
shipped binary. "A synced peer can place bytes at an arbitrary path"
described the DESIGN, not the shipped behaviour; it arms the moment a
provider is registered, which the trait and builder seat plainly anticipate.
SAME BUG CLASS as the P1 vault path-escape triaged 2026-08-02 (the
name-chain `holon-pkm.org` write outside the vault), whose fix introduced
the `VaultPath` newtype (`crates/holon-filesystem/src/vault_path.rs`)
proving containment COMPONENT-WISE after normalization — that newtype is the
natural fix vehicle here (`VaultPath::resolve(root, block.content)?`
returning `Err` on escape, and the function returning the normalized
contained path, not `joined`). Second-order finding: the earlier triage
fixed only the call site it was reported at and never swept the tree for
other `root_dir.join(<untrusted>)` sites, which is why this one survived a
fix landed the same day.

## Missing piece

The keystone cannot GENERATE the triggering content. The only image-block
producer, `crates/holon-integration-tests/src/pbt/generators.rs:585-635`,
builds `format!("attachments/{stem}.{ext}")` from `"[a-z][a-z0-9_]{2,12}"`
and a fixed extension set — the alphabet contains no `/`, no `.`, no `..`,
no leading `/`, no `~`, so no generated run can put a traversal segment into
`block.content` and `resolve_image_path` is only ever called on
already-contained paths. Secondary ORACLE: even if such a path WERE
generated, nothing would go red — no invariant observes the FILESYSTEM
outside the vault root, so a write to `<root>/../evil.png` is unobserved by
the whole invariant catalog. Third, process-level gap: the sibling
path-escape triage produced no "what else has this shape" sweep, so
containment was fixed pointwise rather than as a class.

## Remedy

FIXED 2026-08-02 — `resolve_image_path` now returns a `VaultPath`-proven,
NORMALIZED, contained path or an enriched `Err`; the no-op `assert!` is gone
(untrusted synced data is a `Result`, not a broken invariant). A refused
path is a bounded per-block skip: the block keeps its content, no file is
written, other images in the same doc still materialize, the owning doc is
still written, and the refusal discloses ERROR-once-then-DEBUG per block via
the `first_failure_for_doc`/`clear_failure` machinery. RED (the strong one —
the escaping byte was ACTUALLY WRITTEN, proving the guard was a no-op rather
than merely absent): `an image block's content escaped the vault root and
PLANTED 25 bytes at ".../escape.png"`, with the contained control
`contained_image_path_still_materializes` PASSING in the same run, so the
red is specific to traversal and not "images are broken". CLASS CLOSED, not
just the instance: every `self.root_dir.join(<runtime string>)` in
`file_sync_controller.rs` is now an argument to `VaultPath` — three newly
routed (`file_uri_to_canonical_path`, `companion_doc_id`,
`resolve_image_path`) and seven already proven; zero bare joins remain. One
defect was also fixed one level up IN the shared primitive:
`VaultPath::inside` proved containment on the normalized path but STORED the
caller's un-normalized spelling, so the value proven and the value written
could differ — it now carries the normalized path (verified identity at all
seven prior call sites, since name-chain segments reject `.`/`..` and roots
are pre-canonicalized). TWO RESIDUALS recorded rather than fixed: (i)
`file_sync_controller.rs:3532` writes to the normalized target but bookkeeps
under the un-normalized path — they agree only because normalization is
currently the identity, and would silently diverge if an un-canonicalized
root ever reached the controller; (ii) PRE-EXISTING and unchanged by this
lane — containment is proven LEXICALLY while `CanonicalPath` resolves
symlinks, so an in-vault symlinked subdirectory pointing outward still
passes `VaultPath::inside`. That is the remaining hole in the boundary.
KEYSTONE COVERAGE IS HONEST-GAPPED: the hostile generator pool and the
`inv-no-write-outside-vault-root` invariant are landed and the invariant
engages in 100% of transitions, but a committed `#[ignore]`d non-vacuity
probe records that image materialization never reaches the FS in the
frontend slice, so THIS class is covered by the deterministic
`holon-orgmode` test, not by the keystone, until that is localized.
SUPERSEDED RED-FIRST PLAN (kept for the record): (1) extend the image-block
generator's path alphabet to draw from a small hostile pool alongside the
benign `attachments/<stem>.<ext>` — `../escape.png`, `a/../../escape.png`,
`attachments/../../escape.png`, `/tmp/escape.png`, and a benign-but-nested
`attachments/sub/img.png` control; the reference models a traversal path as
REJECTED (block keeps its content, no file materialized) and a contained
path as materialized. (2) Add a containment invariant
`inv-no-write-outside-vault-root`: after every composed run, assert that the
set of paths the harness `Fs` was asked to `write`/`create_dir_all` —
normalized, `..` resolved — is entirely under the run's vault root; this
needs the test `Fs` to record its write paths, which is the only harness
plumbing the lane must add. Both must be red on the unmodified tree WITH THE
ESCAPE PATH ACTUALLY WRITTEN (proving the assert is a no-op, not merely
absent). (3) Fix by routing `resolve_image_path` through `VaultPath` once it
lands, returning the normalized contained path and an `Err` (never a
swallowed `unwrap_or_else`) otherwise. (4) Sweep every other
`self.root_dir.join(<runtime string>)` in `file_sync_controller.rs` onto the
same primitive so the class, not the instance, is closed.
