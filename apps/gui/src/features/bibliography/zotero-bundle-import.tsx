import { useRef, useState } from "react";
import { FolderInput } from "lucide-react";
import { useTranslation } from "react-i18next";
import { gatewayRequest, saveTextFile, uploadBibAttachment } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { matchBundleFile, parseZoteroBundle, zoteroMetadataWarnings, type ZoteroBundle } from "./zotero-bundle-model";
import { migrateZoteroBundle, type BundleReport } from "./zotero-bundle-runner";

export function ZoteroBundleImport({ onImported }: { onImported: () => void }) {
  const { t } = useTranslation("bibliography");
  const [open, setOpen] = useState(false);
  const [contents, setContents] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [sourceLabel, setSourceLabel] = useState(t("zoteroImport.defaultSourceLabel"));
  const [plan, setPlan] = useState<ZoteroBundle | null>(null);
  const [preview, setPreview] = useState<{ count: number; existing: number; matched: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [progress, setProgress] = useState("");
  const [report, setReport] = useState<BundleReport | null>(null);
  const sequence = useRef(0);
  const invalidate = () => { setPlan(null); setPreview(null); setReport(null); setError(""); };
  const load = async () => {
    setBusy(true); setError(""); setReport(null);
    try {
      if (!sourceLabel.trim()) throw new Error(t("zoteroImport.sourceLabelRequired"));
      const parsed = parseZoteroBundle(contents);
      const response = await gatewayRequest("POST", "/bib/import/preview", { format: "zotero_json", contents });
      if (response.records.length !== parsed.records.length) throw new Error(t("zoteroImport.mappingMismatch"));
      parsed.warnings = [...new Set([...parsed.warnings, ...zoteroMetadataWarnings(response.records)])];
      let matched = 0;
      for (const record of parsed.records) for (const attachment of record.attachments) {
        if (typeof attachment.data.path === "string" && matchBundleFile(attachment.data.path, files)) matched++;
      }
      setPlan(parsed); setPreview({ count: response.records.length, existing: response.records.filter((row: {duplicate_id?: string}) => row.duplicate_id).length, matched });
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  const commit = async () => {
    if (!plan || busy) return;
    setBusy(true); setError(""); setReport(null);
    try {
      const result = await migrateZoteroBundle({ contents, plan, files, sourceLabel,
        request: gatewayRequest, upload: uploadBibAttachment, progress: setProgress,
        noteText: html => {
          const template = document.createElement("template");
          template.innerHTML = html.replace(/<\/(?:p|div|li|h[1-6])\s*>|<br\s*\/?>/gi, "\n");
          template.content.querySelectorAll("script,style").forEach(node => node.remove());
          return template.content.textContent || "";
        },
      });
      setReport(result); setProgress(""); setPlan(null); setPreview(null); onImported();
    } catch (failure) { setError(String(failure)); setProgress(t("zoteroImport.stopped")); onImported(); }
    finally { setBusy(false); }
  };
  return <>
    <Button size="icon-sm" variant="ghost" title={t("zoteroImport.buttonTitle")} aria-label={t("zoteroImport.buttonTitle")} onClick={() => setOpen(true)}><FolderInput /></Button>
    {open && <SearchDialog title={t("zoteroImport.title")} onClose={() => { if (!busy) setOpen(false); }} footer={
      <div className="flex flex-wrap justify-end gap-2">
        {report && <Button variant="outline" onClick={() => void saveTextFile("zotero-migration-report.json", JSON.stringify(report, null, 2), "application/json").catch(failure => setError(String(failure)))}>{t("zoteroImport.downloadReport")}</Button>}
        <Button variant="outline" disabled={busy || !contents} onClick={() => void load()}>{t("zoteroImport.previewMigration")}</Button>
        <Button disabled={busy || !plan} onClick={() => void commit()}>{t("zoteroImport.importButton")}</Button>
      </div>
    }>
      <div className="space-y-4 text-sm">
        <p>{t("zoteroImport.intro")}</p>
        <label className="block space-y-1">{t("zoteroImport.sourceLabel")}<Input aria-label={t("zoteroImport.sourceLabel")} disabled={busy} maxLength={100} value={sourceLabel} onChange={event => { setSourceLabel(event.target.value); invalidate(); }} /><span className="text-xs text-muted-foreground">{t("zoteroImport.sourceLabelHint")}</span></label>
        <label className="block space-y-1">{t("zoteroImport.jsonExport")}<input aria-label={t("zoteroImport.jsonExport")} type="file" accept=".json,application/json" disabled={busy} onChange={event => {
          const file = event.target.files?.[0], current = ++sequence.current; invalidate(); setContents("");
          if (!file) return;
          if (file.size > 8 * 1024 * 1024) { setError(t("zoteroImport.jsonTooLarge")); return; }
          void file.text().then(value => { if (current === sequence.current) setContents(value); }).catch(failure => setError(String(failure)));
        }} /></label>
        <label className="block space-y-1">{t("zoteroImport.exportFolder")}<input aria-label={t("zoteroImport.exportFolderAria")} ref={node => { node?.setAttribute("webkitdirectory", ""); }} type="file" multiple disabled={busy} onChange={event => { setFiles(Array.from(event.target.files || [])); invalidate(); }} /><span className="text-xs text-muted-foreground">{t("zoteroImport.exportFolderHint", { count: files.length })}</span></label>
        {error && <p role="alert" className="text-destructive">{error}</p>}
        {progress && <p role="status">{progress}</p>}
        {busy && !progress && <p role="status">{t("zoteroImport.readingPreview")}</p>}
        {preview && plan && <div className="space-y-2 rounded border p-3">
          <p>{t("zoteroImport.previewCounts", { records: preview.count, existing: preview.existing, collections: plan.collections.length, notes: plan.records.reduce((sum, record) => sum + record.notes.length, 0) })}</p>
          <p>{t("zoteroImport.matchedFiles", { matched: preview.matched, total: plan.records.reduce((sum, record) => sum + record.attachments.length, 0) })}</p>
          <p className="text-xs">{t("zoteroImport.transferNote")}</p>
          {plan.warnings.map((warning, index) => <p key={index} className="text-xs text-muted-foreground">{warning}</p>)}
        </div>}
        {report && <div role="status" className="space-y-2 rounded border p-3">
          <p>{t("zoteroImport.reportSummary", { records: report.articles, collections: report.collections, notes: report.notes, files: report.attachments, skipped: report.skipped })}</p>
          {report.errors.length > 0 && <p className="text-destructive">{t("zoteroImport.reportErrors", { count: report.errors.length })}</p>}
          {[...report.errors, ...report.warnings].slice(0, 50).map((message, index) => <p key={index} className="break-words text-xs">{message}</p>)}
          {report.errors.length + report.warnings.length > 50 && <p>{t("zoteroImport.reportDownloadAll")}</p>}
        </div>}
      </div>
    </SearchDialog>}
  </>;
}
