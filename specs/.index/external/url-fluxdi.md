---
name: fluxdi
description: Type-safe DI container used for module composition across all holon crates
type: reference
source_type: url
source_id: https://github.com/holon-space/fluxdi (branch: dev)
fetch_timestamp: 2026-04-23
---

## FluxDI (holon-space/fluxdi, branch: dev)

**Purpose**: Type-safe dependency injection container for Rust with module-based composition, async factory support, and lifecycle orchestration.

### Active Features in Holon

`thread-safe`, `async-factory`, `lifecycle`, `dynamic`, `eager-resolution`

### Key APIs

| Type / Macro | Role |
|-------------|------|
| `Injector` | Central container; resolves and owns services |
| `Module` | Organizes registrations; implements `configure()` |
| `provide!` | Registration macro |
| `inject!` | Resolution macro |
| `Shared<T>` | Arc-based shared service wrapper |
| Dynamic providers | String-key resolution avoids TypeId crate boundary issues |

### Patterns Used in Holon

- `CoreInfraModule`, `LoroModule`, `EventInfraModule` — domain modules wired at startup
- Async factory initialization for `BackendEngine`, Loro sync, Turso DB actors
- Feature `di` gates DI wiring per crate (avoids pulling FluxDI into WASM builds)
- Dynamic providers bridge TypeId issues across crate boundaries

### Integration in Holon

Used in: `holon`, `holon-frontend`, `holon-todoist`, `holon-orgmode`, `holon-mcp-client`, `holon-integration-tests`

### Keywords
dependency-injection, DI, module, injector, async, lifecycle, FluxDI
