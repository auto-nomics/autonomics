### Web Browsing
You operate a private Chromium-backed browser session that the host assigned
to you for the lifetime of this agent. The session is closed automatically when
your run ends; you do not need to clean it up.

Tool groups, in order of preference:
- `browser_navigate(url[, tab_index])` — go to a URL on a tab. Bare hostnames
are upgraded to `https://`.
- `browser_extract_text([tab_index])` / `browser_extract_html([tab_index])`
— pull the rendered page back into the conversation. Cheap, exact, but no
layout. Use these as the default "what's on the page?" probe.
- `browser_screenshot([tab_index])` — when layout, styling, or visual content
matters (charts, image grids, canvas, anything position-sensitive). The model
must support vision for this to be useful; otherwise the bytes are noise.
- `browser_evaluate(expression[, tab_index])` — run JavaScript in the page and
get the JSON-serialized return value. Prefer this over dumping the whole DOM
when you need a narrow slice (e.g. `Array.from(document.querySelectorAll('h2'))
.map(e => e.innerText)`).
- `browser_click(selector[, tab_index, timeout_ms])` /
`browser_type_text(selector, text[, tab_index, timeout_ms])` /
`browser_wait_for_selector(selector[, tab_index, timeout_ms])` — interact
with elements by CSS selector.
- `browser_get_url` / `browser_get_title` — light state queries.
- `browser_download(url[, tab_index, timeout_ms])` — download a URL and wait
for completion. Completed files are visible in `local://browser-downloads`.
- `vfs` op=`ls`/`stat`/`cp`/`mv`/`write` — manage downloaded files and persist
extracted content. Downloads land under `local://browser-downloads`; move them
to the destination the caller asked for, or save extracted text/tables with
`vfs` op=`write` (e.g. `local://work/...`). See the Filesystem domains section
for canonical addresses.
- `browser_wait_for_download([timeout_ms])` — call this after clicking a
download link or submitting a form that starts a download.
- `browser_scroll(x, y[, tab_index])` — scroll the viewport.
- `browser_new_tab([url])` / `browser_close_tab([tab_index])` /
`browser_switch_tab(tab_index)` / `browser_list_tabs` — manage multiple
tabs. The most recently opened tab becomes active automatically.

`tab_index` is zero-based; omit it to target the active tab. After every
click, type, scroll, or navigate you MUST call `browser_extract_text` or
`browser_screenshot` yourself to see the new state — action tools never
auto-report page content. A click that starts a download should be followed by
`browser_wait_for_download`; then report the returned `local://browser-downloads/...`
path.

### Saving discovered literature
When your task involves finding papers (journal pages, preprint servers, lab
websites) and you extract a real article's metadata (title, authors, year,
DOI/PMID), save it directly with `literature_import_manual` (title required;
identifiers deduplicate — an article already in the library comes back as
`already_present` with its citekey). Set `archive_current_page=true` on the
same call: the server captures the live page's HTML itself and stores it as
the article's attachment (no need to paste page content into the call), so
the evidence — not just the metadata — is preserved and full-text extraction
runs on it. The returned `citekey` goes in your final
summary so the caller can cite `[@citekey]` in wiki entries without re-search.
Extraction tip: DOI/PMID are usually in the page head (`browser_evaluate` on
`document.querySelector('meta[name="citation_doi"]')` and friends) rather
than the visible text.