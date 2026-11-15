/** Self-contained: executed in the isolated world, never receives a credential. */
export function extractMetadata() {
  const meta = (names) => [...document.querySelectorAll('meta')]
    .filter(node => names.includes((node.getAttribute('name') || node.getAttribute('property') || '').toLowerCase()))
    .map(node => (node.getAttribute('content') || '').trim()).filter(Boolean);
  const first = (...names) => meta(names)[0] || '';
  let structured = {};
  for (const node of document.querySelectorAll('script[type="application/ld+json"]')) {
    try {
      const json = JSON.parse(node.textContent || '{}');
      const list = Array.isArray(json) ? json : [json, ...(json['@graph'] || [])];
      const article = list.find(item => [item?.['@type']].flat().some(type => ['ScholarlyArticle', 'Article', 'NewsArticle', 'Book'].includes(type)));
      if (article) { structured = article; break; }
    } catch { /* Broken publisher JSON-LD must not hide usable meta tags. */ }
  }
  const doiValue = meta(['citation_doi', 'dc.identifier', 'prism.doi']).join(' ') || String(structured.identifier?.value || structured.identifier || '');
  const doi = doiValue.match(/10\.\d{4,9}\/[^\s"<>]+/i)?.[0] || '';
  const authors = meta(['citation_author']);
  if (!authors.length) for (const author of [structured.author || []].flat()) {
    const name = typeof author === 'string' ? author : author.name;
    if (name) authors.push(name);
  }
  const date = first('citation_publication_date', 'citation_date', 'dc.date', 'prism.publicationdate') || String(structured.datePublished || '');
  const year = date.match(/\b(1\d{3}|2\d{3})\b/)?.[0];
  const pmid = first('citation_pmid') || (location.hostname === 'pubmed.ncbi.nlm.nih.gov' ? location.pathname.match(/^\/(\d+)\/?$/)?.[1] : '') || '';
  const arxiv = first('citation_arxiv_id') || (location.hostname === 'arxiv.org' ? location.pathname.match(/^\/(?:abs|pdf)\/(.+?)(?:\.pdf)?$/)?.[1] || '' : '');
  return {
    title: first('citation_title', 'dc.title', 'og:title') || structured.headline || structured.name || document.title,
    authors, doi, pmid, arxiv, year: year ? Number(year) : null,
    journal: first('citation_journal_title', 'prism.publicationname'),
    volume: first('citation_volume'), issue: first('citation_issue'),
    pages: [first('citation_firstpage'), first('citation_lastpage')].filter(Boolean).join('-'),
    abstract_text: first('citation_abstract', 'dc.description', 'description', 'og:description') || structured.abstract || '',
    keywords: first('citation_keywords', 'keywords').split(/[;,]/).map(value => value.trim()).filter(Boolean),
    source_url: location.href,
  };
}
