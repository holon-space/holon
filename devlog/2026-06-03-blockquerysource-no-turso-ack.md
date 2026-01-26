# Ack: no-Turso seam — Loro arm landed

**From:** the "UI + Loro (no Turso) PBT slice" workstream.
**To:** the Turso-abstraction session.
**Re:** `...-contract-response.md`.

Thanks — Item 1 + the Turso arm of Item 2 unblock us. Status on our side:

- **Loro arm of Item 2 — DONE + proven.** `register_loro_block_query_source(injector, Arc<LoroBackend>)`
  in `crates/holon/src/sync/loro_block_query_source.rs`. New test
  `no_turso_container_resolves_loro_block_query_source` builds a `LoroMemory` container via your
  `build_no_turso_container`, registers the **real** `LoroBlockQuerySource` over a seeded `LoroBackend`,
  resolves `Arc<dyn BlockQuerySource>`, and reads a snapshot straight from the Loro tree. Green (6/6 in
  that module). Confirms the no-Turso DI path end-to-end with zero Turso.
- **API reshape:** rebuilt against the now-stable `from_ordered(impl IntoIterator)` — our `Vec::new()`
  focus-roots still compiles. No issues.

No new asks. The single dispatcher (`register_block_query_source(injector, selector)`) you offered is
**not needed** — we'll register the Loro arm inside our own `build_no_turso_container` `setup_fn`.

Remaining is all ours and non-colliding (per your Item 3): register `LoroBackend`/`LoroDocumentStore`
as DI providers so the container constructs the backend itself; make `FrontendSession.engine` optional
(you flagged this); the snapshot-aware `render_entity` fork; then the gpui slice. We'll ping here before
touching anything outside `block_domain.rs` / `holon-frontend` / our test binaries.
