/**
 * Translator registry, imitating Zotero's translator contract (detectWeb/doWeb)
 * with bundled first-party code so no sandboxed host frame or remote repo is needed.
 *
 * A translator is { id, label, priority, webRegexp?: {root}, detect(doc,url), doWeb(doc,url) }.
 * detect returns an itemType string, 'multiple', or null. doWeb returns
 * { items: CaptureMetadata[], pdfUrl?: string } for single items or
 * { multiple: {title, url}[] } when more selection is needed.
 */

const array = value => (Array.isArray(value) ? value : value == null ? [] : [value]);

function metaTags(doc) {
  const map = new Map();
  for (const node of doc.querySelectorAll('meta')) {
    const key = (node.getAttribute('name') || node.getAttribute('property') || '').toLowerCase();
    const content = (node.getAttribute('content') || '').trim();
    if (!key || !content) continue;
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(content);
  }
  return map;
}

function firstMeta(meta, ...names) {
  for (const name of names) {
    const values = meta.get(name);
    if (values?.length) return values[0];
  }
  return '';
}

function allMeta(meta, ...names) {
  for (const name of names) if (meta.get(name)?.length) return meta.get(name);
  return [];
}

function jsonLdNodes(doc) {
  const nodes = [];
  for (const node of doc.querySelectorAll('script[type="application/ld+json"]')) {
    try {
      const parsed = JSON.parse(node.textContent || 'null');
      for (const entry of array(parsed)) nodes.push(...array(entry?.['@graph'] ?? entry));
    } catch { /* Broken publisher JSON-LD must not hide usable meta tags. */ }
  }
  return nodes;
}

function jsonLdArticle(doc) {
  const wanted = ['ScholarlyArticle', 'Article', 'NewsArticle', 'Book', 'TechArticle', 'Thesis'];
  return jsonLdNodes(doc).find(node => wanted.some(type => array(node?.['@type']).includes(type))) || null;
}

