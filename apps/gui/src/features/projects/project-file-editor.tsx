/**
 * Shared per-file editor over the project file endpoints: read with
 * optimistic-concurrency save (`original` → 409), dirty tracking,
 * Ctrl+S. Used by the plugin manager and the dev window.
 */

import { useCallback, useEffect, useState } from "react";
import { Save } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";

export function ProjectFileEditor({
  project,
  file,
  onSaved,
}: {
  project: string;
  file: string;
  onSaved?: () => void;
}) {
  const [contents, setContents] = useState("");
  const [original, setOriginal] = useState("");
  const [error, setError] = useState("");
  const [conflict, setConflict] = useState(false);
  const [busy, setBusy] = useState(false);
  const dirty = contents !== original;

  useEffect(() => {
    let cancelled = false;
    setContents("");
    setOriginal("");
    setError("");
    setConflict(false);
    gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/file?path=${encodeURIComponent(file)}`)
      .then((result) => {
        if (cancelled) return;
        setContents(result.contents);
        setOriginal(result.contents);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String((cause as { message?: string } | null)?.message ?? cause));
      });
    return () => { cancelled = true; };
  }, [project, file]);

  const reload = useCallback(async () => {
    setBusy(true);
    try {
      const result = await gatewayRequest(
        "GET",
        `/plugin-projects/${encodeURIComponent(project)}/file?path=${encodeURIComponent(file)}`,
      );
      setContents(result.contents);
      setOriginal(result.contents);
      setConflict(false);
      setError("");
    } finally {
      setBusy(false);
    }
  }, [project, file]);

  const save = useCallback(async () => {
    if (!dirty || busy) return;
    setBusy(true);
    setError("");
    setConflict(false);
    try {
      await gatewayRequest(
        "PUT",
        `/plugin-projects/${encodeURIComponent(project)}/file`,
        { path: file, contents, original },
      );
      setOriginal(contents);
      onSaved?.();
    } catch (cause: unknown) {
      const message = String((cause as { message?: string; status?: number } | null)?.message ?? cause);
      const status = (cause as { status?: number } | null)?.status;
      if (status === 409 || message.includes("reload")) {
        setConflict(true);
      } else {
        setError(message);
      }
    } finally {
      setBusy(false);
    }
  }, [project, file, contents, original, dirty, busy, onSaved]);

  return (
    <section
      aria-label={`编辑 ${file}`}
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      onKeyDown={(event) => {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
          event.preventDefault();
          void save();
        }
      }}
    >
      <header className="flex items-center gap-2 border-b px-3 py-1.5 text-sm">
        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={file}>{file}{dirty ? " •" : ""}</span>
        <Button size="sm" variant="outline" disabled={!dirty || busy} onClick={() => void save()}>
          <Save /> 保存
        </Button>
      </header>
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-3 py-1 text-xs text-destructive">{error}</div>}
      {conflict && (
        <div role="alert" className="flex items-center gap-2 border-b bg-amber-500/10 px-3 py-1 text-xs">
          <span className="min-w-0 flex-1">文件已在磁盘上变更；重新加载会丢弃本地修改。</span>
          <Button size="sm" variant="outline" onClick={() => void reload()}>重新加载</Button>
        </div>
      )}
      <textarea
        aria-label={`编辑 ${file}`}
        data-project-file-editor={file}
        spellCheck={false}
        className="min-h-0 w-full flex-1 resize-none bg-background p-3 font-mono text-xs leading-5 outline-none"
        value={contents}
        onChange={(event) => setContents(event.target.value)}
      />
    </section>
  );
}
