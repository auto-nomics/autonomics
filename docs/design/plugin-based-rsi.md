# Plugin-Based RSI Design

> Status: draft · 2026-10-04
>
> Goal: let feedback, repeated workflow failures, and explicit requests drive a
> supervised multi-agent workflow that drafts new manifest nodes or optimizes
> existing ones. The output is a testable, reviewable plugin proposal. Agents
> never directly mutate the daemon plugin root or live node registry.

## 1. Current Landscape

### Already strong

- **Manifest plugins** provide the correct executable boundary. A family is a
  directory with `manifest.toml`, scripts, fixtures, and tests. Loading is
  fail-closed; its `[image]` field names a reusable runtime environment;
  scripts execute in containers with declared ports, parameters, resources, and
  panels.
- **Plugin distribution is immutable.** `plugins.toml` accepts either a local
  path or git plus a full commit SHA. The plugin source commit and selected
  environment digest are therefore reproducible after release.
- **The node registry is isolated from agent file access.** Manifest plugins are
  loaded from an admin-controlled root when the DataEngine starts. The comments
  and implementation deliberately emphasize that agents cannot reach host plugin
  paths.
- **Multi-agent infrastructure exists.** RuntimeHost can spawn hierarchical
  agents, delegate tasks, route messages, and expose per-session DAG clients.
- **A feedback loop already exists for knowledge, not executable code.**
  `skill_observe` records durable observations; eval failures are captured
  automatically; deterministic distillation creates Markdown skill proposals;
  agent-authored proposals are always human-review-only.
- **Plugin engineering patterns are established.** There are already dozens of
  external families under `/mnt/projects/node-plugins`, plus golden migration
  tests, release checklists, and representative fixtures.

### Main gaps

1. **Feedback cannot express executable-node demand.** `ObservationKind` has
   only failure, recipe, and caveat, and distillation writes `SKILL.md`. There
   is no structured request for a new node kind, missing parameter, changed
   port contract, or adapter bug.
2. **No proposal store for plugin trees.** Skill proposals validate one
   Markdown file. A plugin proposal must carry a whole directory, preserve
   baseline and diff history, and bind evidence to test reports.
3. **`nodedev` is only a minimal scaffold.** It creates a placeholder manifest;
   it does not copy a baseline, generate tests, validate a staged family, or
   produce a review artifact.
4. **Live registry reload is absent by design.** `DataEngineManager` snapshots
   an `Arc<NodeRegistry>` at startup and every existing client retains that
   snapshot. Rebuilding the registry is a deployment operation, not a safe
   in-place mutation.
5. **Environment creation must remain outside the Agent trust boundary.**
   Ordinary plugin proposals select an already-approved environment and contain
   no Dockerfile. New environments are separate reusable assets built only by
   trusted infrastructure.
6. **No integrated test runner for arbitrary staged plugin proposals.** Rust
   integration tests are excellent for checked-out families, but RSI needs a
   runtime-facing runner that accepts a proposal directory and returns a stable
   JSON report.

## 2. Design Principle

Plugin-based RSI should evolve **proposals**, not the live system directly:

```text
feedback / failure / request
        |
        v
structured RSI request
        |
        v
multi-agent proposal workspace
        |
        v
deterministic validation pyramid
        |
        v
human-approved immutable plugin release
        |
        v
daemon restart / registry generation install
```

This mirrors the existing skill-evolution safety model while acknowledging that
plugin changes alter executable behavior. The model may draft and test; humans
and trusted infrastructure promote.

## 3. Artifact Model

Create a daemon-owned staging area:

```text
<state_dir>/
  plugin-rsi/
    requests/
      <request-id>.toml
    proposals/
      <plugin-name>/
        <proposal-id>/
          proposal.toml
          repo/
            manifest.toml
            scripts/
            fixtures/
            README.md
            .git/
          baseline/
            manifest.toml
            scripts/
          reports/
            <attempt-id>.json
          history/
```

### Request record

A request is content-addressed and idempotent. It may be created by a user,
agent, workflow failure, or eval failure:

```toml
id = "R-..."
created_at = 1760000000
source = "user"
intent = "new_node"
plugin_name = "clusterprofiler"
summary = "Expose organism-specific identifier mapping"
body = "..."
evidence_ids = ["O-...", "run-..."]
status = "open"
```

Valid `source` values are `user`, `agent`, `workflow_run`, `eval`, and
`observation`. Valid intents are `new_node`, `optimize_node`, `fix_node`, and
`clarify_contract`. Statuses are `open`, `working`, `proposal_pending`,
`consumed`, and `rejected`. Requests intentionally omit implementation choices
such as node kinds, scripts, and environments; those belong to the responding
proposal.

