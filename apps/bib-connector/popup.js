import { extractMetadata } from './extract.js';
import { api, gatewayOrigin, captureRequest } from './client.js';
const el = id => document.getElementById(id);
let metadata = {}, busy = false;
const status = message => { el('status').textContent = message; };
await chrome.storage.local.setAccessLevel({ accessLevel: 'TRUSTED_CONTEXTS' });
const settings = await chrome.storage.local.get(['origin', 'outbox']);
el('origin').value = settings.origin || 'http://127.0.0.1:8765';
const session = await chrome.storage.session.get(['token', 'origin']);
el('token').value = session.origin === el('origin').value ? session.token || '' : '';
let outbox = settings.outbox || [];
let connectedOrigin = session.origin;
async function request(method, path, body) {
  if (gatewayOrigin(el('origin').value) !== connectedOrigin) throw new Error('Connect to this gateway before saving.');
  return api(el('origin').value, el('token').value, method, path, body);
}
async function connect() {
  const origin = gatewayOrigin(el('origin').value);
  if (!await chrome.permissions.request({ origins: [`${origin}/*`] })) throw new Error('Gateway access was not granted.');
  const result = await api(origin, el('token').value, 'GET', '/collections');
  await chrome.storage.local.set({ origin });
  await chrome.storage.session.set({ token: el('token').value, origin });
  connectedOrigin = origin;
  el('collection').replaceChildren(new Option('My library', ''));
  for (const collection of result.collections || []) if (!collection.deleted) el('collection').add(new Option(collection.name, collection.id));
  status('Connected. Review the metadata before saving.');
}
async function run(action) {
  if (busy) return;
  busy = true; for (const button of document.querySelectorAll('button')) button.disabled = true;
  try { await action(); } catch (error) { status(String(error.message || error)); }
  finally { busy = false; for (const button of document.querySelectorAll('button')) button.disabled = false; }
}
function draft() {
  return { ...metadata, ...Object.fromEntries(['title', 'doi', 'pmid', 'arxiv', 'journal'].map(id => [id, el(id).value])),
    year: el('year').value ? Number(el('year').value) : null,
    authors: el('authors').value.split('\n').map(value => value.trim()).filter(Boolean) };
}
async function save(item) {
  if (gatewayOrigin(el('origin').value) !== item.origin) throw new Error(`This capture belongs to ${item.origin}. Reconnect to that gateway before retrying.`);
  // A queued saved article ID prevents duplicate creation if only collection assignment failed.
  if (!item.articleId) {
    const result = await request('POST', '/articles/manual', captureRequest(item.metadata));
    item.articleId = result.article.id;
    await persist();
  }
  if (item.collection) await request('POST', `/collections/${encodeURIComponent(item.collection)}/articles`, { article_id: item.articleId });
  outbox = outbox.filter(entry => entry.id !== item.id);
  await persist(); status('Saved to your library.');
}
async function persist() { await chrome.storage.local.set({ outbox }); renderOutbox(); }
function renderOutbox() {
  el('outbox').replaceChildren();
  for (const item of outbox) {
    const row = document.createElement('div'); row.className = 'pending';
    const title = document.createElement('div'); title.textContent = item.metadata.title;
    const retry = document.createElement('button'); retry.textContent = 'Retry'; retry.onclick = () => run(() => save(item));
    const remove = document.createElement('button'); remove.textContent = 'Remove'; remove.onclick = () => run(async () => { outbox = outbox.filter(value => value.id !== item.id); await persist(); });
    const target = document.createElement('small'); target.textContent = item.origin;
    row.append(title, target, document.createElement('br'), retry, remove); el('outbox').append(row);
  }
}
async function enqueue() {
  const value = draft(); captureRequest(value);
  const item = { id: crypto.randomUUID(), metadata: value, collection: el('collection').value, origin: gatewayOrigin(el('origin').value) };
  if (outbox.length >= 100) throw new Error('Offline queue is full (100 captures). Retry or remove existing captures.');
  outbox.push(item); await persist(); return item;
}
el('settings').onsubmit = event => { event.preventDefault(); void run(connect); };
el('capture').onsubmit = event => { event.preventDefault(); void run(async () => { const item = await enqueue(); await save(item); }); };
el('queue').onclick = () => run(async () => { await enqueue(); status('Kept offline on this device.'); });
el('forget').onclick = () => run(async () => { await chrome.storage.session.remove('token'); el('token').value = ''; status('Token forgotten.'); });
renderOutbox();
try {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const [{ result }] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, func: extractMetadata });
  metadata = result;
  for (const id of ['title', 'doi', 'pmid', 'arxiv', 'journal', 'year']) el(id).value = metadata[id] || '';
  el('authors').value = metadata.authors.join('\n'); el('source').textContent = metadata.source_url;
} catch (error) { status(`Could not read this page: ${error.message}`); }
