import { useRef, useState } from "react";
import { FolderInput } from "lucide-react";
import { gatewayRequest, saveTextFile, uploadBibAttachment } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { matchBundleFile, parseZoteroBundle, zoteroMetadataWarnings, type ZoteroBundle } from "./zotero-bundle-model";
import { migrateZoteroBundle, type BundleReport } from "./zotero-bundle-runner";

export function ZoteroBundleImport({ onImported }: { onImported: () => void }) {
  const [open, setOpen] = useState(false);
  const [contents, setContents] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [sourceLabel, setSourceLabel] = useState("My Zotero library");
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
      if (!sourceLabel.trim()) throw new Error("Enter a source library label for repeatable migration.");
      const parsed = parseZoteroBundle(contents);
      const response = await gatewayRequest("POST", "/bib/import/preview", { format: "zotero_json", contents });
      if (response.records.length !== parsed.records.length) throw new Error("Source record mapping differs from the server preview. No changes were made.");
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
    } catch (failure) { setError(String(failure)); setProgress("Migration stopped. Completed steps remain saved; preview and retry to resume."); onImported(); }
    finally { setBusy(false); }
  };
  return <>
    <Button size="icon-sm" variant="ghost" title="Migrate Zotero library" aria-label="Migrate Zotero library" onClick={() => setOpen(true)}><FolderInput /></Button>
    {open && <SearchDialog title="Migrate Zotero library" onClose={() => { if (!busy) setOpen(false); }} footer={
      <div className="flex flex-wrap justify-end gap-2">
        {report && <Button variant="outline" onClick={() => void saveTextFile("zotero-migration-report.json", JSON.stringify(report, null, 2), "application/json").catch(failure => setError(String(failure)))}>Download report</Button>}
        <Button variant="outline" disabled={busy || !contents} onClick={() => void load()}>Preview migration</Button>
        <Button disabled={busy || !plan} onClick={() => void commit()}>Import metadata, notes and files</Button>
      </div>
    }>
      <div className="space-y-4 text-sm">
        <p>Select a Zotero JSON export, and optionally its exported file folder. Only files you explicitly select are uploaded. Existing metadata is preserved.</p>
        <label className="block space-y-1">Source library label<Input aria-label="Source library label" disabled={busy} maxLength={100} value={sourceLabel} onChange={event => { setSourceLabel(event.target.value); invalidate(); }} /><span className="text-xs text-muted-foreground">Use one unique label per source library, and reuse it on retries. Different libraries can contain identical Zotero item keys.</span></label>
        <label className="block space-y-1">Zotero JSON export<input aria-label="Zotero JSON export" type="file" accept=".json,application/json" disabled={busy} onChange={event => {
          const file = event.target.files?.[0], current = ++sequence.current; invalidate(); setContents("");
          if (!file) return;
          if (file.size > 8 * 1024 * 1024) { setError("JSON exceeds 8 MiB; split the export."); return; }
          void file.text().then(value => { if (current === sequence.current) setContents(value); }).catch(failure => setError(String(failure)));
        }} /></label>
        <label className="block space-y-1">Export folder (optional)<input aria-label="Zotero export folder" ref={node => { node?.setAttribute("webkitdirectory", ""); }} type="file" multiple disabled={busy} onChange={event => { setFiles(Array.from(event.target.files || [])); invalidate(); }} /><span className="text-xs text-muted-foreground">{files.length} selected files. Choose the export root containing the paths referenced in the JSON. Files over the 50 MiB upload limit are skipped.</span></label>
        {error && <p role="alert" className="text-destructive">{error}</p>}
        {progress && <p role="status">{progress}</p>}
        {busy && !progress && <p role="status">Reading migration preview…</p>}
        {preview && plan && <div className="space-y-2 rounded border p-3">
          <p>{preview.count} records ({preview.existing} already stored), {plan.collections.length} collections, {plan.records.reduce((sum, record) => sum + record.notes.length, 0)} notes.</p>
          <p>{preview.matched} of {plan.records.reduce((sum, record) => sum + record.attachments.length, 0)} referenced attachment files matched.</p>
          <p className="text-xs">Notes become plain text comments. PDF annotations, rich-text formatting and document citation links are not transferred. Missing files are reported and skipped. The migration saves steps incrementally; retry preserves existing notes and matching file contents.</p>
          {plan.warnings.map((warning, index) => <p key={index} className="text-xs text-muted-foreground">{warning}</p>)}
        </div>}
        {report && <div role="status" className="space-y-2 rounded border p-3">
          <p>Imported {report.articles} records, {report.collections} collections, {report.notes} notes and {report.attachments} files. Skipped {report.skipped} existing or unavailable objects.</p>
          {report.errors.length > 0 && <p className="text-destructive">{report.errors.length} steps failed. Review the report and retry; completed steps remain saved.</p>}
          {[...report.errors, ...report.warnings].slice(0, 50).map((message, index) => <p key={index} className="break-words text-xs">{message}</p>)}
          {report.errors.length + report.warnings.length > 50 && <p>Download the report for all messages.</p>}
        </div>}
      </div>
    </SearchDialog>}
  </>;
}
