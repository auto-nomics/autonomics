# Autonomics GUI

Tauri 2 desktop frontend for the resident Autonomics gateway. Like the TUI, it is a
thin client: it never opens a second `RuntimeHost`, and agents continue running when
the window is closed.

## Run

Start the resident backend first:

```bash
autonomics serve
```

Then, from this directory:

```bash
npm install
npm run tauri dev
```

The GUI discovers `AUTONOMICS_HTTP_API_ADDR` and `AUTONOMICS_HTTP_API_TOKEN` first.
Without overrides it uses `127.0.0.1:8765` and reads `~/.autonomics/gateway.token`.

On Linux, the binary sets `WEBKIT_DISABLE_COMPOSITING_MODE=1` and
`WEBKIT_DISABLE_DMABUF_RENDERER=1` by default to avoid blank WebKitGTK windows on
NVIDIA drivers. Set either variable explicitly to override this behavior.

## Build

```bash
npm run tauri build --no-bundle
```

The executable is written to the workspace's `target/release/autonomics-gui`. Add
`--bundles app` or omit `--no-bundle` when creating an installable package.

## Implemented alignment

- Gateway hydration with consistent snapshot/SSE sequence handling
- Live agent list, sessions, transcript history, and persisted plan
- Streaming text/thinking, tools, usage, errors, and cancel/compact controls
- Provider catalogue, API-key editing, and default model activation
- Agent creation and new-session creation through the gateway
- Bibliography collections, article search, metadata, paged full text, and notes
- KMS skill governance, wiki browser, evidence browser, live worker status, and
  pending proposal approval/rejection
- Per-agent runtime overrides for memory injection, memory generation, and reasoning
  effort
- Agent-scoped DAG snapshot browsing with status coloring, selection, and edge focus
- Persisted thinking/tool display collapse settings with transcript filtering
- Embedding endpoint/model configuration
- Custom model add/edit/delete, provider remote-catalog import, and ChatGPT
  login/refresh controls
- Session create/activate/rename/close with active-session tracking from gateway
  lifecycle events
- Persisted-agent resume picker with search, rename, and typed delete confirmation
- Filterable message-copy picker with keyboard navigation and per-message copy
- Message fork picker backed by raw message IDs, creating a branch at the selected
  backend message
- Ctrl+P command palette covering navigation, agent/session actions, view toggles,
  clipboard/fork, compact/cancel, and configuration reload
- Per-agent shutdown and destructive delete controls in the live agent sidebar
- Provider base URL editing alongside API keys, plus ChatGPT subscription status
- Native full-text upload for PDF/HTML/TXT, with daemon-side extraction
- Citation export to BibTeX/RIS/Markdown/CSL JSON for the current collection or
  entire library
- Pending full-text request review and DOI/PMID/arXiv/other identifier import
- Per-agent composer drafts and transcript scroll restoration
- Composer Up/Down prompt recall and incremental Ctrl+R history search; queued
  sends stay enabled while an agent is running

On daemons that predate the dedicated `/kms/{knowledge,skills,skill-proposals,evidence}`
routes, KMS automatically falls back to `/skills/library` and `/kms/workers`; proposal
review remains read-only until the newer daemon endpoints are available.

The remaining major TUI-only area is deep authoring/editing of DAG graphs; the GUI
currently provides the same read-only runtime snapshot view.