This should be a separate `plugin-rsi` store rather than overloading the skill
observation schema. Skill observations remain procedural knowledge; executable
system-change requests need richer lifecycle state.

### Proposal manifest

`proposal.toml` records identity, authorship, evidence, baseline, and the exact
validation result being considered:

```toml
schema_version = 1
proposal_id = "P-..."
plugin_name = "clusterprofiler-bitr"
action = "new_plugin"
status = "draft"
authored_by = "agent"
request_ids = ["R-..."]
environment_id = "bioconductor-r"
environment_reference = "ghcr.io/auto-nomics/environments/bioconductor-r@sha256:..."
baseline_source = "git:<sha>"
source_commit = "<40-hex-sha>"
remote = "git@github.com:auto-nomics/clusterprofiler-bitr-plugin.git"
test_report = "reports/....json"
rationale = "..."
rollback_baseline = "baseline/"
created_at = 1760000000
updated_at = 1760000000
```

For updates, the baseline is a complete snapshot of the installed source. For a
new family, it is empty. Approval archives the prior installed plugin before any
replacement, enabling rollback.

The `repo/` directory is initialized as a git repository when the proposal is
created, before the first file is written. The daemon-owned RSI worker is the
only component that creates commits; Agents request structured workspace writes
and validation runs, but never run git themselves. Every attempt leaves an
auditable commit plus a report. A proposal cannot enter review with a dirty
working tree or a source commit that does not match the reviewed report.

## 4. Agent Topology

Start with one RSI orchestrator that can delegate specialized children. The
first implementation does not need a fully autonomous peer network.

| Role | Responsibility | Key constraint |
| --- | --- | --- |
| Triage analyst | Cluster requests, classify intent, decide whether a plugin change is justified | Deterministic prefilter; explain evidence coverage |
| Contract architect | Define scientific input/output contract, parameters, docs, and node boundary | No scripts yet; manifest contract must pass validation |
| Plugin engineer | Draft or update manifest, scripts, fixtures, README, and test cases | Works only through proposal tools; no host shell |
| Evaluator | Run the validation pyramid, inspect failures, request focused revisions | Cannot edit proposal files |
| Release reviewer | Summarize diff, security posture, provenance, test evidence, and rollout risk | Can submit for review, never approve |

Recommended first profiles are `rsi/orchestrator`, `rsi/contract`,
`rsi/engineer`, and `rsi/evaluator`. Existing host tools can spawn and delegate
to these profiles. A release-review child can be added after the first
end-to-end path is stable.

## 5. Agent Tool Surface

Expose narrowly scoped `plugin_rsi_*` tools instead of generic host filesystem
or bash access:

- `plugin_rsi_requests`: list/open requests and linked evidence.
- `plugin_rsi_request_record`: create a structured executable-node request.
- `plugin_rsi_baseline_get`: copy an installed family into a proposal baseline.
- `plugin_rsi_proposal_create`: create a draft for a new or updated family.
- `plugin_rsi_file_write`: write one safe relative path under `plugin/`.
- `plugin_rsi_file_read`: read one proposal or baseline file.
- `plugin_rsi_diff`: return a structured baseline/proposal diff.
- `plugin_rsi_validate`: run manifest/static validation.
- `plugin_rsi_test`: run selected fixture/environment/DAG tests.
- `plugin_rsi_submit`: mark a tested proposal pending human review.

All paths are lexical relative paths under the proposal root. Symlinks and
absolute paths are rejected. File writes are bounded and atomic. Tools return
structured errors that the evaluator can feed back to the engineer.

## 6. Validation Pyramid

Each proposal must progress through deterministic gates. A test report is a
JSON artifact generated by infrastructure, never an Agent assertion.

1. **Manifest parse and validation**: unknown fields, digest-pinned
   environment, safe
   script paths, ports, outputs, params, templates, resources, and panels.
2. **Static adapter review**: strict script behavior, environment-based
   parameters, no host paths or unexplained egress, and complete outputs.
3. **Script fixture harness**: happy path, malformed input, missing key, invalid
   identifiers, valid empty result, and optional branches.
4. **Registry compile**: factory construction, schema and port compilation,
   representative specs, and invalid-spec rejection.
5. **DAG preview**: an isolated one-session registry and representative
   file-to-file DAG execution; no live session is touched.
6. **Regression**: baseline/proposal semantic comparison plus golden contract
   fields; breaking changes require explicit review.
