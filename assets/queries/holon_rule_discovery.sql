-- Single-block `holon_rule` discovery (ADR 0024 §7.2 — unified rule surface).
--
-- Unlike the query+action *pair* (action_discovery.sql), a single-block rule is
-- self-contained: its YAML body carries BOTH the guard (`when:`) and the effect
-- (`emit:`), so there is no sibling trigger block to join. This is the shape the
-- migrated journal-auto-create rule uses.
--
-- The scan mirrors the advice-rule discovery (get_advice_rules.sql): a flat
-- `WHERE source_language = 'holon_rule'` over `block`. The body is parsed at the
-- Rust boundary (`parse_holon_rule`); a body that is not valid rule YAML surfaces
-- a LOUD RuleStatus::ParseError on the rule card — `holon_rule` is YAML-only now,
-- the legacy Rhai `block.create(...)` action body having been retired.
--
-- No anti-join against a sibling trigger is used (it would require a matview
-- reading the `block` matview — the chained-matview hang). The pair-watcher and
-- this watcher never both fire one block: the pair-watcher requires a sibling
-- `holon_sql`/`holon_prql`/`holon_gql` trigger, which a single-block rule lacks.
SELECT id, content
FROM block
WHERE content_type = 'source'
  AND source_language = 'holon_rule'
