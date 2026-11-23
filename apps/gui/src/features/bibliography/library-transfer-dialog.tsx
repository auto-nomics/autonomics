import { useEffect, useState } from "react";
import { Copy, Download, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { gatewayRequest, saveTextFile } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";
import { renderCitations, type CitationStyle } from "./citation-renderer";
import type { BibArticle } from "@/types";
type PreviewRow = { index: number; article: BibArticle; warnings: string[]; duplicate_id: string | null; duplicate_in_file: number | null };
export function LibraryTransferDialog({ mode, articleIds = [], collectionId, onClose, onImported }: {
  mode: "import" | "export"; articleIds?: string[]; collectionId?: string | null; onClose: () => void; onImported: () => void;
}) {
  const { t } = useTranslation("bibliography");
  const [format, setFormat] = useState(mode === "import" ? "bibtex" : "apa");
  const [contents, setContents] = useState("");
  const [rows, setRows] = useState<PreviewRow[]>([]);
  const [selected, setSelected] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [status, setStatus] = useState("");
  const [output, setOutput] = useState("");
  const [citation, setCitation] = useState("");
  const invalidate = () => { setRows([]); setSelected([]); setStatus(""); setError(""); };
  useEffect(() => {
    if (mode !== "export") return;
    let active = true;
    setBusy(true); setOutput(""); setCitation(""); setError("");
    void (async () => {
      const styled = format === "apa" || format === "vancouver" || format === "gb7714";
      const response = await gatewayRequest("POST", "/bib/citations/render", { article_ids: articleIds, format: styled ? "csl_json" : format });
      if (!active) return;
      if (styled) {
        const rendered = await renderCitations(response.items, format as CitationStyle);
        if (active) { setOutput(rendered.bibliography); setCitation(rendered.citation); }
      } else setOutput(response.export);
    })().catch(failure => { if (active) setError(String(failure)); }).finally(() => { if (active) setBusy(false); });
    return () => { active = false; };
  }, [mode, format, JSON.stringify(articleIds)]);
  const preview = async () => {
    setBusy(true); setError(""); setStatus("");
    try {
      const response = await gatewayRequest("POST", "/bib/import/preview", { format, contents });
      setRows(response.records); setSelected(response.records.filter((row: PreviewRow) => !row.duplicate_id && row.duplicate_in_file === null).map((row: PreviewRow) => row.index));
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  const commit = async () => {
    setBusy(true); setError("");
    try {
      const response = await gatewayRequest("POST", "/bib/import/commit", { format, contents, selected_indices: selected, collection_id: collectionId || null });
      setStatus(t("libraryTransfer.importedStatus", { imported: response.imported_ids.length, skipped: response.skipped_ids.length }));
      setRows([]); setSelected([]); onImported();
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  const copy = async (text: string) => {
    try { await navigator.clipboard.writeText(text); setStatus(t("libraryTransfer.copied")); } catch { setError(t("libraryTransfer.clipboardUnavailable")); }
  };
  return <SearchDialog title={mode === "import" ? t("libraryTransfer.importTitle") : t("libraryTransfer.exportTitle")} onClose={() => { if (!busy) onClose(); }} footer={
    <div className="flex flex-wrap justify-end gap-2">
      {mode === "import" ? <>
        <Button variant="outline" disabled={busy || !contents.trim()} onClick={() => void preview()}>{t("libraryTransfer.previewImport")}</Button>
        <Button disabled={busy || !selected.length} onClick={() => void commit()}><Upload />{t("libraryTransfer.importSelected", { count: selected.length })}</Button>
      </> : <>
        {citation && <Button variant="outline" disabled={busy} onClick={() => void copy(citation)}><Copy />{t("libraryTransfer.copyCitation")}</Button>}
        <Button variant="outline" disabled={busy || !output} onClick={() => void copy(output)}><Copy />{t("libraryTransfer.copyBibliography")}</Button>
        <Button disabled={busy || !output} onClick={() => void saveTextFile(`references.${({ bibtex: "bib", ris: "ris", csl_json: "json", markdown: "md" } as Record<string,string>)[format] || "txt"}`, output).catch(failure => setError(String(failure)))}><Download />{t("libraryTransfer.download")}</Button>
      </>}
    </div>
  }>
    <div className="space-y-3">
      <label className="flex items-center gap-2">{t("libraryTransfer.format")}
        <select aria-label={t("libraryTransfer.formatAria")} disabled={busy} className="rounded border bg-background p-2" value={format} onChange={event => { setFormat(event.target.value); invalidate(); }}>
          {mode === "export" && <><option value="apa">{t("libraryTransfer.formatApa")}</option><option value="vancouver">{t("libraryTransfer.formatVancouver")}</option><option value="gb7714">{t("libraryTransfer.formatGb7714")}</option></>}
          <option value="bibtex">BibTeX</option><option value="ris">RIS</option>
          {mode === "import" ? <option value="zotero_json">{t("libraryTransfer.formatZoteroJson")}</option> : <><option value="csl_json">CSL-JSON</option><option value="markdown">Markdown</option></>}
        </select>
      </label>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {status && <p role="status">{status}</p>}
      {busy && <p role="status">{t("libraryTransfer.working")}</p>}
      {mode === "import" ? <>
        <p className="text-xs text-muted-foreground">{t("libraryTransfer.importHint")}</p>
        {format === "zotero_json" && <p className="text-xs">{t("libraryTransfer.zoteroJsonHint")}</p>}
        <input aria-label={t("libraryTransfer.fileAria")} type="file" accept=".bib,.ris,.json,.txt" disabled={busy} onChange={event => {
          const file = event.target.files?.[0]; if (!file) return;
          invalidate(); if (file.size > 8 * 1024 * 1024) { setError(t("libraryTransfer.fileTooLarge")); return; }
          void file.text().then(setContents).catch(failure => setError(String(failure)));
        }} />
        <textarea aria-label={t("libraryTransfer.contentsAria")} className="min-h-36 w-full rounded border bg-background p-2 font-mono text-xs" disabled={busy} value={contents} onChange={event => { setContents(event.target.value); invalidate(); }} placeholder={t("libraryTransfer.contentsPlaceholder")} />
        {rows.map(row => <label key={row.index} className="flex gap-2 rounded border p-2">
          <input type="checkbox" aria-label={t("libraryTransfer.importAria", { title: row.article.title })} disabled={busy || Boolean(row.duplicate_id) || row.duplicate_in_file !== null} checked={selected.includes(row.index)} onChange={event => setSelected(current => event.target.checked ? [...current, row.index] : current.filter(index => index !== row.index))} />
          <span><span className="block">{row.article.title}</span>
            {(row.duplicate_id || row.duplicate_in_file !== null) && <span className="block text-xs">{t("libraryTransfer.duplicateSkipped")}</span>}
            {row.warnings.map((warning, index) => <span key={index} className="block text-xs text-muted-foreground">{warning}</span>)}
          </span>
        </label>)}
      </> : <>
        <p className="text-xs text-muted-foreground">{t("libraryTransfer.exportHint", { count: articleIds.length })}</p>
        {citation && <p aria-label={t("libraryTransfer.citationPreviewAria")} className="rounded border p-2">{citation}</p>}
        <textarea aria-label={t("libraryTransfer.bibliographyPreviewAria")} readOnly value={output} className="min-h-64 w-full rounded border bg-background p-2 text-sm" />
      </>}
    </div>
  </SearchDialog>;
}
