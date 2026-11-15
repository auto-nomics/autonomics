/**
 * Background service worker, structured after Zotero Connector's background.js:
 * connectivity probing drives per-tab icon state, saves are orchestrated here,
 * tab state survives service-worker restarts via storage.session, and page
 * interaction happens through the injected progress window.
 */
import { api, ping, uploadAttachment, captureRequest } from './client.js';

const settings = { origin: null, token: null, preferPdf: true, online: false };

async function loadSettings() {
  const local = await chrome.storage.local.get(['origin', 'preferPdf']);
  const session = await chrome.storage.session.get(['token', 'origin']);
  settings.origin = local.origin || null;
  settings.preferPdf = local.preferPdf !== false;
  settings.token = session.origin === settings.origin ? session.token : null;
}

/** Zotero-style availability probe; the icon follows the answer. */
let pinging = null;
async function checkOnline() {
  if (!settings.origin || !settings.token) { settings.online = false; return false; }
  pinging ??= ping(settings.origin, settings.token)
    .then(() => true).catch(() => false)
    .then(result => { settings.online = result; pinging = null; refreshUi().catch(() => {}); return result; });
  return pinging;
}

// --- per-tab state, persisted so a service-worker restart loses nothing ---
const tabInfo = {};
let loadedTabInfo = false;
async function persistTabInfo() {
  await chrome.storage.session.set({ tabInfo: { ...tabInfo } });
}
async function getTabInfo(tabId) {
  if (!loadedTabInfo) {
    loadedTabInfo = true;
    const stored = await chrome.storage.session.get(['tabInfo']);
    Object.assign(tabInfo, stored.tabInfo || {});
  }
  return tabInfo[tabId];
}
async function setTabInfo(tabId, value) {
  tabInfo[tabId] = value;
  await persistTabInfo();
}

async function refreshUi(tabId) {
  const apply = async props => { for (const [api2, value] of Object.entries(props)) await chrome.action[api2]({ ...value, tabId }).catch(() => {}); };
  if (settings.origin && settings.token) await chrome.action.setPopup({ popup: '', tabId }).catch(() => {});
  else await chrome.action.setPopup({ popup: 'popup.html', tabId }).catch(() => {});
  const info = tabId != null ? tabInfo[tabId] : null;
  if (!settings.origin || !settings.token) {
    await apply({ setTitle: { title: 'Autonomics Library Connector — connect a gateway first' }, setBadgeText: { text: '' } });
  } else if (!settings.online) {
    await apply({ setTitle: { title: 'Autonomics gateway is not reachable' }, setBadgeText: { text: '×' }, setBadgeBackgroundColor: { color: '#9ca3af' } });
  } else if (info?.type === 'multiple') {
    await apply({ setTitle: { title: `Save multiple items to Autonomics (${info.label})` }, setBadgeText: { text: String(info.count || '') }, setBadgeBackgroundColor: { color: '#7c3aed' } });
  } else if (info?.type) {
    await apply({ setTitle: { title: `Save to Autonomics (${info.label})` }, setBadgeText: { text: '●' }, setBadgeBackgroundColor: { color: '#059669' } });
  } else {
    await apply({ setTitle: { title: 'Save webpage to Autonomics' }, setBadgeText: { text: '' } });
  }
}

// --- context menu, rebuilt per navigation like Zotero's ---
async function rebuildContextMenu(tab) {
  await chrome.contextMenus.removeAll();
  if (!tab?.id) return;
  const info = tabInfo[tab.id];
  await chrome.contextMenus.create({
    id: 'save-page', title: info?.type ? `Save to Autonomics (${info.label})` : 'Save webpage to Autonomics',
    contexts: ['page', 'action'],
  });
  await chrome.contextMenus.create({ id: 'open-popup', title: 'Connector settings…', contexts: ['action'] });
}

chrome.tabs.onUpdated.addListener(async (tabId, change) => {
  if (change.status === 'complete') { delete tabInfo[tabId]; await persistTabInfo(); await refreshUi(tabId); }
});
chrome.tabs.onActivated.addListener(async ({ tabId }) => { await getTabInfo(tabId); await refreshUi(tabId); await rebuildContextMenu({ id: tabId }); });
chrome.tabs.onRemoved.addListener(async tabId => { delete tabInfo[tabId]; await persistTabInfo(); });