function doiFrom(value) {
  return String(value || '').match(/10\.\d{4,9}\/[^\s"<>]+/i)?.[0]?.replace(/[.,;)]+$/, '') || '';
}

function yearFrom(value) {
  return String(value || '').match(/\b(1\d{3}|2\d{3})\b/)?.[0] || '';
}

/** Extract a capture from Dublin Core / PRISM / Highwire meta tags plus JSON-LD. */
function captureFromEmbeddedMetadata(doc, url) {
  const meta = metaTags(doc);
  const structured = jsonLdArticle(doc);
  const doi = doiFrom(
    firstMeta(meta, 'citation_doi', 'dc.identifier', 'prism.doi', 'doi')
      || structured?.identifier?.value || structured?.identifier,
  );
  let authors = [...allMeta(meta, 'citation_author'), ...allMeta(meta, 'citation_authors', 'author')];
  if (!authors.length) {
    authors = array(structured?.author).map(author =>
      typeof author === 'string' ? author : [author?.family, (array(author?.given) || author?.name).filter(Boolean).join(' ')].filter(Boolean).join(', ') || author?.name,
    ).filter(Boolean);
  }
  const date = firstMeta(meta, 'citation_publication_date', 'citation_date', 'dc.date', 'prism.publicationdate', 'dc.issued')
    || structured?.datePublished || '';
  const year = yearFrom(date);
  return {
    title: firstMeta(meta, 'citation_title', 'dc.title', 'og:title') || structured?.headline || structured?.name || doc.title || '',
    authors, doi,
    pmid: firstMeta(meta, 'citation_pmid', 'pmid'),
    arxiv: firstMeta(meta, 'citation_arxiv_id'),
    year: year ? Number(year) : null,
    journal: firstMeta(meta, 'citation_journal_title', 'prism.publicationname', 'citation_source'),
    volume: firstMeta(meta, 'citation_volume', 'prism.volume'),
    issue: firstMeta(meta, 'citation_issue', 'prism.number'),
    pages: [firstMeta(meta, 'citation_firstpage', 'prism.startingpage'), firstMeta(meta, 'citation_lastpage')].filter(Boolean).join('-') || null,
    abstract_text: firstMeta(meta, 'citation_abstract', 'dc.description', 'og:description', 'description') || structured?.abstract || '',
    keywords: firstMeta(meta, 'citation_keywords', 'keywords').split(/[;,]/).map(value => value.trim()).filter(Boolean),
    pdf_url: firstMeta(meta, 'citation_pdf_url') || null,
    source_url: url,
  };
}

function parseCoins(title) {
  const params = new URLSearchParams(String(title || '').replace(/\+/g, ' '));
  const decode = suffix => params.get(`rft.${suffix}`) || '';
  return {
    title: decode('atitle') || decode('title') || decode('btitle'),
    authors: params.getAll('rft.au').filter(Boolean),
    year: Number(yearFrom(decode('date'))) || null,
    journal: decode('jtitle'),
    volume: decode('volume'), issue: decode('issue'),
    pages: [decode('spage'), decode('epage')].filter(Boolean).join('-') || null,
    doi: doiFrom(params.get('rft_id') || params.get('rft_id=info:doi') || ''),
  };
}

const embeddedMetadata = {
  id: 'embedded-metadata', label: 'Embedded Metadata', priority: 90,
  detect(doc, url) {
    const capture = captureFromEmbeddedMetadata(doc, url);
    if (capture.title && (capture.doi || capture.journal || capture.pdf_url || capture.authors.length)) return 'journalArticle';
    return null;
  },
  doWeb(doc, url) {
    const capture = captureFromEmbeddedMetadata(doc, url);
    return { items: [capture], pdfUrl: capture.pdf_url };
  },
};

const coins = {
  id: 'coins', label: 'COinS', priority: 80,
  detect(doc) {
    return doc.querySelectorAll('span.Z3988[title]').length > 1 ? 'multiple'
      : doc.querySelector('span.Z3988[title]') ? 'journalArticle' : null;
  },
  doWeb(doc, url) {
    const entries = [...doc.querySelectorAll('span.Z3988[title]')]
      .map(span => ({ span, coins: parseCoins(span.getAttribute('title')) }))
      .filter(entry => entry.coins.title);
    if (entries.length > 1) {
      return { multiple: entries.map((entry, index) => ({ index, title: entry.coins.title, url })) };
    }
    const single = entries[0]?.coins;
    return single ? { items: [{ ...captureFromEmbeddedMetadata(doc, url), ...single, source_url: url }] } : null;
  },
};

const pubmed = {
  id: 'pubmed', label: 'PubMed', priority: 100,
  webRegexp: { root: /^https?:\/\/pubmed\.ncbi\.nlm\.nih\.gov\// },
  detect(doc, url) {
    if (/^\/\d+\/?$/.test(new URL(url).pathname)) return 'journalArticle';
    return doc.querySelectorAll('a[data-article-id], a.docsum-title, a[href^="/"][href*="/"]').length &&
      doc.querySelector('a.docsum-title') ? 'multiple' : null;
  },
  doWeb(doc, url) {
    const path = new URL(url).pathname;
    const pmid = path.match(/^\/(\d+)\/?$/)?.[1];
    if (pmid) {
      const capture = captureFromEmbeddedMetadata(doc, url);
      return { items: [{ ...capture, pmid: firstMeta(metaTags(doc), 'citation_pmid') || pmid }], pdfUrl: capture.pdf_url };
    }
    const results = [...doc.querySelectorAll('a.docsum-title')].map((anchor, index) => ({
      index,
      title: anchor.textContent.trim(),
      url: new URL(anchor.getAttribute('href') || '', url).href,
    })).filter(entry => entry.title && /^\/\d+\/?$/.test(new URL(entry.url).pathname));
    return results.length > 1 ? { multiple: results } : null;
  },
};

const arxiv = {
  id: 'arxiv', label: 'arXiv', priority: 100,
  webRegexp: { root: /^https?:\/\/arxiv\.org\// },
  detect(doc, url) {
    if (/^\/(?:abs|pdf)\//.test(new URL(url).pathname)) return 'preprint';
    const listings = doc.querySelectorAll('a[href^="/abs/"]');
    return listings.length > 1 ? 'multiple' : listings.length === 1 ? 'preprint' : null;
  },
  doWeb(doc, url) {
    const id = new URL(url).pathname.match(/^\/(?:abs|pdf)\/(.+?)(?:\.pdf)?$/)?.[1];
    if (id) {
      const capture = captureFromEmbeddedMetadata(doc, url);
      return {
        items: [{ ...capture, arxiv: capture.arxiv || id, pdf_url: capture.pdf_url || `https://arxiv.org/pdf/${id}` }],
        pdfUrl: capture.pdf_url || `https://arxiv.org/pdf/${id}`,
      };
    }
    const results = [...doc.querySelectorAll('a[href^="/abs/"]')].map((anchor, index) => ({
      index, title: anchor.textContent.trim(), url: new URL(anchor.getAttribute('href'), url).href,
    })).filter(entry => entry.title && entry.url.includes('/abs/'));
    return results.length > 1 ? { multiple: results } : null;
  },
};

const webpage = {
  id: 'webpage', label: 'Web Page', priority: 10,
  detect(doc) { return doc.title ? 'webpage' : null; },
  doWeb(doc, url) {
    const meta = metaTags(doc);
    return {
      items: [{
        title: doc.title || new URL(url).hostname,
        authors: [], doi: '', pmid: '', arxiv: '', year: null,
        journal: null, volume: null, issue: null, pages: null,
        abstract_text: firstMeta(meta, 'description', 'og:description') || '',
        keywords: [], pdf_url: null, source_url: url,
      }],
    };
  },
};

export const TRANSLATORS = [pubmed, arxiv, embeddedMetadata, coins, webpage]
  .sort((a, b) => b.priority - a.priority);

/** Zotero-style entry point: best translator for a document, or null. */
export function detectWeb(doc, url) {
  for (const translator of TRANSLATORS) {
    if (translator.webRegexp?.root && !translator.webRegexp.root.test(url)) continue;
    const type = translator.detect(doc, url);
    if (type) return { translator, type };
  }
  return null;
}