7. **Clean-room release checks**: clean source, exact environment digest, panels,
   registry install, and smoke DAG.

For the first milestone, levels 1, 2, and 4 plus deterministic static checks can
run without a model. Levels 3 and 5 use the existing container runtime when
Podman and the pinned environment are available; unavailable infrastructure is an
explicit test-blocked state, never a pass.

## 7. Environment And Release Policy

The manifest's `[image]` field is an environment reference, not plugin-owned
artifact provenance. An ordinary plugin proposal:

1. selects an id from `EnvironmentCatalog`;
2. resolves that id to a digest-pinned environment reference;
3. writes the reference into `manifest.toml`;
4. stores only source, tests, fixtures, and documentation in its git repository;
5. never contains or builds a Dockerfile.

This separates environment control from business logic. The environment supplies
the interpreter and system dependencies; the plugin repository supplies the
adapter source. At release time, the immutable pair is a plugin source commit
and an already-approved environment digest.

New or modified environments are a separate future proposal type. An environment
proposal may include a Dockerfile and lock files, but it is built, tested,
pushed, and digest-pinned only by trusted infrastructure or an operator. It is
then registered in `EnvironmentCatalog` and reused by plugin proposals.

Agent-authored plugin proposals are never auto-approved. This is stronger than
the current skill policy because executable code and data access are involved.

## 8. Promotion And Installation

Phase 1 should deliberately require a daemon restart:

1. A human reviews the proposal, diff, evidence, and test report.
2. The human approves through gateway or TUI.
3. The trusted RSI publisher verifies that the repo is clean, the reviewed
   report passed, and the source commit is the reviewed commit.
4. The publisher checks `gh auth status`, creates the configured GitHub
   repository if needed, adds it as `origin`, pushes the default branch, and
   records the pushed commit SHA and clone URL.
5. Runtime updates `plugins.toml` with a git source and the full commit SHA.
6. The operator restarts the daemon after the immutable source is configured.
7. Existing startup sync, loader validation, and registry preflight execute as
   they do today.
8. Failure blocks startup with the existing fail-closed behavior; the baseline
   remains available for rollback.

For the production path, a published git source is mandatory: `path = "..."`
remains a development convenience, but a human-approved RSI release may not be
installed from an unpublished local directory. The install declaration is always
of this shape:

```toml
[[plugin]]
name = "clusterprofiler-bitr"
git = "git@github.com:auto-nomics/clusterprofiler-bitr-plugin.git"
rev = "<40-hex-commit-sha>"
```

### GitHub publisher

GitHub support should use the installed `gh` CLI through a daemon-owned
publisher, rather than embedding GitHub credentials or API tokens in the RSI
core or exposing `gh` to Agents. The CLI already carries the host credential and
SSH configuration, and it keeps this integration small and auditable.

The publisher supports four operations:

- `status`: run `gh auth status` and report whether repo creation/push is
  available.
- `prepare`: validate repository naming, collision, git identity, and clean
  state.
- `create`: run the equivalent of `gh repo create <owner>/<repo> --private`,
  then set `origin`.
- `push`: push the reviewed default branch, resolve the pushed commit SHA, and
  record the resulting source.

Only the publisher may execute these commands. The RSI Agent tools terminate at
`submit`; they cannot create repositories, push branches, modify remotes, read
credentials, or write `plugins.toml`.

Suggested runtime configuration:

```toml
[plugin_rsi.github]
owner = "auto-nomics"
visibility = "private"
repository_suffix = "-plugin"
default_branch = "main"
commit_author_name = "Autonomics RSI"
commit_author_email = "rsi@autonomics.local"
allow_repo_create = true
```

Repository names are derived from the validated plugin name, for example
`clusterprofiler-bitr` becomes
`git@github.com:auto-nomics/clusterprofiler-bitr-plugin.git`. Existing remote
names are never force-overwritten; a collision becomes a blocked release rather
than an implicit update.
5. Existing startup sync, loader validation, and registry preflight execute as
   they do today.
6. Failure blocks startup with the existing fail-closed behavior; the baseline
   remains available for rollback.

Hot reload can be added later as a registry-generation mechanism. It should not
mutate `NodeRegistry` in place: build a new registry, validate it, then switch
future sessions and explicit preview sessions to the new generation. Existing
clients can continue on their old snapshot or be explicitly invalidated. This is
a separate design milestone.

## 9. API And UI

### Gateway

