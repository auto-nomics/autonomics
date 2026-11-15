export type ImportIdentifier = { id_type: "doi" | "pmid" | "pmc" | "arxiv" | "openalex"; id: string };

/** Resolve the identifier before choosing a provider; a numeric PMID is never a DOI. */
export function parseImportIdentifier(input: string): ImportIdentifier {
  let value = input.trim();
  if (!value) throw new Error("Enter a DOI, PMID, PMCID, arXiv ID, or OpenAlex ID.");
  if (/^https?:\/\//i.test(value)) {
    const url = new URL(value);
    const host = url.hostname.toLowerCase().replace(/^www\./, "");
    const path = decodeURIComponent(url.pathname).replace(/^\/+|\/+$/g, "");
    if (host === "doi.org" || host === "dx.doi.org") value = path;
    else if (host === "pubmed.ncbi.nlm.nih.gov") value = `pmid:${path}`;
    else if (host === "pmc.ncbi.nlm.nih.gov") value = path.replace(/^articles\//, "");
    else if (host === "arxiv.org" || host === "export.arxiv.org") value = `arxiv:${path.replace(/^(abs|pdf)\//, "").replace(/\.pdf$/i, "")}`;
    else if (host === "openalex.org") value = path;
    else throw new Error("Unsupported URL. Paste the article's DOI or supported identifier.");
  }
  if (/^(?:doi:\s*)?10\.\d{4,9}\/\S+$/i.test(value)) return { id_type: "doi", id: value.replace(/^doi:\s*/i, "") };
  if (/^(?:pmid:\s*)?\d+$/i.test(value)) return { id_type: "pmid", id: value.replace(/^pmid:\s*/i, "") };
  if (/^(?:pmc:\s*)?PMC\d+$/i.test(value)) return { id_type: "pmc", id: value.replace(/^pmc:\s*/i, "").toUpperCase() };
  if (/^(?:openalex:\s*)?W\d+$/i.test(value)) return { id_type: "openalex", id: value.replace(/^openalex:\s*/i, "").toUpperCase() };
  const arxiv = value.replace(/^arxiv:\s*/i, "");
  if (/^(?:\d{4}\.\d{4,5}|[a-z][a-z.\-]+\/\d{7})(?:v\d+)?$/i.test(arxiv)) return { id_type: "arxiv", id: arxiv };
  throw new Error("Identifier not recognized. Use a DOI, PMID, PMCID, arXiv ID, or OpenAlex ID.");
}
