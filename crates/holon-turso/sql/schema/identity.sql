CREATE TABLE IF NOT EXISTS canonical_entity (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    primary_label TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_canonical_entity_kind
ON canonical_entity(kind);

CREATE TABLE IF NOT EXISTS entity_alias (
    canonical_id TEXT NOT NULL REFERENCES canonical_entity(id),
    system TEXT NOT NULL,
    foreign_id TEXT NOT NULL,
    confidence REAL NOT NULL,
    PRIMARY KEY (system, foreign_id)
);

CREATE INDEX IF NOT EXISTS idx_entity_alias_canonical_id
ON entity_alias(canonical_id);

CREATE TABLE IF NOT EXISTS proposal_queue (
    id INTEGER PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_proposal_queue_status
ON proposal_queue(status);
