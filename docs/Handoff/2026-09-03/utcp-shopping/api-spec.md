# That Shopping List — API spec derived from the code

Sources: the code in `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/kitchen-dogfood`
(read-only) plus the capture doc `docs/Plans/ThatShoppingList-API-2026-09-01.md`, which the
sidecar names as the authoritative wire contract. Line references are absolute into that tree.

## 1. Base URL and the empty-path rule

`SHOPPING_LIST_URL` is the whole API base of one list, ending in the list id:
`<scheme>://<host>/!<token>/api/list/<listId>`. Nothing in Holon parses it
(`assets/integrations/shopping.yaml:14-18`, `:39`).

Join rule (`crates/holon-mcp-client/src/rest_transport.rs:306-315`): the base is right-trimmed of
`/`, the relative path is left-trimmed of `/`, and **an empty relative path yields the base
itself** with no trailing separator. That is what lets `list-items` declare `path: ""`.

The `!`-prefixed opaque token is a **path segment**, never a header and never a query parameter.
There is no auth block (`RestAuth::None`). The token is stable per list, so there is no refresh
handshake. The base URL is therefore the credential in full; referencing it as `${VAR}` is what
marks it secret and strips it from errors and logs (`rest_transport.rs:286`, `safe_url`).

Measured against the live host, two response quirks are not in the code doc: bodies come back
**gzip-encoded** and labelled **`Content-Type: text/html`** despite being JSON.

## 2. Read

```
GET <base>?oldVersion=<n>&version=<n>&_nocache=<epoch_ms>
```

All three parameters are sent on every call (`crates/holon-app/src/shopping_rest.rs:86-93`). Both
version parameters carry the same high-water mark, 0 before the first pull (`:73-75`). `_nocache`
is `Utc::now().timestamp_millis()` (`:79-81`) and is load-bearing: the write leg re-reads to
confirm its own commit landed, and a cached body there reports a landed write as missing and gets
it sent twice.

Response, parsed at `crates/holon-kitchen/src/shopping.rs:262-300`, every field required unless
noted:

| Field | Type | Notes |
|---|---|---|
| `version` | integer | `shopping.rs:268` |
| `pickedItemsVersion` | integer, optional | absent or null reuses `version` (`:269-272`) |
| `items` | array | the ACTIVE, unchecked list (`:276-283`) |
| `items[].name` | non-empty string | `:398-404` |
| `items[].cat` | non-empty string | `:406-412` |
| `items[].count` | number, null, or absent | `:414-424` |
| `pickedItems` | object keyed by item NAME | membership means checked (`:285-292`, `:394`) |
| `options.cats` | array of strings | category vocabulary (`:349-363`) |

A `cats` entry is either `code` or `code_icon_color`; the code is the segment before the first
underscore (`shopping.rs:41-49`). A duplicate code is a hard error (`:89-93`). There is **no
compiled-in vocabulary** — it is parsed per list.

```json
{"items":[{"name":"Milk","cat":"R","count":2},{"name":"Bread","cat":"B"}],
 "pickedItems":{"Rice":{"cat":"Ca","date":"2026-09-01T08:00:00Z"}},
 "version":7,
 "options":{"prices":false,"cats":["R","B","Ca","Ir","Kleidung_clothes_1976D2"]}}
```

## 3. Write

```
POST <base>/commit?version=<oldVersion>
```

Envelope (`shopping.yaml:61-67`, `shopping_rest.rs:104-114`, `shopping_sync.rs:58-64`):

```json
{"oldVersion":7,"oldPickedItemsVersion":7,"device":{"id":"<deviceId>"},"lang":"en",
 "commands":[{"cmd":"add","good":{"name":"Eggs","cat":"R","new":true},"id":"1756713600000_3f0a1c9d2b4e5f60"},
             {"cmd":"del","good":{"name":"Bread","cat":"B","new":true},"id":"1756713600000_a1b2c3d4e5f60718"}]}
```

Only `add` and `del` are emitted (`shopping_sync.rs:34-45`); `good.new` is always literal `true`
(`:61`). A check toggle is deliberately unrepresentable (`shopping.rs:519-531`) because the
on-versus-off encoding was never isolated in a capture, and a guessed encoding paired with an
add/del risks deleting the item instead of ticking it. A rename is not atomic: it is `del` of the
old name plus `add` of the new one, which is why a rename drops local-only state.

Response: `version` (required), `pickedItemsVersion` (optional, defaults to the request's old
value, `shopping_rest.rs:129-132`). The transport re-emits the new version under a neutral key so
the caller carries no field name of its own (`shopping.yaml:70`, `rest_transport.rs:386-418`).

## 4. Concurrency and idempotency

The ack is **not** proof the commit landed (`shopping_sync.rs:178-187`): the server bumps the
version whether or not someone else wrote in between, so the number cannot distinguish "applied on
top of what I read" from "applied on top of something newer".

The round therefore detects conflict from the list itself (`shopping_sync.rs:195, 202-238`):
pull, reconcile, commit, then re-pull to at least the acked version. If the reconciler still emits
pushes, the list moved and a second commit goes out against the freshly read version. At most two
commits, then a loud failure. `ack.version < batch.old_version` is treated as a peer bug and errors
out (`:228-234`). `pull_at_least` retries the read up to three times (`:250-269`), which is the
other reason `_nocache` matters.

Idempotency: `command_id = "{round_ms}_{16 hex of a hash over verb and key}"`
(`shopping_sync.rs:129-134`). The same logical command in the same round gets an **identical** id
across both commits, deliberately using `now_ms` rather than `now_ms + attempt`. Reminting the id
on retry is a pinned regression
(`docs/Testing/bugfunnel/entries/2026-09-01-shopping-retry-remints-idempotency-key.md`).

## 5. Reconciler

`ShoppingReconciler` (`shopping.rs:562-653`) takes local rows plus one complete snapshot. Local
rows are keyed by `(name, cat)`; a duplicate key is a hard error (`:570-577`).

For each remote item: unknown yields `Insert`; held with a live tombstone yields a `Remove` push;
an expired tombstone resurrects as `Insert`; otherwise `SetCount` on divergence, `Check` only
remote-to-local, and always `TouchLastSeenRemote` (`:580-632`). For local rows absent from the
snapshot: tombstoned yields `ReapTombstone`; a row previously seen remotely yields `Delete`
(absence as deletion); a row never seen remotely yields an `Add` push (`:634-650`). The tombstone
window defaults to seven days (`:547`).

API-specific: the `(name, cat)` identity (the peer issues no id, `:182-191`), `row_id()` as
`"shopping:{cat}:{name}"` (`:205-207`), checked derived from `pickedItems` membership plus the
one-way check rule, duplicate count folding (`:336-347`), and rename as del-plus-add.

Generic: tombstone and watermark bookkeeping, absence-as-deletion under a complete snapshot, and
the intent-versus-push split. That is ordinary reconciliation of local intents against remote rows
carrying keys and versions.
