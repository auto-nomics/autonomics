-- 0002_skills.sql — reusable subgraphs with versioned history.

CREATE TABLE skills (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    current_version INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE skill_versions (
    id                   TEXT PRIMARY KEY,
    skill_id             TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    version              INTEGER NOT NULL,
    parent_id            TEXT,
    description          TEXT NOT NULL,
    sop_text             TEXT NOT NULL,
    tool_refs_json       TEXT NOT NULL,
    manifest_json        TEXT NOT NULL,
    surface_inputs_json  TEXT NOT NULL,
    surface_outputs_json TEXT NOT NULL,
    created_at           TEXT NOT NULL,
    UNIQUE(skill_id, version)
);

CREATE INDEX idx_skill_versions_skill
    ON skill_versions(skill_id, version DESC);