// --- messages -------------------------------------------------------------
const pendingSelections = new Map(); // tabId -> {resolve, entries}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  handleMessage(message, sender).then(sendResponse, error => {
    console.error('[autonomics]', message?.type, error);
    sendResponse({ error: String(error?.message || error) });
  });
  return true; // async response
});

async function handleMessage(message, sender) {
  switch (message?.type) {
    case 'detected': return onDetected(message, sender);
    case 'translate': return onTranslate(message, sender);
    case 'captureTab': return onCaptureTab();
    case 'selection': return onSelection(message, sender);
    case 'connectivity': { await checkOnline(); return { online: settings.online }; }
    default: throw new Error(`Unknown message ${message?.type}`);
  }
}

/** Content script reports each navigation after running detection in the page DOM. */
async function onDetected(message, sender) {
  const tabId = sender.tab?.id;
  if (tabId == null) return { ok: true };
  await setTabInfo(tabId, {
    type: message.itemType || null,
    translatorId: message.translatorId || null,
    label: message.label || null,
    count: message.count || 0,
    url: message.url,
  });
  await checkOnline();
  await refreshUi(tabId);
  if (sender.tab?.active) await rebuildContextMenu(sender.tab);
  return { ok: true };
}

/**
 * Capture the live tab. MV3 service workers have no DOMParser, so the
 * detect/doWeb pipeline runs in the page (content script, or an injected
 * dynamic import under activeTab when the content script is absent).
 */
async function captureDocument(tabId) {
  try {
    const result = await chrome.tabs.sendMessage(tabId, { type: 'capture' });
    if (result?.error) throw new Error(result.error);
    if (result) return result;
  } catch { /* no content script on this page */ }
  const [injected] = await chrome.scripting.executeScript({
    target: { tabId },
    func: async () => {
      const { detectWeb } = await import(chrome.runtime.getURL('translators.js'));
      const match = detectWeb(document, location.href);
      if (!match) return { error: 'Nothing recognizable on this page.' };
      const result = match.translator.doWeb(document, location.href);
      if (!result) return { error: 'Nothing recognizable on this page.' };
      return { label: match.translator.label, type: match.type, ...result };
    },
  });
  const result = injected.result;
  if (result?.error) throw new Error(result.error);
  return result;
}

/** Subpages of a multi-item save: the worker fetches, the tab's content script parses. */
async function fetchEntryPage(tabId, url) {
  const response = await fetch(url, { credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(30_000) });
  if (!response.ok) throw new Error(`Could not load ${url} (${response.status}).`);
  const html = await response.text();
  try {
    return await chrome.tabs.sendMessage(tabId, { type: 'parseEntry', url, html });
  } catch {
    return { capture: null, pdfUrl: null }; // activeTab-only mode: keep the listing title
  }
}

/** Popup preview: run detection on the live tab without saving. */
async function onCaptureTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const captured = await captureDocument(tab.id);
  return {
    translator: captured.label,
    multiple: captured.multiple || null,
    capture: captured.items?.[0] || null,
  };
}

/** The save pipeline: click → detect → (selection) → create → attachment → progress. */
async function onTranslate(_message, sender) {
  await startSave(sender.tab?.id);
  return { started: true };
}

