# That Shopping List — reverse-engineered sync API (2026-09-01)

Captured by driving the macOS/iOS app through a local mitmproxy (Martin's own account,
own list). Auth tokens, device UUID/serial, and real list contents were scrubbed; the raw
flow file was deleted after extraction. This doc is the authoritative spec for Kitchen
Inc C1 (read) and C2 (write).

Backend host: `thatshoppinglist.markenapps.com`. TLS is NOT certificate-pinned (mitm
decrypted cleanly). Content-Type `application/json`.

## Credential-in-path (C1 P26 / redact_url)
Every call is `…/!<TOKEN>/api/…`. The `!<TOKEN>` **first path segment is a bearer
credential** and it **ROTATES per request** (two captured calls carried two different
tokens of the shape `!<30ish chars of [A-Za-z0-9_-]>` — the real values are deliberately
NOT reproduced here; see the note below). Any log/error line printing the URL leaks
it. `redact_url` MUST scrub the leading `/!…/` segment. This is C1's hard prerequisite.

> **Two real captured tokens were originally quoted here and have been replaced with a
> shape description (kitchen-c1, 2026-09-01).** They were live bearer credentials for
> Martin's account at capture time, and `docs/Plans/Kitchen.md` §4 already forbids quoting
> the credential in a plan or a fixture. Rotation shortens the exposure but does not remove
> it, and the git history still holds the originals — if that matters, the history needs
> scrubbing too. Nothing is lost for the C2 lane: only the SHAPE is load-bearing, and tests
> use synthetic tokens of the same shape.

## Read (C1)
`GET /!<tok>/api/list/<LIST_ID>?oldVersion=<n>&version=<n>&_nocache=<ms>`
LIST_ID is a stable opaque id (e.g. `<LIST_ID>`). Response:
```
{
  "items":   [ {"name": str, "cat": <catCode>} ],        // the ACTIVE (unchecked) list
  "pickedItems": { "<name>": {"cat": <catCode>, "date": <iso>} },  // CHECKED-off items, keyed by NAME
  "version": <int>,                                       // list version (optimistic concurrency)
  "options": {"prices": bool, "cats": [<catCode>...]}     // category vocabulary for this list
}
```
- **Item identity = `(name, cat)`.** There is **NO server-issued item id.** (Confirms §4 and R4.)
- **`checked` is real**: an item is checked iff its name is a key in `pickedItems`. (Confirms R3 — the app DOES have a checked concept; C1 stores it in the local-only `checked` column.)
- `cat` is a short code (`"O"`, `"S"`, `"FuV"`, …); the code→label vocabulary is `options.cats`
  (some entries carry an icon/color suffix, e.g. `Kleidung_clothes_1976D2`). Parse cat as an
  enum/newtype over the list's own `options.cats`, not a free string (P23 / parse-don't-validate).

## Write (C2)
`POST /!<tok>/api/list/<LIST_ID>/commit?version=<n>&mode=<m>`  — a **batched command** protocol.
Request envelope:
```
{
  "oldVersion": <int>,               // the version the client last saw (optimistic concurrency)
  "oldPickedItemsVersion": <int>,    // same, for the pickedItems vector
  "device": {"id": <client-uuid>},   // stable per install; not a secret in the same way as the path token
  "lang": "en",
  "commands": [ <cmd>, ... ]         // applied in order; each has a client-generated id
}
```
Response: `{"version": <newInt>, "pickedItemsVersion": <newInt>, "options": {...}}` — the server
returns the NEW versions; feed them into the next `oldVersion`/`oldPickedItemsVersion`.

Each command carries `id` = `"<epoch_ms>_<seq>"` (client-generated, idempotency/ordering key)
and a `good` = `{"name": str, "cat": <catCode>, "new"?: true}` identifying the item by (name,cat):

| Intent | Command shape (observed) |
|---|---|
| **Add** an item | `{"cmd":"add", "good":{"name","cat","new":true}, "id"}` |
| **Delete** an item | `{"cmd":"del", "good":{"name","cat","new":true}, "id"}` |
| **Check / uncheck** (toggle picked) | a command carrying `"picked":"del"` on the `good`, usually PAIRED with an `add`/`del` — the app moves the item between `items` and `pickedItems`. The exact on-vs-off encoding must be pinned by the C2 lane against a fresh 2-action capture (check, then uncheck, isolated) — see "Open for the lane" below. |
| **Rename** | NOT atomic — emitted as **`del <oldName>` + `add <newName>`** in one commit. This is the mechanism behind **R4: a rename drops any local-only state** (checked flag, product binding) because the new name is a new identity. (R4 accepted as a disclosed limitation — now evidence-backed.) |

### Concurrency / conflict
Optimistic via `oldVersion`. The `mode` query param almost certainly selects merge behavior
(e.g. force vs. rejec-on-stale); its values were not exhaustively observed. The C2
`ShoppingReconciler` (the seam C1 builds) takes `(local_rows, complete_remote_snapshot)` and
emits these commands; on a version mismatch the server response's new version tells the client
to re-pull and re-reconcile.

### Not a new transport kind (P2/P3)
This is the existing `rest` transport GROWN to POST with a JSON body template + response-version
extraction (Kitchen plan C2 scope). gmail/gcal write legs reuse the same machinery.

## Open for the C2 lane (pin before building)
1. The exact **check-on vs check-off** command encoding (`picked:"del"` semantics). Do a fresh
   isolated capture: check ONE item, commit; uncheck it, commit; diff the two commit bodies.
2. `mode` query values and the **stale-`oldVersion` rejection** response (force a conflict).
3. Whether `del` of a *checked* item removes it from `pickedItems` too.
