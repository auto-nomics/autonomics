declare module "citeproc" {
  const CSL: { Engine: new (sys: { retrieveLocale: (language: string) => string; retrieveItem: (id: string) => unknown }, style: string, locale?: string) => {
    setOutputFormat: (format: string) => void;
    updateItems: (ids: string[]) => void;
    makeCitationCluster: (items: { id: string }[]) => string;
    makeBibliography: () => [Record<string, unknown>, string[]] | false;
  } };
  export default CSL;
}
