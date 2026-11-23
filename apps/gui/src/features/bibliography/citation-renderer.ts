import i18next from "i18next";

export type CitationStyle = "apa" | "vancouver" | "gb7714";
export type CslItem = { id: string; type: string; title?: string; [key: string]: unknown };
/** Bundled CSL assets run locally; no citation contents are sent to third parties. */
export async function renderCitations(items: CslItem[], style: CitationStyle, citationIds?: string[]) {
  const [{ default: CSL }, { default: apa }, { default: vancouver }, { default: locale }, { default: chineseStyle }, { default: chineseLocale }] = await Promise.all([
    import("citeproc"), import("./csl/apa.csl?raw"), import("./csl/nlm-citation-sequence.csl?raw"), import("./csl/locales-en-US.xml?raw"), import("./csl/china-national-standard-gb-t-7714-2015-numeric.csl?raw"), import("./csl/locales-zh-CN.xml?raw"),
  ]);
  const indexed = new Map(items.map(item => [String(item.id), item]));
  const processor = new CSL.Engine({ retrieveLocale: language => language.startsWith("zh") ? chineseLocale : locale, retrieveItem: id => indexed.get(String(id)) }, style === "apa" ? apa : style === "gb7714" ? chineseStyle : vancouver, style === "gb7714" ? "zh-CN" : "en-US");
  processor.setOutputFormat("text");
  processor.updateItems([...indexed.keys()]);
  const citation = processor.makeCitationCluster((citationIds ?? [...indexed.keys()]).map(id => ({ id })));
  const rendered = processor.makeBibliography();
  if (!rendered) throw new Error(i18next.t("bibliography:citationRenderer.noBibliography"));
  return { citation, bibliography: rendered[1].join("\n").trim() };
}
