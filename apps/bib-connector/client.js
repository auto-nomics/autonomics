/** Gateway client shared by the popup and the background service worker. */

export function gatewayOrigin(value) {
  const url = new URL(value);
  if (url.username || url.password || url.search || url.hash || !['', '/'].includes(url.pathname)) throw new Error('Enter the gateway origin only, without a path or credentials.');
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && ['127.0.0.1', 'localhost'].includes(url.hostname))) throw new Error('Use HTTPS, or HTTP on localhost.');
  return url.origin;
}

async function request(origin, token, method, path, options = {}) {
  if (!token?.trim()) throw new Error('A gateway token is required.');
  const response = await fetch(`${origin}/api/v1/bib${path}`, {
    method, credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(options.timeout ?? 30_000),
    headers: { Authorization: `Bearer ${token}`, ...(options.headers || {}) },
    body: options.body,
  });
  if (options.raw) return response;
  const result = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(response.status === 409
    ? `This record already exists in the library. Review the existing record before removing this pending capture. ${result.error || ''}`
    : result.error || `Gateway returned ${response.status}`);
  return result;
}

export function api(origin, token, method, path, body) {
  return request(origin, token, method, path, {
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
}

/** Zotero-style availability probe: the icon follows this answer. */
export async function ping(origin, token) {
  const response = await request(origin, token, 'GET', '/health', { raw: true, timeout: 5_000 });
  if (!response.ok) throw new Error(`Gateway returned ${response.status}`);
  return true;
}

/** Fetch a linked PDF and upload it as a Zotero-style attachment. */
export async function uploadAttachment(origin, token, articleId, pdfUrl, title) {
  const file = await fetch(pdfUrl, { credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(120_000) });
  if (!file.ok) throw new Error(`Could not download the PDF (${file.status}).`);
  const blob = await file.blob();
  if ((blob.size || 0) === 0) throw new Error('The PDF link returned an empty file.');
  const form = new FormData();
  const name = (() => { try { return new URL(pdfUrl).pathname.split('/').pop() || 'article.pdf'; } catch { return 'article.pdf'; } })();
  form.append('file', blob, name.toLowerCase().endsWith('.pdf') ? name : `${name}.pdf`);
  form.append('title', title || name);
  form.append('preferred', 'true');
  return request(origin, token, 'POST', `/articles/${encodeURIComponent(articleId)}/attachments`, { body: form, timeout: 180_000 });
}

export function captureRequest(metadata) {
  if (!String(metadata.title || '').trim()) throw new Error('Title is required.');
  if (metadata.year && (!Number.isInteger(metadata.year) || metadata.year < 1 || metadata.year > 9999)) throw new Error('Year must be between 1 and 9999.');
  const identifiers = ['doi', 'pmid', 'arxiv'].filter(kind => metadata[kind]?.trim()).map(kind => ({ kind, value: metadata[kind].trim() }));
  const source = new URL(metadata.source_url);
  if (!['http:', 'https:'].includes(source.protocol)) throw new Error('Only HTTP(S) pages can be captured.');
  source.hash = '';
  identifiers.push({ kind: 'other', value: source.href });
  return {
    id: '', title: metadata.title.trim(),
    authors: (metadata.authors || []).map(name => ({ last_name: name.trim(), fore_name: null })).filter(author => author.last_name),
    identifiers, year: metadata.year || null, journal: metadata.journal || null,
    volume: metadata.volume || null, issue: metadata.issue || null, pages: metadata.pages || null,
    abstract_text: metadata.abstract_text || null, keywords: metadata.keywords || [], source: 'manual',
    pub_types: identifiers.length > 1 || metadata.journal ? ['Journal Article'] : ['Web Page'],
  };
}
