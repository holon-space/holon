# That Shopping List — reverse-engineered sync API (2026-09-01)

Captured by driving the macOS/iOS app through a local mitmproxy (Martin's own account,
own list). Auth tokens, device UUID/serial, and real list contents were scrubbed; the raw
flow file was deleted after extraction. This doc is the authoritative spec for Kitchen
Inc C1 (read) and C2 (write).

Backend host: `thatshoppinglist.markenapps.com`. TLS is NOT certificate-pinned (mitm
decrypted cleanly). Content-Type `application/json`.

## Credential-in-path (C1 P26 / redact_url)
Every call is `…/!<TOKEN>/api/…`. The `!<TOKEN>` **first path segment is a bearer
credential**, of the shape `!<30ish chars of [A-Za-z0-9_-]>` — the real values are
deliberately NOT reproduced here; see the note below. Any log/error line printing the URL
leaks it. `redact_url` MUST scrub the leading `/!…/` segment.

**The token is STABLE per list, not per request.** Two captures taken across a cold
restart of the app carried the SAME token for the same list, so there is no mint/refresh
handshake to capture and no token-refresh machinery to build: the WHOLE URL is the secret,
it is configured once as `base_url: ${SHOPPING_LIST_URL}`, and rotating it means the user
obtaining a new share link. (This corrects the earlier "rotates per request" reading, which
was drawn from two captures of *different* lists.) The structural `!`-segment scrub stays
regardless — it is what protects an unregistered token in an echoed response body, and it
costs nothing.

> **Two real captured tokens were originally quoted here and have been replaced with a
> shape description (kitchen-c1, 2026-09-01).** They were live bearer credentials for
> Martin's account at capture time, and `docs/Plans/Kitchen.md` §4 already forbids quoting
> the credential in a plan or a fixture. **Published history is clean** (measured at C2): this
> file was created already scrubbed, so of the commits touching it that are reachable from
> `main@origin` — exactly one — none carries a token-shaped segment. No history rewrite is
> warranted. Two token-shaped segments survive only in local, unpushed jj operation-log /
> keep-refs on the capture machine, which is a local-disk residue (`jj util gc` territory), not
> a published exposure. Nothing is lost for the C2 lane: only the SHAPE is load-bearing, and
> tests use synthetic tokens of the same shape.

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
| **Check / uncheck** (toggle picked) | a command carrying `"picked":"del"` on the `good`, usually PAIRED with an `add`/`del` — the app moves the item between `items` and `pickedItems`. The exact on-vs-off encoding must be pinned by the C2 lane against a fresh 2-action capture (check, then uncheck, isolated) — see "Still open after C2" below. |
| **Rename** | NOT atomic — emitted as **`del <oldName>` + `add <newName>`** in one commit. This is the mechanism behind **R4: a rename drops any local-only state** (checked flag, product binding) because the new name is a new identity. (R4 accepted as a disclosed limitation — now evidence-backed.) |

### Concurrency / conflict
Optimistic via `oldVersion`. The `mode` query param almost certainly selects merge behavior
(e.g. force vs. rejec-on-stale); its values were not exhaustively observed. The C2
`ShoppingReconciler` (the seam C1 builds) takes `(local_rows, complete_remote_snapshot)` and
emits these commands. **C2 does not trust the ack's version as proof**: the peer answers a new
version whether or not someone else wrote in between, so the number cannot tell "applied on top of
what I read" from "applied on top of something newer". Instead the round re-pulls and re-reconciles
— pushes gone means the commit landed, pushes remaining means the list moved and the next commit is
based on the version just read. Two commits at most, then a loud failure; never a re-send over a
list the round has not read.

### Not a new transport kind (P2/P3)
This is the existing `rest` transport GROWN to POST with a JSON body template + response-version
extraction — landed at C2, generic and sidecar-declared, so gmail/gcal write legs reuse the same
machinery with no engine work.

## Still open after C2 (built around, not blocked on)
1. The exact **check-on vs check-off** command encoding (`picked:"del"` semantics). Needs a fresh
   isolated capture: check ONE item, commit; uncheck it, commit; diff the two commit bodies.
   Until then a local check is deliberately NOT pushed — a guessed encoding paired with an
   `add`/`del` risks deleting the item instead of ticking it. Checked still travels INBOUND, from
   `pickedItems` membership.
2. `mode` query values and the **stale-`oldVersion` rejection** response (force a conflict). C2
   needs neither: it does not trust the ack's version as proof a commit landed, it re-pulls and
   lets the reconciler decide, so the conflict is detected from the list itself.
3. Whether `del` of a *checked* item removes it from `pickedItems` too.