- `GET /api/v1/plugin-rsi/requests`
- `POST /api/v1/plugin-rsi/requests`
- `GET /api/v1/plugin-rsi/proposals`
- `GET /api/v1/plugin-rsi/proposals/{id}`
- `GET /api/v1/plugin-rsi/proposals/{id}/diff`
- `POST /api/v1/plugin-rsi/proposals/{id}/validate`
- `POST /api/v1/plugin-rsi/proposals/{id}/test`
- `POST /api/v1/plugin-rsi/proposals/{id}/submit`
- `POST /api/v1/plugin-rsi/proposals/{id}/approve`
- `POST /api/v1/plugin-rsi/proposals/{id}/reject`
- `GET /api/v1/plugin-rsi/github/status`
- `POST /api/v1/plugin-rsi/proposals/{id}/publish`
- `POST /api/v1/plugin-rsi/proposals/{id}/install`

Agent tools and HTTP handlers should share one control worker, following the
skill evolution model. This gives ordered mutations, timeout handling, and one
audit path.

### TUI

Extend the evolution dashboard with a request inbox, proposal detail,
baseline/proposal diff, test-report pyramid, security and provenance summary,
and explicit approve/reject actions. The initial UI can be read-heavy: creation
and repair happen through the orchestrator, while humans inspect and gate
promotion.

## 10. Implementation Milestones

### M1: Proposal substrate

- Add a `plugin-rsi` crate for requests, proposals, atomic file operations,
  validation reports, lifecycle, and audit records.
- Add path safety, size limits, proposal idempotency, and rollback snapshots.
- Unit-test malicious paths, duplicate ids, status transitions, and malformed
  manifests.

### M2: Deterministic validator

- Add `container-plugin::validate_dir` around the existing loader logic.
- Produce structured reports for manifest parse, node validation, static checks,
  spec compilation, and factory registration.
- Add an isolated preview registry builder that accepts an explicit proposal
  directory and container infrastructure.

### M3: Request capture and triage

- Add the request store and `plugin_rsi_request_record`.
- Automatically capture qualifying DAG node failures and eval failures.
- Cluster by intent, family, node kind, and error signature; require explicit
  evidence before an orchestrator run.

### M4: Multi-agent authoring loop

- Add RSI profiles and the host-owned tools listed above.
- Implement triage, contract, implementation, validation, revision, and
  submission.
- Add a fixed attempt budget and require the evaluator to produce concrete
  failing checks rather than free-form retries.

### M5: Review and installation

- Add gateway and TUI review surfaces.
- Add the daemon-owned git snapshot layer and gh publisher.
- Verify clean-tree, reviewed-commit, pushed-SHA, and install-pin linkage.
- Add git-source installation handoff and restart-required semantics.
- Add rollback metadata and install audit events.

### M6: Optional registry generations

- Explore new-session-only registry switching.
- Add an invalidation policy for existing clients.
- Add concurrent-run and rollback tests before enabling hot installation.

## 11. Risks And Guardrails

| Risk | Guardrail |
| --- | --- |
| Prompt-injected code changes | Agents manipulate only daemon-owned proposal paths through structured tools |
| Unsafe adapter code | Static checks plus fixed container resources; scripts use environment parameters, not code interpolation |
| False test success | Reports are generated by infrastructure; missing Podman or an environment is blocked, not skipped |
| Model self-approval | Agent proposals are human-review-only by construction |
| Registry corruption | Live registry remains immutable; phase-one install occurs through startup validation |
| Irreversible update | Every update carries a baseline snapshot and history |
| Kind or contract collisions | Preview registry validates against installed kinds before review |
| Unbounded model loops | Attempt cap, deterministic failure evidence, and request-queue backpressure |
| Supply-chain drift | Release requires a plugin commit SHA plus an approved environment digest |
| Credential leakage | Only the daemon-owned publisher runs `gh`; Agents never receive tokens or git remotes |
| Accidental remote overwrite | Existing repository/remote collisions block publication instead of force-updating |
| Unreviewed source publication | Only an approved proposal with a clean tree and matching passing report can publish |

## 12. Open Questions

1. Should proposal workspaces also be exposed through the main VFS for easier
   human editing, or remain daemon-owned with only API/TUI access?
2. Which families should form the first adapter-only pilot: one small existing
   family plus one new deterministic utility family?
3. Should the first automatic trigger be explicit user request only, or repeated
   workflow failures as well?
4. Where should trusted environment CI live, and which registry credentials can
   it use without exposing them to agents?
5. Which GitHub owner and repository naming policy should be the default?

The conservative answer for the first production slice is: daemon-owned git
workspaces, adapter-only changes, explicit user-triggered RSI runs, and a
mandatory GitHub-published git source before installation.
