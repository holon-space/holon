# Feasibility: Todoist WSJF Browser Overlay

*2026-02-21 — Phase 1 wedge exploration (see [BusinessAnalysis.md](BusinessAnalysis.md))*

---

## Goal

Inject a WSJF-sorted task list view into `app.todoist.com` via a browser plugin, userscript, or bookmarklet. Validate whether WSJF ranking over Todoist tasks delivers noticeable value before building deeper integration.

---

## Axis 1: Todoist Data Access

### REST API v2

Todoist exposes a REST API at `https://api.todoist.com/rest/v2/` with **open CORS** (`Access-Control-Allow-Origin: *`). A single `GET /rest/v2/tasks` call with a Bearer token returns all active tasks as structured JSON. No proxy, no DOM scraping, no intercepting internal calls.

Authentication is a personal API token (Settings → Integrations → API token). OAuth is also available for distributable apps.

### WSJF-Relevant Fields

| WSJF Component | Todoist Field | Type | Notes |
|---|---|---|---|
| Business Value | `priority` | Integer 1–4 | 1 = normal, 4 = urgent. Coarse but usable. |
| Time Criticality | `deadline.date` | `YYYY-MM-DD` | Hard deadline. Days-until-deadline → urgency score. |
| Time Criticality (secondary) | `due.date` / `due.datetime` | Date | Planned work date, not a hard deadline. |
| Job Size | `duration.amount` + `duration.unit` | Integer + `"minute"`/`"day"` | Estimated effort. Max 24h. |
| (Augmentation) | `labels` | String[] | Can encode richer scoring: `bv:8`, `risk:high`, etc. |
| (Augmentation) | `description` | String | Could contain structured metadata (YAML frontmatter, key-value pairs). |

### What's Missing

- **Business value granularity**: `priority` has only 4 levels. Fibonacci-scale labels (`bv:1` through `bv:13`) or description metadata can augment this.
- **Risk Reduction / Opportunity Enablement**: No native field. Requires label or description conventions.
- **Dependency information**: Todoist has no dependency model. Subtasks (`parent_id`) exist but are hierarchical, not dependency edges.

### Sync API v9

The web app itself uses `POST /sync/v9/sync` for incremental sync. Same auth, same data, slightly different field names. Relevant if intercepting in-page traffic, but the REST API is simpler for a standalone integration.

### Verdict

Data access is straightforward. One API call returns everything needed. No DOM scraping required.

---

## Axis 2: Injection Approach

### Bookmarklet

**Not viable.** Modern SPAs set CSP headers that block `javascript:` URIs. Todoist is a React SPA with CSP. Even if it worked:
- Must re-click after every SPA navigation
- Practically limited to ~2000 characters of code
- No persistent storage for settings or API token
- No cross-origin requests without CORS

### Userscript (Tampermonkey / Greasemonkey)

**Viable, fastest to prototype.**

