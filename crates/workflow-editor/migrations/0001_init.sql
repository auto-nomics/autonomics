-- 0001_init.sql — workflows + git-style snapshots.

CREATE TABLE workflows (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    manifest_json   TEXT NOT NULL,
    manifest_hash   TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX idx_workflows_updated_at ON workflows(updated_at DESC);

CREATE TABLE workflow_snapshots (
    id              TEXT PRIMARY KEY,
    workflow_id     TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    parent_id       TEXT,
    manifest_hash   TEXT NOT NULL,
    manifest_json   TEXT NOT NULL,
    commit_message  TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_workflow_snapshots_workflow
    ON workflow_snapshots(workflow_id, created_at DESC);
CREATE INDEX idx_workflow_snapshots_parent
    ON workflow_snapshots(parent_id);