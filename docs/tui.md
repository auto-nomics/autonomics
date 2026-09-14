# Autonomics TUI (`autonomics-tui`)

[English](tui.md) | [中文](tui_zh.md)

Interactive terminal user interface for the Autonomics agent platform — chat with an
LLM-backed agent, configure model providers, manage OpenGWAS cache, and curate a local
bibliography library, all from a single `ratatui`-based binary.

## Features

- **Local HTTP API** — starts with the TUI and exposes the versioned `/api/v1`
  backend; bibliography management is the first module. See
  [TUI HTTP API](tui-http-api_zh.md) (Chinese).
- **Agent chat** — Streaming conversation with tool-calling agents built on
  `agentik-core`. Live Markdown rendering (with syntax highlighting via `syntect`),
  thinking deltas, per-turn token usage, background tool tasks, and a scrollable
  transcript with incremental history search (`Ctrl+R`).
- **Model configuration** — Tree-based provider catalogue. Activate any built-in
  model by entering its API key; the change is persisted to `phloem.db` and
  hot-swapped into the running agent (no restart).
- **Bibliography management** — `bib` subcommand: upload full-text PDFs (with
  automatic text extraction), list pending full-text requests, inspect article
  status, search the local library, and export to BibTeX / RIS / Markdown / CSL JSON.
- **OpenGWAS cache** — `cache` subcommand: refresh or clear the on-disk SQLite
  snapshot of the OpenGWAS `gwasinfo` catalogue.
- **KMS tree browser** — `kms` subcommand: dedicated ratatui workspace for the
  Turso-backed knowledge tree in `agent.db`, with tree navigation,
  knowledge/entity inspection, and live diagnostics.

## Build & run

```bash
# Build (from workspace root)
cargo build -p tui

# Launch the interactive TUI (default subcommand)
cargo run -p tui
# …or after installing the binary
autonomics-tui

# Pass a TUI configuration file
autonomics-tui tui --config /path/to/config.toml
```

The binary name in the workspace is `tui`; install/copy it as `autonomics-tui` if
you want the CLI name shown below. Both names refer to the same artifact.

## CLI overview