- Tampermonkey injects scripts as extension content scripts, bypassing page CSP
- Full DOM access for injecting a sidebar panel
- `GM_xmlhttpRequest` for API calls (bypasses CORS if needed, though Todoist's API already allows CORS)
- `GM_setValue` / `GM_getValue` for persistent storage (API token, settings)

Challenges:
- Todoist is a React SPA that re-renders DOM subtrees on navigation and state changes
- Needs `MutationObserver` to detect when React tears down/rebuilds the DOM
- Tampermonkey's `@match` only fires on full page loads, not SPA route changes — needs URL polling or `popstate` listener
- Coupled to Todoist's DOM structure for positioning the overlay (though not for data, since we use the API)

Estimated effort: **~150–250 lines** of JS. No build step. Paste into Tampermonkey and go.

### Browser Extension (Manifest V3)

**Viable, more robust long-term.**

- Content script runs in isolated world, exempt from page CSP
- Background service worker can make API calls independently
- `chrome.storage` for persistent settings
- `chrome.webRequest` / `declarativeNetRequest` available if you ever want to intercept Todoist's own API traffic
- Can inject CSS files cleanly for sidebar styling

Same SPA challenges as userscript (MutationObserver, route change detection), but the architecture is cleaner: background worker handles data + scoring, content script handles rendering.

Estimated effort: **~300–400 lines** across 4–5 files (`manifest.json`, background worker, content script, styles, popup for settings). Requires loading as unpacked extension during dev.

### Comparison

| | Bookmarklet | Userscript | Browser Extension |
|---|---|---|---|
| CSP blocked? | Likely yes | No | No |
| SPA navigation | Must re-click | Workaround needed | Workaround needed |
| Persistent storage | None | `GM_setValue` | `chrome.storage` |
| API calls | CORS-dependent | `GM_xmlhttpRequest` | Background worker |
| Build step | None | None | None (zip to distribute) |
| Effort | ~50 LOC (broken) | ~150–250 LOC | ~300–400 LOC |
| Distribution | Share URL | Share .user.js | Chrome Web Store or .crx |

---

## Axis 3: Implementation Language

### Option A: TypeScript / JavaScript

The conventional choice for browser extensions. Ecosystem advantages:
- No build toolchain beyond optional TS transpilation (or skip TS entirely for vanilla JS)
- Direct DOM manipulation, direct `fetch()`, direct `chrome.*` APIs
- Instant reload cycle (save → reload extension)
- Entire extension is a few hundred lines in a single language

The WSJF computation is arithmetic on ~100 tasks — any language handles this trivially.

### Option B: Rust → WebAssembly

Rust compiled to Wasm via `wasm-pack` + `wasm-bindgen`. The scoring logic lives in Rust, called from a JS shell.

**Advantages:**
- Shared codebase with existing `petri.rs` scoring engine — no parallel implementation to maintain
- Type safety and correctness guarantees from Rust's type system
- If the scoring model grows beyond simple WSJF (dependency graphs, Petri Net materialization, Rhai expression evaluation), Rust handles that complexity better
- Path toward running `petri.rs` in-browser if the product evolves that direction

**Costs:**
- Build toolchain: `wasm-pack`, `wasm-bindgen`, bundler configuration for loading `.wasm` in extension context
- JS↔Wasm boundary: API calls happen in JS, results pass through serialization to Wasm and back
- DOM manipulation either goes through `web-sys` (verbose) or stays in JS (meaning most code is JS anyway)
- MV3 CSP can complicate Wasm module loading (requires `wasm-unsafe-eval` in extension CSP)
- Bundle size: ~30–100KB for minimal Wasm vs ~2KB for equivalent JS
- Slower dev cycle: `cargo build --target wasm32-unknown-unknown` + wasm-pack + reload

**When Rust/Wasm becomes compelling:**
- Porting `petri.rs` to run client-side (real graph computation, topological sorting, prototype resolution)
- Adding CRDT-based offline sync (Loro, Automerge — tight loops over binary structures)
- Sharing a single scoring core across CLI, backend, Flutter, and browser
- Running local LLM inference for task classification

### Option C: Rust → Wasm (UI framework)

Using Leptos, Yew, or Dioxus for the entire extension UI. These compile to Wasm and manage their own DOM.

**Advantages:**
- Entire extension in Rust, no JS at all (in theory)
- Component model familiar to React developers

**Costs:**
- Significant framework overhead for what is a single sorted list
- Browser extension lifecycle (`chrome.*` APIs, service workers) still needs JS glue or `web-sys` bindings
- Heaviest bundle size of all options
- Slowest iteration speed

This makes more sense for a standalone web app than a lightweight overlay on someone else's SPA.

---

## WSJF Scoring Model (Applicable to All Approaches)

Basic formula using available Todoist fields:

```
urgency = max(0, 1 - days_until_deadline / horizon_days)   // 0..1, 1 = due today
value   = priority / 4                                       // 0.25..1
size    = duration_minutes (default: 30 if unset)

wsjf    = (value + urgency) / size
```

Refinements via label conventions:
- `bv:N` — business value on Fibonacci scale (1, 2, 3, 5, 8, 13), replaces `priority`
- `risk:N` — risk reduction score
- `@PersonName` — delegation detection (task is "waiting for" if assigned + has person label)

Tasks without `deadline` get `urgency = 0`. Tasks without `duration` get a configurable default.

---

## Open Questions

1. **Todoist UI Extensions API**: Todoist has an [official UI Extensions SDK](https://developer.todoist.com/ui-extensions). Could this provide a sanctioned way to add a WSJF view without injection? Needs investigation — it may be limited to context menus and modals rather than custom views.

2. **Token management**: Asking users to paste an API token is friction. Could the extension extract the session token from Todoist's cookies/localStorage instead? This avoids the setup step but couples to Todoist's auth internals.

3. **Real-time updates**: The REST API is request/response. For live updates as tasks change in the Todoist UI, you'd either poll periodically or intercept the Sync API websocket. Polling every 30–60s is probably sufficient for an MVP.

4. **Mobile**: A browser extension only works on desktop. Phase 1 wedge is desktop-only. Mobile users continue using Todoist natively.

---

## Related Documents

- [BusinessAnalysis.md](BusinessAnalysis.md) — Wedge strategy and go-to-market phases
- [docs/Vision/PetriNet.md](docs/Vision/PetriNet.md) — Petri Net model that WSJF scoring feeds into
- [docs/Vision.md](docs/Vision.md) — Technical vision and phased roadmap