async function startSave(tabId) {
  const report = async (event, payload = {}) =>
    chrome.tabs.sendMessage(tabId, { type: 'progress', event, ...payload }).catch(() => {});
  if (tabId == null) return;
  if (!settings.origin || !settings.token) {
    await chrome.action.setPopup({ popup: 'popup.html', tabId }).catch(() => {});
    await report('error', { message: 'Connect a gateway in the connector settings first.' });
    return;
  }
  await checkOnline();
  try {
    if (!settings.online) await report('show', { headline: 'Gateway offline — capture will be kept offline' });
    const captured = await captureDocument(tabId);
    let selections = null; // [{capture, pdfUrl}] resolved items, in save order
    if (captured.multiple) {
      const entries = await askSelection(tabId, captured.multiple);
      if (!entries?.length) { await report('close'); return; }
      await report('show', { headline: `Saving to Autonomics via ${captured.label}` });
      selections = [];
      for (const entry of entries) {
        await report('itemProgress', { index: selections.length, title: entry.title, progress: 'Fetching…' });
        const page = await fetchEntryPage(tabId, entry.url);
        selections.push({
          capture: page.capture || { title: entry.title, authors: [], keywords: [], source_url: entry.url },
          pdfUrl: page.pdfUrl,
        });
      }
    } else {
      await report('show', { headline: `Saving to Autonomics via ${captured.label}` });
      selections = [{ capture: captured.items?.[0], pdfUrl: captured.pdfUrl }];
    }
    for (let index = 0; index < selections.length; index += 1) {
      await saveCapture(selections[index].capture, selections[index].pdfUrl, { report, index });
    }
    await report('done');
  } catch (error) {
    await report('error', { message: String(error?.message || error) });
  }
}

/** Zotero-style itemSelector, rendered inside the progress window. */
function askSelection(tabId, entries) {
  return new Promise(resolve => {
    pendingSelections.set(tabId, { resolve, entries });
    chrome.tabs.sendMessage(tabId, { type: 'progress', event: 'select', entries: entries.map(entry => ({ index: entry.index, title: entry.title })) }).catch(() => {
      pendingSelections.delete(tabId); resolve([]);
    });
  });
}

async function onSelection({ tabId, indices }, sender) {
  const target = tabId ?? sender?.tab?.id;
  const pending = pendingSelections.get(target);
  if (!pending) return { ok: false };
  pendingSelections.delete(tabId);
  pending.resolve(pending.entries.filter(entry => indices.includes(entry.index)));
  return { ok: true };
}

async function saveCapture(capture, pdfUrl, { report, index }) {
  const title = capture.title || 'Untitled capture';
  await report('itemProgress', { index, title, progress: 'Saving…' });
  const enqueue = async () => {
    const stored = await chrome.storage.local.get(['outbox']);
    const outbox = stored.outbox || [];
    if (outbox.length >= 100) throw new Error('Offline queue is full (100 captures).');
    outbox.push({ id: crypto.randomUUID(), metadata: capture, pdf_url: pdfUrl || null, collection: '', origin: settings.origin });
    await chrome.storage.local.set({ outbox });
  };
  if (!settings.online) { await enqueue(); await report('itemProgress', { index, title, progress: 'Kept offline' }); return null; }
  try {
    const result = await api(settings.origin, settings.token, 'POST', '/articles/manual', captureRequest(capture));
    const articleId = result.article?.id;
    if (pdfUrl && settings.preferPdf && articleId) {
      await report('itemProgress', { index, title, progress: 'Attaching PDF…' });
      try {
        await uploadAttachment(settings.origin, settings.token, articleId, pdfUrl, title);
        await report('itemProgress', { index, title, progress: 'Saved with PDF', done: true, articleId });
      } catch (error) {
        await report('itemProgress', { index, title, progress: 'Saved (PDF failed)', done: true, articleId, warning: String(error?.message || error) });
      }
    } else {
      await report('itemProgress', { index, title, progress: 'Saved', done: true, articleId });
    }
    return articleId;
  } catch (error) {
    await enqueue();
    await report('itemProgress', { index, title, progress: 'Kept offline', warning: String(error?.message || error) });
    return null;
  }
}

// --- entry points ---------------------------------------------------------
chrome.action.onClicked.addListener(async tab => { if (tab.id != null) await startSave(tab.id); });

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === 'save-page' && tab?.id != null) await startSave(tab.id);
  if (info.menuItemId === 'open-popup') chrome.runtime.openOptionsPage().catch(() => {});
});

chrome.runtime.onInstalled.addListener(async () => {
  await loadSettings();
  await checkOnline();
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tab?.id != null) await refreshUi(tab.id);
});

chrome.runtime.onStartup.addListener(async () => { await loadSettings(); await checkOnline(); });

// Refresh UI state when the popup changes settings.
chrome.storage.onChanged.addListener(async (_changes, area) => {
  if (area !== 'local' && area !== 'session') return;
  await loadSettings();
  await checkOnline();
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tab?.id != null) await refreshUi(tab.id);
});
