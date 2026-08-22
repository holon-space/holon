# Capability ledger

One entry per TIGHTENING PROMPT the certifier raised: a place where a format
does MORE than its capability profile claims. A prompt never fails a gate
(CV-C) — it is a standing invitation to tighten the profile, and it stays here
until someone measures the question it asks and either promotes the clause or
records why not.

Violations do not live here. A violation — the profile declares a restriction
the format does not honour — is a RED in the certification run, not a ledger
entry.

## The three commands

```
just capability-cert     # certify org against crates/holon-org-format/profile.yaml
just capability-sweep    # falsify every clause; a clause that cannot go red fails
python3 scripts/capability-ledger.py counts | sync --from <report> | diff --from <report>
```

`capability-cert` runs the real ingest → write-back → ingest loop and prints
violations, coverage gaps, tightening prompts, clauses driven, layer deferrals
and `not_yet_certified` markers. It also writes the machine-readable report to
`target/capability-certification/<profile>.json`.

`capability-sweep` is the gate ON the gate. Every clause the profile declares —
and every member of every set-valued clause — gets flipped to a value the
format does not honour; a driven clause must move
some counter, and a clause the profile EXCUSES — deferred to another layer, or
marked `not_yet_certified` — must stay inert. Either surprise fails the script.
It certifies a copy of the yaml under `target/capability-sweep/` and never
writes into the source tree.

A sweep writes its reports to `target/capability-sweep/reports/` and re-runs a
clean certification as its last act, so the ledger's input is always the honest
profile's report: a sweep run that overwrote it would make `diff` accuse the
profile of a prompt only a deliberately broken flip produced.

`capability-ledger.py diff --from target/capability-certification/org.json`
names every prompt in the report that has no entry here. It is
non-blocking by design: a HUMAN runs `sync --from …` to materialize the
entries, the same way the bug funnel is hand-written.

## Entry front matter

`id`, `date`, `profile`, `axis`, `clause`, `leg`, `construct`, `status`,
`summary`. `status` is `OPEN` (the question stands) or `PROMOTED` (the profile
now declares what was measured). `(profile, axis, leg, construct)` is the key
the tooling deduplicates on, so it must match the report's prompt exactly.
