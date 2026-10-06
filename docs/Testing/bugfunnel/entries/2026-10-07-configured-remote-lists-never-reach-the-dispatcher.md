---
id: 2026-10-07-configured-remote-lists-never-reach-the-dispatcher
date: 2026-10-07
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  An enabled `holon.list_sync` sidecar never gets its remote-list operation in
  the production wiring: the provider-set member reads `ConfiguredLists` with a
  sync resolve, which always fails on that async provider, and the failure is
  replaced with an empty default.
---

## Bug

`crates/holon/src/di/registration.rs:331-343` builds `ConfiguredRemoteLists`
inside the `OperationProvider` set. It reads the lists with
`inj.try_resolve::<holon_connections::ConfiguredLists>()` and falls back to
`unwrap_or_default()`. `McpIntegrationsModule` registers `ConfiguredLists` with
`Provider::root_async` (`crates/holon-app/src/mcp_integrations.rs:945`).
fluxdi's sync resolve returns `AsyncFactoryRequiresAsyncResolve` for an async
provider that has no cached instance (fluxdi `injector/ts_instance_factory.rs`,
the `async_factory.is_some()` arm), and nothing resolves `ConfiguredLists`
asynchronously first. So the dispatcher always gets an empty list set, and
`remote_list_sync` is never registered for any connection. Nothing says so.

Found by agent exploration (lane `incr-boot`, spike S1 of the incremental
integration boot plan). Probe on integration c47f80f5, bundled `shopping`
sidecar enabled, `SHOPPING_LIST_URL` set to a loopback URL:

```
S1 sync try_resolve AFTER boot: Err((AsyncFactoryRequiresAsyncResolve) - Type holon_connections::registry::ConfiguredLists is registered with an async provider; use try_resolve_async/resolve_async)
S1 async resolve AFTER boot: Ok(lists=1, refusals=[])
S1 dispatcher remote-list ops: []
```

## Missing piece

The keystone's remote-list SUT registers `RemoteListOperations` on the
dispatcher by hand (`crates/holon-integration-tests/src/pbt/composed/remote_list_sut.rs:128`),
and `crates/holon-app/tests/shopping_pull_mock.rs` builds the provider
directly. No test resolves the remote-list operation through the production DI
wiring, so the empty default is unobservable.

## Remedy

OPEN. The incremental integration boot plan
(`~/.claude/plans/incremental-integration-boot.md`) moves remote-list
registration into the per-integration connect path, which removes this
provider-set member. The `unwrap_or_default()` must go either way: an absent
`ConfiguredLists` is "no list sidecar configured", any other resolve error is a
wiring bug and must fail loud.
