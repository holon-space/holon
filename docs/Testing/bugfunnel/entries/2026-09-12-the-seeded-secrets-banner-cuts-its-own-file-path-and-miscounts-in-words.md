---
id: 2026-09-12-the-seeded-secrets-banner-cuts-its-own-file-path-and-miscounts-in-words
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The in-memory-secrets banner names how many fixture secrets were planted but
  ellipsises the file they came from, and says "1 seeded fixture secrets" — so
  the one fact a reader would act on is the one that is cut.
---

## Bug

Found by the `user-connections` dogfood RE-CHECK of 2026-09-12, driving the
real GPUI app at `main` `060022da56b6` with
`HOLON_SECRETS_MEMORY_SEED` pointing at a one-entry fixture file.

The disclosure banner is painted and correct in substance. Its last sentence
reads, in a 1400x900 window with the rest of the toast intact:

```
1 seeded fixture secrets were pre-loaded from
'/private/var/folders/hc/2q6czxpx6j9_87bq…
```

Two defects in that sentence:

- **The path is cut.** The clause exists to say WHICH file planted the
  credentials the session is about to read as configured. The temp path is long
  enough that the cap lands inside it, and the part that survives —
  `/private/var/folders/hc/…` — is the part every macOS temp path shares. The
  file name, the only distinguishing half, is never painted.
- **"1 seeded fixture secrets"** does not agree in number.

The cut is not a layout problem: the toast's own box has room below the last
line, and the same banner at 800 wide cuts at the same character.

Evidence under `scratchpad/dogfood-uc-recheck/shots/`:

- `A-01-boot.png`, `A-02-settings.png` — 1400x900.
- `B-01-table-800.png` — 800x900, cut at the same point.

## Root cause

`crates/holon-secrets/src/lib.rs:359-374` builds ONE `disclosure` string: the
standing memory-backend sentence, then `push_str` of the seed sentence with the
count and the path. The whole thing reaches the toast as a headline, and the
headline is the capped half — `DegradedToast`'s `ToastDetail` split (headline
capped, body lines painted verbatim) was introduced by the `uc-fixes-2` lane
for the integration refusals and this disclosure never moved onto it.

So the seed path is a payload sitting on a prose line, which is precisely the
shape that lane's fix was written to eliminate.

The plural is the `format!` at line 369: `"{planted} seeded fixture secrets"`
with no singular arm.

## Missing piece

`in_memory_secret_backend_boot.rs` asserts the seed is applied, the collision
is refused and the platform-keychain combination stops the boot — all through
`Result`s. The windowed rung `settings_seeded_secret_windowed` asserts the
Settings field reads as stored. Neither judges the banner's painted text, so
the disclosure's own legibility is unowned.

## Remedy

Open, and small: move the seed path onto a `ToastDetail` body line the way the
refusal payloads now are, and give the count a singular arm. A windowed case
asserting the seed file's NAME is painted would pin it.

Severity is low — this banner appears only in a throwaway session that opted
into `HOLON_SECRETS_BACKEND=memory` — but it is the disclosure that stops a
screenshot passing a fixture off as a real credential, so it should say which
fixture.
