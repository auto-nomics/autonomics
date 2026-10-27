import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1428';
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1428', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser, activePage;
vite.stdout.on('data', value => output += value); vite.stderr.on('data', value => output += value);
const article = (id, title) => ({ id, title, year: 2024, abstract_text: 'Evidence about gene editing.', authors: [{ name: 'Ada Researcher' }], identifiers: [{ kind: 'doi', value: '10.1000/' + id }] });
const first = article('paper-a', 'Fast-source paper');
const second = article('paper-a', 'Slow-source curated variant');
function candidate(merged = false) {
  const record = (source, article) => ({ source, article, source_rank: 1, retrieved_at: '2026-10-28T10:00:00Z', executed_parameters: { term: 'gene' }, query_translation: 'Gene[Title/Abstract]' });
  return { id: 'candidate-a', fingerprint: merged ? 'merged-fingerprint' : 'initial-fingerprint', article: first, records: [record('pubmed', first), ...(merged ? [record('openalex', second)] : [])], first_seen: 0, rrf_score: .01, conflicts: merged ? [{ field: 'title', variants: [{ source: 'pubmed', value: first.title }, { source: 'openalex', value: second.title }] }] : [], related_versions: [], possible_duplicates: [] };
}
try {
  for (let attempt = 0; !output.includes(origin); attempt++) {
    if (vite.exitCode !== null || attempt > 100) throw new Error(output);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || ['/usr/bin/chromium', '/opt/google/chrome/chrome'].find(existsSync), args: ['--no-sandbox'] });
  for (const native of [false, true]) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } }); activePage = page; page.setDefaultTimeout(10000);
    const errors = [], calls = [], sessions = new Map();
    page.on('pageerror', error => errors.push(error.message));
    if (!native) await page.addInitScript(() => Object.defineProperty(crypto, 'randomUUID', { value: undefined }));
    if (native) await page.addInitScript(() => {
      const pending = new Map();
      window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
        if (command === 'cancel_api_request') { pending.get(args.requestId)?.abort(); return; }
        if (command !== 'api') throw new Error('Unexpected command: ' + command);
        const abort = new AbortController(); if (args.requestId) pending.set(args.requestId, abort);
        try {
          const response = await fetch('/api/v1' + args.path, { method: args.method, signal: abort.signal, headers: { 'Content-Type': 'application/json', ...(args.path === '/bib/search/sessions' && args.method === 'POST' ? { 'x-discovery-session-id': args.requestId } : {}) }, body: args.body == null ? undefined : JSON.stringify(args.body) });
          const text = await response.text(), data = text ? JSON.parse(text) : null;
          if (!response.ok) throw new Error(data?.error || String(response.status));
          return { data };
        } finally { pending.delete(args.requestId); }
      } };
    });
    await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html><body></body></html>' }));
    await page.route('**/api/v1/**', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname.replace('/api/v1', ''), body = request.postDataJSON();
      calls.push({ path, query: Object.fromEntries(url.searchParams), method: request.method(), body, headers: request.headers() });
      const reply = (json, status = 200) => route.fulfill({ json, status }).catch(() => {});
      if (path === '/bib/collections') return reply({ collections: [{ id: 'target', name: 'Research project', library_id: 1, article_ids: [] }, { id: 'readonly', name: 'Read-only group', library_id: 2, article_ids: [] }] });
      if (path === '/bib/health') return reply({ article_count: 1 });
      if (path === '/bib/articles') return reply({ articles: [article('local', 'Local library paper')], total: 1 });
      if (path === '/bib/search/sources') return reply({ sources: ['pubmed', 'openalex', 'arxiv', 'crossref', 'semantic_scholar', 'biorxiv'].map(source => ({ source, searchable: source !== 'biorxiv', native_query: source !== 'biorxiv', notes: [], credentials_configured: false, authentication: source === 'arxiv' ? 'none' : ['pubmed', 'crossref', 'semantic_scholar'].includes(source) ? 'optional' : 'required_for_continuation', quota: null })) });
      if (path === '/bib/search/sessions' && request.method() === 'POST') {
        const id = request.headers()['x-discovery-session-id'];
        assert.match(id, /^[0-9a-f-]{36}$/);
        const status = source => ({ source, phase: source === 'pubmed' ? 'ready' : 'running', total: 2, delivered_records: source === 'pubmed' ? 1 : 0, pages: 1, has_more: source === 'pubmed', failure: null, notices: [], plan: { parameters: { term: 'gene' }, notices: [] }, executed_parameters: { term: 'gene' }, query_translation: 'Gene[Title/Abstract]' });
        const state = { id, sequence: 1, phase: 'running', merged: false, request: body, sources: body.sources.map(status), deleted: false };
        if (body.query === 'paging fixture') {
          state.phase = 'idle';
          Object.assign(state.sources.find(source => source.source === 'openalex'), { phase: 'failed', failure: { kind: 'rate_limited', message: 'Fixture upstream cooldown', retryable: true, retry_after_seconds: 1 } });
        }
        if (body.query === 'candidate paging fixture') {
          state.large = true; state.phase = 'idle';
          state.sources.forEach(source => Object.assign(source, { phase: 'exhausted', total: 1001, delivered_records: 1001, has_more: false }));
        }
        sessions.set(id, state);
        return reply({ session: { ...state, merged_candidates: state.large ? 1001 : 1 } }, 201);
      }
      const match = path.match(/^\/bib\/search\/sessions\/([^/]+)(.*)$/);
      if (match) {
        const [, id, tail] = match, state = sessions.get(id);
        if (request.method() === 'DELETE') { if (state) state.deleted = true; return route.fulfill({ status: 204 }).catch(() => {}); }
        if (!state || state.deleted) return reply({ error: 'session expired' }, 404);
        const snapshot = () => ({ session: { ...state, merged_candidates: state.large ? 1001 : state.extra ? 2 : 1 } });
        if (!tail) return reply(snapshot());
        if (tail === '/events') {
          await new Promise(resolve => setTimeout(resolve, 60));
          const after = Number(url.searchParams.get('after'));
          return reply({ sequence: state.sequence, reset_required: false, events: state.history ? state.history.filter(event => event.sequence > after) : after < state.sequence ? [{ type: 'batch', sequence: state.sequence }] : [] });
        }
        if (tail === '/results') {
          if (state.large) {
            const offset = Number(url.searchParams.get('offset'));
            if (url.searchParams.has('sequence') && Number(url.searchParams.get('sequence')) !== state.sequence) return reply({ error: 'Fixture result snapshot changed' }, 409);
            const candidates = Array.from({ length: Math.min(100, 1001 - offset) }, (_, relative) => {
              const index = offset + relative, value = article('obtained-' + index, 'Obtained candidate ' + index), base = candidate();
              return { ...base, id: value.id, article: value, fingerprint: 'fingerprint-' + index, records: [{ ...base.records[0], article: value, source_rank: index + 1 }] };
            });
            return reply({ candidates, obtained_candidates: 1001, next_offset: offset + candidates.length < 1001 ? offset + candidates.length : null, sequence: state.sequence });
          }
          const extra = { ...candidate(), id: 'candidate-page-2', fingerprint: 'page-2', article: article('page-2', 'Next-page source paper') };
          extra.records = [{ ...extra.records[0], article: extra.article, source_rank: 2 }];
          return reply({ candidates: [candidate(state.merged), ...(state.extra ? [extra] : [])], obtained_candidates: state.extra ? 2 : 1, next_offset: null, sequence: state.sequence });
        }
        if (tail === '/sources/pubmed/next') {
          state.nextAttempts = (state.nextAttempts ?? 0) + 1;
          if (state.nextAttempts === 1) return reply({ error: 'Fixture candidate cache capacity reached' }, 503);
          state.history = [{ type: 'batch', sequence: state.sequence }, { type: 'source_started', sequence: state.sequence + 1 }];
          Object.assign(state.sources.find(source => source.source === 'pubmed'), { phase: 'running' });
          state.phase = 'running'; state.sequence++;
          return reply(snapshot());
        }
        if (tail === '/sources/openalex/retry') {
          Object.assign(state.sources.find(source => source.source === 'openalex'), { phase: 'exhausted', failure: null });
          state.sequence++;
          return reply(snapshot());
        }
        if (tail === '/cancel') { state.sources.forEach(source => { if (!body.source || body.source === source.source) source.phase = 'cancelled'; }); state.phase = 'idle'; state.sequence++; return reply(snapshot()); }
        if (tail === '/import/preview' || tail === '/import') {
          if (state.large && tail.endsWith('/preview')) return reply({ preview: { library_id: 1, collection_id: null, editable: true, items: body.selections.map(item => ({ candidate_id: item.candidate_id, existing: [], related_existing: [], differences: [], error: null })) } });
          if (body.selections[0].expected_fingerprint !== candidate(state.merged).fingerprint) return reply({ error: 'candidate metadata changed; refresh the import preview' }, 409);
          if (tail.endsWith('preview')) return reply({ preview: { library_id: 1, collection_id: body.collection_id, editable: body.collection_id !== 'readonly', items: [{ candidate_id: 'candidate-a', existing: state.imported && body.collection_id !== 'readonly' ? [second] : [], related_existing: [], differences: [], error: null }] } });
          state.imported = true;
          return reply({ imported: 1, failed: 0, items: [{ candidate_id: 'candidate-a', status: 'imported', article: second, error: null }], acquisition: null });
        }
        throw new Error('Unexpected discovery path: ' + tail);
      }
      return reply({});
    });
    await page.goto(origin);
    await page.evaluate(async () => {
      await import('/src/app.css');
      const { default: RefreshRuntime } = await import('/@react-refresh');
      RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => type => type; window.__vite_plugin_react_preamble_installed__ = true;
      const { mountBibliography } = await import('/tests/bibliography-menu-mount.tsx'); mountBibliography(document.body);
    });
    const navigation = page.getByRole('navigation', { name: 'Bibliography workspace' });
    await page.getByRole('button', { name: /Local library paper/ }).waitFor();
    await navigation.getByRole('button', { name: 'Discover', exact: true }).click();
    await page.getByLabel('biorxiv (unavailable for discovery)', { exact: true }).waitFor();
    assert.equal(await page.getByLabel('biorxiv (unavailable for discovery)', { exact: true }).isDisabled(), true);
    await page.getByRole('textbox', { name: 'Discover literature' }).fill('gene');
    assert.equal(calls.filter(call => call.path === '/bib/search/sessions').length, 0, 'external queries require explicit submission');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    const state = [...sessions.values()].at(-1);
    assert.equal(state.request.allow_approximate, false, 'approximate source matching is opt-in');
    assert.equal(state.phase, 'running', 'fast result arrives while slow source is still running');
    await page.getByLabel(`Select ${first.title}`, { exact: true }).check();
    await page.getByLabel('Import target collection').selectOption('target');
    await page.getByRole('button', { name: 'Preview import', exact: true }).click();
    await page.getByRole('button', { name: 'Confirm import', exact: true }).waitFor();
    state.merged = true; state.phase = 'idle'; state.sequence++;
    state.sources[1].phase = 'exhausted'; state.sources[1].delivered_records = 1;
    await page.getByRole('button', { name: 'Show updated results', exact: true }).waitFor();
    assert.equal(await page.getByLabel(`Select ${first.title}`, { exact: true }).isChecked(), true, 'incremental arrival preserves the selected snapshot');
    await page.getByRole('button', { name: 'Confirm import', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: 'candidate metadata changed' }).waitFor();
    await page.getByRole('button', { name: 'Show updated results', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).click();
    await page.getByLabel('Candidate source version').selectOption('1');
    await page.getByRole('heading', { name: second.title, exact: true }).waitFor();
    await page.getByRole('button', { name: 'Preview import', exact: true }).click();
    await page.getByRole('button', { name: 'Confirm import', exact: true }).click();
    await page.getByText('1 imported · 0 already present · 0 failed', { exact: true }).waitFor();
    await page.getByText('Already in target library', { exact: true }).waitFor();
    const imported = calls.findLast(call => call.path.endsWith('/import') && call.body?.selections?.[0]?.expected_fingerprint === 'merged-fingerprint');
    assert.equal(imported.body.selections[0].record_index, 1); assert.equal(imported.body.collection_id, 'target');
    await page.getByLabel('Import target collection').selectOption('readonly');
    await page.getByRole('button', { name: 'Preview import', exact: true }).click();
    await page.getByText('Target library is read only.', { exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: 'Confirm import', exact: true }).isDisabled(), true);
    await page.getByLabel('Import target collection').selectOption('target');
    await navigation.getByRole('button', { name: 'Library', exact: true }).click();
    await page.getByRole('button', { name: /Local library paper/ }).waitFor();
    await navigation.getByRole('button', { name: 'Discover', exact: true }).click();
    assert.equal(await page.getByRole('textbox', { name: 'Discover literature' }).inputValue(), 'gene');
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    await page.screenshot({ path: `/tmp/discovery-workflow-${native ? 'native-bridge' : 'browser'}.png` });
    await page.setViewportSize({ width: 760, height: 900 });
    await page.getByRole('button', { name: 'Close candidate details', exact: true }).click();
    assert.ok(await page.getByLabel('Discovery result list', { exact: true }).evaluate(node => node.clientHeight) > 150, 'narrow layout reserves space for results');
    await page.getByRole('button', { name: 'Source status', exact: true }).click();
    await page.getByRole('button', { name: 'Close source status', exact: true }).click();
    await page.getByRole('textbox', { name: 'Discover literature' }).press('Control+f');
    assert.equal(await page.getByRole('textbox', { name: 'Discover literature' }).evaluate(node => node === document.activeElement), true, 'hidden library must not steal search focus');
    await page.screenshot({ path: `/tmp/discovery-workflow-narrow-${native ? 'native-bridge' : 'browser'}.png` });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.getByRole('checkbox', { name: 'Native query', exact: true }).check();
    await page.getByLabel('Native source parameters').fill('{"term":"gene[Title]"}');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    const last = calls.filter(call => call.path === '/bib/search/sessions').at(-1);
    assert.deepEqual(last.body.native, { source: 'pubmed', parameters: { term: 'gene[Title]' } });
    assert.equal(state.deleted, true, 'new query deletes the previous session');
    await page.getByRole('radio', { name: 'arxiv', exact: true }).check();
    assert.equal(await page.getByRole('radio', { name: 'pubmed', exact: true }).isChecked(), false, 'native mode switches to a single source');
    await page.getByText('arxiv: public API · no key needed', { exact: true }).waitFor();
    assert.deepEqual(JSON.parse(await page.getByLabel('Native source parameters').inputValue()), { search_query: 'gene' });
    await page.getByLabel('Native source parameters').fill('{"search_query":"ti:gene ANDNOT au:Smith"}');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    assert.deepEqual(calls.filter(call => call.path === '/bib/search/sessions').at(-1).body.native, { source: 'arxiv', parameters: { search_query: 'ti:gene ANDNOT au:Smith' } });
    await page.getByRole('radio', { name: 'pubmed', exact: true }).check();
    assert.deepEqual(JSON.parse(await page.getByLabel('Native source parameters').inputValue()), { term: 'gene[Title]' }, 'switching sources restores its native draft without reinterpretation');
    await page.getByRole('radio', { name: 'arxiv', exact: true }).check();
    assert.equal(JSON.parse(await page.getByLabel('Native source parameters').inputValue()).search_query, 'ti:gene ANDNOT au:Smith');
    await page.getByRole('radio', { name: 'crossref', exact: true }).check();
    await page.getByText('crossref: API key optional', { exact: true }).waitFor();
    assert.deepEqual(JSON.parse(await page.getByLabel('Native source parameters').inputValue()), { 'query.bibliographic': 'gene' });
    await page.getByLabel('Native source parameters').fill('{"query.bibliographic":"gene editing","filter":"type:journal-article"}');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    assert.deepEqual(calls.filter(call => call.path === '/bib/search/sessions').at(-1).body.native, { source: 'crossref', parameters: { 'query.bibliographic': 'gene editing', filter: 'type:journal-article' } });
    await page.getByRole('radio', { name: 'arxiv', exact: true }).check();
    await page.getByRole('radio', { name: 'crossref', exact: true }).check();
    assert.equal(JSON.parse(await page.getByLabel('Native source parameters').inputValue())['query.bibliographic'], 'gene editing', 'Crossref native draft survives source switches');
    await page.getByRole('radio', { name: 'semantic_scholar', exact: true }).check();
    assert.deepEqual(JSON.parse(await page.getByLabel('Native source parameters').inputValue()), { query: 'gene' });
    await page.getByLabel('Native source parameters').fill('{"endpoint":"bulk","query":"gene -mice","year":"2020-2024"}');
    await page.getByLabel('Discovery source order').selectOption('newest');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    const s2request = calls.filter(call => call.path === '/bib/search/sessions').at(-1).body;
    assert.equal(s2request.sort, 'newest', 'source ordering is part of the executed query');
    assert.deepEqual(s2request.native, { source: 'semantic_scholar', parameters: { endpoint: 'bulk', query: 'gene -mice', year: '2020-2024' } });
    await page.getByRole('radio', { name: 'crossref', exact: true }).check();
    assert.equal(JSON.parse(await page.getByLabel('Native source parameters').inputValue())['query.bibliographic'], 'gene editing');
    await page.getByLabel('Discovery source order').selectOption('relevance');
    await page.getByRole('checkbox', { name: 'Native query', exact: true }).uncheck();
    await page.getByRole('checkbox', { name: 'Allow approximate source matching', exact: true }).check();
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    const approximate = calls.filter(call => call.path === '/bib/search/sessions').at(-1).body;
    assert.equal(approximate.allow_approximate, true);
    assert.deepEqual(approximate.sources, ['crossref']);
    assert.equal(approximate.native, undefined);
    await page.getByRole('button', { name: 'Stop search', exact: true }).click();
    await page.getByLabel('Discovery result source').selectOption('crossref');
    await page.getByRole('checkbox', { name: 'crossref', exact: true }).uncheck();
    await page.getByRole('checkbox', { name: 'pubmed', exact: true }).check();
    await page.getByRole('checkbox', { name: 'openalex', exact: true }).check();
    await page.getByRole('textbox', { name: 'Discover literature' }).fill('paging fixture');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    await page.getByRole('button', { name: first.title, exact: true }).waitFor();
    assert.equal(await page.getByLabel('Discovery result source').inputValue(), '', 'new source set clears an inapplicable result filter');
    assert.equal(calls.filter(call => call.path.endsWith('/results')).at(-1).query.source, undefined, 'removed source is not sent to the result endpoint');
    await page.getByText('rate_limited: Fixture upstream cooldown', { exact: true }).waitFor();
    const paging = [...sessions.values()].at(-1);
    const creations = calls.filter(call => call.path === '/bib/search/sessions').length;
    await page.getByRole('button', { name: 'Fetch next page', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: 'Fixture candidate cache capacity reached' }).waitFor();
    assert.equal(await page.getByRole('button', { name: first.title, exact: true }).count(), 1, 'capacity failure preserves obtained candidates');
    const startedSnapshot = page.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/bib/search/sessions/${paging.id}` && response.request().method() === 'GET' && Boolean(paging.history));
    await page.getByRole('button', { name: 'Fetch next page', exact: true }).click();
    await startedSnapshot;
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(await page.getByRole('button', { name: 'Show updated results', exact: true }).count(), 0, 'replayed historical batches plus a newer source_started event do not announce new results');
    Object.assign(paging.sources.find(source => source.source === 'pubmed'), { phase: 'exhausted', has_more: false, pages: 2, delivered_records: 2 });
    paging.extra = true; paging.phase = 'idle'; paging.sequence++;
    paging.history.push({ type: 'batch', sequence: paging.sequence });
    await page.getByRole('button', { name: 'Show updated results', exact: true }).click();
    await page.getByRole('button', { name: 'Next-page source paper', exact: true }).waitFor();
    assert.equal(paging.sources.find(source => source.source === 'pubmed').pages, 2);
    await page.getByRole('button', { name: 'Retry source', exact: true }).click();
    await page.getByText('rate_limited: Fixture upstream cooldown', { exact: true }).waitFor({ state: 'detached' });
    assert.equal(calls.filter(call => call.path === '/bib/search/sessions').length, creations, 'pagination and retry reuse the existing session');
    assert.equal(calls.filter(call => call.path.endsWith(`/sessions/${paging.id}/sources/openalex/retry`)).length, 1);
    await page.getByRole('checkbox', { name: 'openalex', exact: true }).uncheck();
    await page.getByRole('textbox', { name: 'Discover literature' }).fill('candidate paging fixture');
    await page.getByRole('button', { name: 'Search sources', exact: true }).click();
    const list = page.getByLabel('Discovery result list', { exact: true });
    await list.locator('[data-loaded-count="100"]').waitFor();
    await list.press('End');
    await page.getByRole('button', { name: 'Obtained candidate 99', exact: true }).waitFor();
    const large = [...sessions.values()].at(-1); large.sequence++;
    large.history = [{ type: 'source_started', sequence: large.sequence }];
    await page.getByRole('button', { name: 'Load more obtained candidates', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: 'Fixture result snapshot changed' }).waitFor();
    assert.equal(await list.locator('[data-loaded-count]').getAttribute('data-loaded-count'), '100', 'stale append does not discard or duplicate displayed rows');
    assert.equal(await page.getByRole('button', { name: 'Load more obtained candidates', exact: true }).isDisabled(), true);
    await page.getByRole('button', { name: 'Show updated results', exact: true }).click();
    for (let loaded = 100; loaded < 1001;) {
      await page.getByRole('button', { name: 'Load more obtained candidates', exact: true }).click();
      loaded = Math.min(1001, loaded + 100);
      await list.locator(`[data-loaded-count="${loaded}"]`).waitFor();
    }
    await list.press('End');
    await page.getByRole('button', { name: 'Obtained candidate 1000', exact: true }).waitFor();
    assert.ok(await list.getByRole('listitem').count() < 60, '1001 obtained candidates remain virtualized');
    assert.equal(await page.getByRole('button', { name: 'Load more obtained candidates', exact: true }).count(), 0);
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('Discovery browser/native-bridge: explicit search, partial delivery, stable selection, stale import rejection, source variant, import, workspace preservation and cleanup passed');
} catch (error) { if (activePage && !activePage.isClosed()) { await activePage.screenshot({ path: '/tmp/discovery-workflow-failure.png' }); console.error((await activePage.locator('body').innerText()).slice(0, 5000)); } throw error; } finally { await browser?.close(); vite.kill('SIGTERM'); }
