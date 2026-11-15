/**
 * Content script, modeled on Zotero's inject layer. MV3 service workers have no
 * DOMParser (Zotero runs an offscreen translate host for this reason); we run
 * the translator registry here in the page instead, where the live DOM already
 * is. Credentials never pass through this context — saving happens in the
 * service worker. The progress window iframe lives in a closed shadow root so
 * page scripts cannot reach it.
 */
const registry = chrome.runtime.getURL('translators.js');

async function detectOnDocument(doc, url) {
  const { detectWeb } = await import(registry);
  const match = detectWeb(doc, url);
  if (!match) return null;
  const result = match.translator.doWeb(doc, url);
  return result && { translatorId: match.translator.id, label: match.translator.label, type: match.type, ...result };
}

// Report this navigation so the background can set the per-tab icon.
detectOnDocument(document, location.href).then(detection => {
  chrome.runtime.sendMessage({
    type: 'detected', url: location.href,
    translatorId: detection?.translatorId || null, label: detection?.label || null,
    itemType: detection?.type || null, count: detection?.multiple?.length || 0,
  }).catch(() => {});
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'capture') {
    detectOnDocument(document, location.href)
      .then(result => sendResponse(result || { error: 'Nothing recognizable on this page.' }))
      .catch(error => sendResponse({ error: String(error?.message || error) }));
    return true;
  }
  if (message?.type === 'parseEntry') { // a subpage fetched by the worker: parse it with this tab's DOMParser
    const doc = new DOMParser().parseFromString(message.html, 'text/html');
    const base = doc.createElement('base'); base.href = message.url; doc.head?.append(base);
    detectOnDocument(doc, message.url)
      .then(result => sendResponse(result ? { capture: result.items?.[0] || null, pdfUrl: result.pdfUrl || null } : { capture: null, pdfUrl: null }))
      .catch(error => sendResponse({ error: String(error?.message || error) }));
    return true;
  }
  if (message?.type === 'save') { chrome.runtime.sendMessage({ type: 'translate' }).catch(() => {}); sendResponse({ ok: true }); return; }
  if (message?.type === 'progress') { deliverProgress(message).then(() => sendResponse({ ok: true }), error => sendResponse({ error: String(error) })); return true; }
  return false;
});

let frame = null; // { host, root, nonce }

async function ensureFrame() {
  if (frame?.host?.isConnected) return frame;
  const nonce = crypto.randomUUID();
  const host = document.createElement('div');
  host.dataset.autonomicsProgress = '';
  const root = host.attachShadow({ mode: 'closed' });
  const iframe = document.createElement('iframe');
  iframe.src = `${chrome.runtime.getURL('progress.html')}#${nonce}`;
  root.append(iframe);
  document.documentElement.append(host);
  const queue = [];
  frame = { host, root, nonce, queue };
  window.addEventListener('message', event => {
    if (event.data?.nonce !== nonce) return;
    const { kind, ...payload } = event.data;
    if (kind === 'close') host.remove();
    if (kind === 'selection') chrome.runtime.sendMessage({ type: 'selection', indices: payload.indices }).catch(() => {});
  });
  return frame;
}

function postIntoFrame(message) {
  frame.root.querySelector('iframe')?.contentWindow?.postMessage({ nonce: frame.nonce, ...message }, '*');
}

async function deliverProgress(message) {
  if (message.event === 'close') { frame?.host.remove(); return; }
  await ensureFrame();
  postIntoFrame(message);
}