```
autonomics-tui [OPTIONS] [COMMAND]

Commands:
  tui     Launch the interactive TUI (default when no subcommand is given)
  serve   Run the resident backend gateway daemon (see below)
  run     Headless one-shot agent run (docs/headless-run-design.md)
  kms     Launch the dedicated KMS tree browser
  cache   Local cache management helpers (refresh, inspect, purge)
  bib     Bibliography management — upload full-text PDFs, list pending requests
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## Architecture: thin client + resident gateway

The TUI is a thin client. A single resident daemon — `tui serve` — owns the
`RuntimeHost`, the app/model databases, and the local HTTP API; every
frontend (the TUI today; web/desktop later) connects to it over
REST + SSE. Quitting the TUI does **not** stop agents: they keep running in
the daemon, and reopening the TUI rehydrates sessions and transcripts.

```bash
autonomics-tui serve          # run the daemon in the foreground
autonomics-tui serve status   # is it running? (pid, uptime, agent count)
autonomics-tui serve stop     # graceful shutdown (stops agents)
```

Launching the TUI auto-spawns `tui serve --daemon` when no daemon is
running. Full protocol and design: `docs/design/gateway-architecture.md`.

## KMS TUI

Launch the dedicated knowledge-tree browser using the runtime's default
`agent.db`:

```bash
autonomics-tui kms
autonomics-tui kms --agent-db /path/to/agent.db
```

Layout:

```
┌───────────────┬──────────────────────────────┐
│ Tree          │ Knowledge / Entity           │
├───────────────┼──────────────────────────────┤
│ Diagnostics   │ status + keybindings         │
└───────────────┴──────────────────────────────┘
```

| Key | Action |
|-----|--------|
| `Tab` | Cycle Tree / Knowledge / Diagnostics focus |
| `j` / `k` | Move selection or scroll focused panel |
| `Space` / `Enter` | Expand or collapse the selected tree group |
| `t` | Toggle Knowledge and Entity details |
| `r` | Reload tree, knowledge, entities, and diagnostics |
| `q` or `Esc` | Quit |

---

## Interactive TUI

### Layout

```
┌─────────────────────── TabBar ───────────────────────┐
│  [Agent]   Config                                    │
├──────────────────────────────────────────────────────┤
│  status bar (model · tokens · agent status)          │
│  background tool tasks (when running)                │
│                                                       │
│  chat transcript (scrollable, Markdown-rendered)      │
│                                                       │
│  ❯ input composer (borderless, auto-growing)          │
│  footer keybinding hints                             │
└──────────────────────────────────────────────────────┘
```

Two tabs: **Agent** and **Config**. The default tab is Agent.

### Global keybindings

| Key | Action |
|-----|--------|
| `]` / `[` | Switch to next / previous tab |
| `Ctrl+C` | Cooperative cancel (first press) → force quit (second press within 3 s) |

### Agent tab

The Agent tab has two input modes — **Browse** (default) and **Input** (composer).

**Browse mode — scrolling the transcript**

| Key | Action |
|-----|--------|
| `↑` / `↓` | Scroll one line up / down |
| `PageUp` / `PageDown` | Scroll half a page (12 lines) |
| `Home` | Jump to top |
| `End` | Jump to bottom (re-enables auto-scroll) |
| `Enter` | Enter the composer (Input mode) |
| Mouse wheel | Scroll; releases auto-scroll lock |
| `Ctrl+G` | Toggle auto-scroll-to-bottom lock |

**Input mode — composing messages**

| Key | Action |
|-----|--------|
| Type | Insert text into the composer |
| `Enter` | Send message (returns to Browse) |
| `Shift+Enter` / `Alt+Enter` | Insert newline (multiline compose) |
| `Esc` | Leave composer without sending |
| `↑` / `↓` | Recall previous / next prompt from history |
| `Ctrl+R` | Start incremental history search (type to filter, `↑`/`↓` to navigate, `Enter` to accept, `Esc` to cancel) |

The composer is an Emacs-style textarea (via `xai_textarea`): full Emacs keybindings,
undo/redo, mouse selection, and an internal scrollbar. Word wrap is automatic; the
box grows up to 10 rows and then scrolls internally.

While the agent is running, the composer is disabled and a placeholder is shown.

**Chat transcript rendering**

- User messages, assistant responses (streamed), thinking blocks, tool calls &
  results, background task completions, and errors are each rendered as distinct
  block types.
- Assistant messages render as Markdown with syntax-highlighted code blocks.
- Per-turn token usage (`input`, `output`, `cache_read`) is attached below each
  assistant message and accumulated in the status bar.
- Background tool tasks appear in a dedicated strip above the chat area while
  running, then collapse when complete.

### Config tab

A **tree view** of the built-in provider catalogue (from
`agentik_sdk::provider::registry`):

```
▼ deepseek ✓                ← configured, expanded
  ● deepseek-v4-pro         ← active model (green ●)
    deepseek-v4-flash
▶ mimo ✓                    ← configured, collapsed
── minimax ✗                ← unconfigured (dimmed)
```

| Key | Action |
|-----|--------|
| Type / paste | Edit the filter query with cursor motion and clipboard paste |
| `↑` / `↓` (or `k` / `j`) | Move cursor |
| `→` / `←` (or `l` / `h`, `Tab` / `BackTab`) | Expand / collapse provider node |
| `Enter` | Activate the model under the cursor |
| `Ctrl+D` | Set the model under the cursor as the default for new agents |
| `Ctrl+E` | Add or edit the provider's API key (opens credential panel) |
| `Ctrl+F` | Fetch the provider's live remote model catalogue (OpenRouter's public `/v1/models`; imported models persist in the DB) |
| `Ctrl+R` | Reload the catalogue from DB + SDK registry |

In the **credential panel** (right side), type the API key into the textarea:

| Key | Action |
|-----|--------|
| Type / paste | Enter API key |
| `Enter` | Save to DB (`providers` table) |
| `Esc` | Cancel without saving |

Activating a model persists `provider:model` to the `settings` table and atomically
hot-swaps the `Model` inside the agent runtime — the next turn uses the new model
with no restart.

---

## `cache` subcommand

Manages the on-disk OpenGWAS `gwasinfo` SQLite cache.

```
autonomics-tui cache <ACTION>

Actions:
  refresh-opengwas   Re-fetch the OpenGWAS gwasinfo catalog from the remote API
  clear-opengwas     Delete the on-disk OpenGWAS gwasinfo cache file
```

**Refresh**

```bash
autonomics-tui cache refresh-opengwas [--show-cache-path]
```

Fetches the full catalogue and overwrites the SQLite snapshot. Requires
`OPENGWAS_TOKEN`.

**Clear**

```bash
autonomics-tui cache clear-opengwas [-y]
```

Deletes the cache file. The next query in a TUI session triggers a fresh fetch.
Prompts for confirmation unless `-y` is passed.

**Cache directory resolution**

1. `$OPENGWAS_CACHE_DIR` (if non-empty)
2. `$HOME/.cache/opengwas`
3. `$TMPDIR/opengwas` (fallback)

---

## `bib` subcommand

Local bibliography library management. All subcommands accept `--db <PATH>`
(default `bib.db`).

```
autonomics-tui bib [OPTIONS] <ACTION>

Actions:
  upload    Upload a PDF (or other document) as the full text for an article
  requests  List articles that have been marked as needing full-text upload
  info      Show the current full-text status for a specific article
  list      List all articles in the library
  export    Export articles in a citation format (BibTeX, RIS, Markdown)
```

### `upload` — upload full text

```bash
autonomics-tui bib upload \
  --pdf <PATH> \
  --article-id <ID> \
  [--collection-id <CID>]
```

Reads the file, detects the format from the extension (`pdf` / `html` / `txt`),
extracts plain text via `SimpleExtractor`, and stores the original bytes in the
runtime bibliography VFS using a SHA-256 content-addressed path. PDF extraction
also tries the optional local `tesseract` command when the built-in extractor
returns no text. Extraction failure does not reject the upload; the original
remains downloadable and the text column is left empty. If `--collection-id` is given, the article's
`fetch_status` in that collection is updated to `fulltext_available`.

### `requests` — pending full-text requests

```bash
autonomics-tui bib requests [--collection-id <CID>]
```

Lists all articles with `fetch_status = fulltext_requested`. Output includes
article ID, title, DOI, year, collection, and note.

### `info` — article detail

```bash
autonomics-tui bib info --article-id <ID>
```

Prints the article's bibliographic metadata (title, journal, year, DOI) and —
if a full text has been uploaded — its format, source, file path, byte size, and
a 200-character text preview.

### `list` — search / list the library

```bash
autonomics-tui bib list [--query <KEYWORDS>] [--limit <N>]
```

`--query` matches title + abstract. Default limit 50 (max 500).

### `export` — citation export

```bash
autonomics-tui bib export \
  --format <bibtex|ris|markdown|csl_json> \
  [--collection-id <CID>] \
  [--output <PATH>] \
  [--limit <N>]
```

Export a collection or the entire library. Without `--output`, the rendered
citations are written to stdout.

---

## Environment variables

| Variable | Purpose |
|----------|---------|
| `OPENGWAS_TOKEN` | Required for `cache refresh-opengwas` and any OpenGWAS agent query |
| `OPENGWAS_CACHE_DIR` | Override the OpenGWAS cache directory |
| `RUST_LOG` | Override the default tracing filter (defaults to `tui=debug,agentik_core=debug,agentik_sdk=debug,runtime=debug,nodes_ldsc=debug,ldsc=debug`) |

## On-disk artifacts

| Path | Contents |
|------|----------|
| `phloem.db` | TUI configuration database (`providers`, `models`, `settings` tables) |
| `bib.db` | Bibliography library (articles, collections, full-text records) |
| `logs/autonomics-tui.log.<date>` | Daily-rotated tracing log |

## Architecture notes

- **Main loop** — `tokio::select!` driven, with a fixed-rate ~60 fps render tick
  (`Skip` missed-tick policy). Event branches only mutate state and set a dirty
  flag; rendering happens solely in the tick branch. Idle → zero CPU.
- **State / render separation** — `AppState` holds all mutable state; widgets are
  stateless and read state via `StatefulWidgetRef`. A per-message content-version
  cache (`msg_versions` + `cached_msg_lines`) skips re-rendering unchanged messages.
- **Hot-swap model** — `active_model: Arc<ArcSwapOption<Model>>` lets the Config
  tab swap the model atomically; the agent reads the latest model on each turn.
- **Panic safety** — A panic hook restores the terminal before propagating, so a
  crash never leaves the shell in raw/alternate-screen mode.

## Source map

| File | Responsibility |
|------|----------------|
| `src/main.rs` | CLI definition (clap), subcommand dispatch, `bib` implementation |
| `src/app.rs` | `App` struct, async main loop, event handling, model hot-swap |
| `src/state.rs` | All TUI state types + `AgentEvent → state` reducer |
| `src/config_db.rs` | SQLite CRUD for the `providers` / `models` / `settings` tables |
| `src/widgets/` | Ratatui widgets (agent tab, chat, status bar, input, model config, tool exec) |
| `src/xai_textarea/` | Embedded multiline textarea (Emacs keybindings, wrapping, undo/redo) |
