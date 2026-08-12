-- 0003_node_kinds.sql — registry cache for node kinds and their schemas.

CREATE TABLE node_kinds (
    kind            TEXT PRIMARY KEY,
    label           TEXT NOT NULL,
    description     TEXT NOT NULL,
    spec_schema     TEXT NOT NULL,
    ports_json      TEXT NOT NULL,
    doc             TEXT NOT NULL,
    category        TEXT NOT NULL,
    registered_at   TEXT NOT NULL
);

CREATE INDEX idx_node_kinds_category ON node_kinds(category);