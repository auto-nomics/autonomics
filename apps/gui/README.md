# Autonomics GUI

Gateway-hosted web frontend and Tauri 2 desktop shell for the resident Autonomics
gateway. Like the TUI, it is a thin client: it never opens a second `RuntimeHost`,
and agents continue running when the window is closed.

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

## Gateway-hosted web client

`npm run build:gateway` builds the browser-compatible bundle and syncs it to
`crates/gateway/gui-dist`. The gateway embeds those files with `include_str!`
and serves them automatically when the daemon starts:

```text
http://127.0.0.1:8765/
```

The GUI index handler injects the loopback gateway bearer token into the page.
Static JS/CSS are metadata-only and unauthenticated; all API requests remain
bearer-guarded. Tauri uses the same frontend and switches to its native bridge
and dialogs when `__TAURI_INTERNALS__` is present.

## Source layout

`src/` is organized by domain. Dependencies only point downward:
`app → features/* → gateway/types/lib/components`, and between features only
through each domain's `index.ts` barrel (`discovery → bibliography`,
`bibliography → reader`).

```text
src/
  main.tsx               entry: mounts <App/>
  app/                   shell + agent runtime store
    app-view.tsx         navigation, view switch, workspace hosting, login/boot screens
    use-agent-runtime.ts gateway connection, event reducer, agent/session actions
  gateway.ts             HTTP/SSE client, the only backend access point
  types.ts               shared wire types
  features/
    agent/               agent view, floating workspace, layout persistence
    bibliography/        library view, search model + panels, collection tree
    discovery/           federated discovery, subscriptions, snapshots, citation exploration
    reader/              PDF/TXT readers, annotations, reading mode (opened from bibliography)
    system/              config, KMS, DAG views
  components/ui/         shadcn-style primitives (no domain logic)
  lib/                   markdown rendering and misc utilities
  hooks/                 shared React hooks
  vendor/                generated third-party code (zotero-sdt)
```

Playwright tests in `tests/` import modules by path (`/src/features/...` or
`@/...`); the four reader tests spin up a bare Vite server and therefore pass
the `@` alias explicitly.

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


## Research workbench and conversation references

The activity rail opens Agent, Bibliography, Knowledge and Settings in the central
work area. Bibliography and Knowledge stay mounted after their first visit, so
selections, searches and scroll positions survive module navigation. On wide
screens the Agent assistant docks in a resizable auxiliary pane. Its width and
placement are remembered; the header also offers floating and hidden modes.
Below 1100px, a docked assistant uses the existing floating shell.

PDF/TXT readers opened from the workbench stay inside the literature editor,
leaving the Agent pane available. Standalone reader entry points retain their
full-window behavior.

Use **加入当前会话** on a paper, search hit, reader text selection, knowledge
node, evidence record or skill to explicitly add a reference. The basket above
the composer shows the current Agent/session and supports preview, removal,
replacement and clear. Adding a reference reveals the assistant, including a
previously collapsed floating shell. Each Agent/session has an independent
basket, held in memory for this application lifetime (reloading clears it).

References are snapshots: papers include metadata/abstract, search hits include
matched snippets, selections include only selected text and location, and KMS
records include their displayed body and provenance. Browsing does not change
references. Re-adding an object updates its snapshot. Each message appends the
basket as labeled JSON reference data in the existing user-message text; no new
backend API is required. References stay selected after sending, including on
failure; clear or remove them to stop attaching them to subsequent messages.

Run `npm run test:workbench` for the complete mocked-gateway browser workflow,
including hosted reading, session isolation, request payloads, failed sends,
module continuity, pane resizing and responsive placement.
