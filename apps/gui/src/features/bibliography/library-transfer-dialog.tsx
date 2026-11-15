import { useEffect, useState } from "react";
import { Copy, Download, Upload } from "lucide-react";
import { gatewayRequest, saveTextFile } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";
import { renderCitations, type CitationStyle } from "./citation-renderer";
import type { BibArticle } from "@/types";
type PreviewRow = { index: number; article: BibArticle; warnings: string[]; duplicate_id: string | null; duplicate_in_file: number | null };
export function LibraryTransferDialog({ mode, articleIds = [], collectionId, onClose, onImported }: {
  mode: "import" | "export"; articleIds?: string[]; collectionId?: string | null; onClose: () => void; onImported: () => void;
}) {
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
      setStatus(`Imported ${response.imported_ids.length}; skipped ${response.skipped_ids.length} existing records.`);
      setRows([]); setSelected([]); onImported();
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  const copy = async (text: string) => {
    try { await navigator.clipboard.writeText(text); setStatus("Copied to clipboard."); } catch { setError("Clipboard unavailable. Select and copy the preview text, or download it."); }
  };
  return <SearchDialog title={mode === "import" ? "Import bibliography file" : "Export and cite"} onClose={() => { if (!busy) onClose(); }} footer={
    <div className="flex flex-wrap justify-end gap-2">
      {mode === "import" ? <>
        <Button variant="outline" disabled={busy || !contents.trim()} onClick={() => void preview()}>Preview import</Button>
        <Button disabled={busy || !selected.length} onClick={() => void commit()}><Upload />Import {selected.length} selected</Button>
      </> : <>
        {citation && <Button variant="outline" disabled={busy} onClick={() => void copy(citation)}><Copy />Copy citation</Button>}
        <Button variant="outline" disabled={busy || !output} onClick={() => void copy(output)}><Copy />Copy bibliography</Button>
        <Button disabled={busy || !output} onClick={() => void saveTextFile(`references.${({ bibtex: "bib", ris: "ris", csl_json: "json", markdown: "md" } as Record<string,string>)[format] || "txt"}`, output).catch(failure => setError(String(failure)))}><Download />Download</Button>
      </>}
    </div>
  }>
    <div className="space-y-3">
      <label className="flex items-center gap-2">Format
        <select aria-label="Bibliography format" disabled={busy} className="rounded border bg-background p-2" value={format} onChange={event => { setFormat(event.target.value); invalidate(); }}>
          {mode === "export" && <><option value="apa">APA 7 (CSL)</option><option value="vancouver">NLM / Vancouver (CSL)</option><option value="gb7714">GB/T 7714-2015 顺序编码 (CSL)</option></>}
          <option value="bibtex">BibTeX</option><option value="ris">RIS</option>
          {mode === "import" ? <option value="zotero_json">Zotero JSON metadata</option> : <><option value="csl_json">CSL-JSON</option><option value="markdown">Markdown</option></>}
        </select>
      </label>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {status && <p role="status">{status}</p>}
      {busy && <p role="status">Working…</p>}
      {mode === "import" ? <>
        <p className="text-xs text-muted-foreground">Preview before importing. Existing records are skipped. Up to 2,000 records / 8 MiB per file.</p>
        {format === "zotero_json" && <p className="text-xs">Zotero JSON imports metadata only. Files, notes, collection hierarchy and document citation links are not transferred.</p>}
        <input aria-label="Bibliography file" type="file" accept=".bib,.ris,.json,.txt" disabled={busy} onChange={event => {
          const file = event.target.files?.[0]; if (!file) return;
          invalidate(); if (file.size > 8 * 1024 * 1024) { setError("File exceeds 8 MiB."); return; }
          void file.text().then(setContents).catch(failure => setError(String(failure)));
        }} />
        <textarea aria-label="Import contents" className="min-h-36 w-full rounded border bg-background p-2 font-mono text-xs" disabled={busy} value={contents} onChange={event => { setContents(event.target.value); invalidate(); }} placeholder="Paste BibTeX, RIS, or Zotero JSON here" />
        {rows.map(row => <label key={row.index} className="flex gap-2 rounded border p-2">
          <input type="checkbox" aria-label={`Import ${row.article.title}`} disabled={busy || Boolean(row.duplicate_id) || row.duplicate_in_file !== null} checked={selected.includes(row.index)} onChange={event => setSelected(current => event.target.checked ? [...current, row.index] : current.filter(index => index !== row.index))} />
          <span><span className="block">{row.article.title}</span>
            {(row.duplicate_id || row.duplicate_in_file !== null) && <span className="block text-xs">Duplicate — skipped</span>}
            {row.warnings.map((warning, index) => <span key={index} className="block text-xs text-muted-foreground">{warning}</span>)}
          </span>
        </label>)}
      </> : <>
        <p className="text-xs text-muted-foreground">{articleIds.length} selected records. APA, NLM and GB/T 7714 formatting uses bundled CSL styles locally.</p>
        {citation && <p aria-label="Citation preview" className="rounded border p-2">{citation}</p>}
        <textarea aria-label="Bibliography preview" readOnly value={output} className="min-h-64 w-full rounded border bg-background p-2 text-sm" />
      </>}
    </div>
  </SearchDialog>;
}
