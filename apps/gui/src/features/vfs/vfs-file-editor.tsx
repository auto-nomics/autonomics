/**
 * `vfs-file` editor body: one tab per virtual file. Text files show the
 * contents (read-only view until the write endpoints land; editing +
 * Ctrl+S save arrives with them); images preview inline; binary files
 * fall back to a download panel. Read-only mounts always render the
 * read-only surface with a lock badge.
 */

import { useEffect, useState } from "react";
import { FileText, Lock } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { EditorRef } from "@/app/workbench/workbench-store";
import { downloadVfsFile, formatBytes, isImagePath, readTextFile, vfsFileUrl } from "./vfs-api";

type Load =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "text"; contents: string; size: number; truncated: boolean }
  | { state: "image" }
  | { state: "binary"; size?: number };

export function VfsFileEditorBody({ editor }: { editor: EditorRef }) {
  const path = String(editor.params.path ?? "");
  const readOnly = Boolean(editor.params.readOnly);
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    setLoad({ state: "loading" });
    setError("");
    if (isImagePath(path)) {
      setLoad({ state: "image" });
      return () => { cancelled = true; };
    }
    void readTextFile(path)
      .then((result) => {
        if (cancelled) return;
        setLoad({ state: "text", contents: result.contents, size: result.size, truncated: result.truncated });
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        const message = String((cause as { message?: string } | null)?.message ?? cause);
        // Non-UTF-8 (400) or too large: offer the raw download instead.
        if (message.includes("UTF-8") || message.includes("400")) {
          setLoad({ state: "binary" });
          return;
        }
        setLoad({ state: "error", message });
      });
    return () => { cancelled = true; };
  }, [path]);

  const download = async () => {
    setSaving(true);
    setError("");
    try {
      await downloadVfsFile(path);
    } catch (cause: unknown) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section aria-label={`VFS 文件 ${path}`} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex items-center gap-2 border-b px-4 py-2 text-sm">
        <FileText className="size-4 shrink-0 text-primary" />
        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={path} data-vfs-file-path={path}>{path}</span>
        {readOnly && (
          <span className="flex shrink-0 items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground" title="该文件位于只读挂载">
            <Lock className="size-3" /> 只读挂载
          </span>
        )}
        {load.state === "text" && (
          <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
            {formatBytes(load.size)} · {load.contents.length} 字符
          </span>
        )}
        <Button size="sm" variant="outline" disabled={saving} onClick={() => void download()}>下载</Button>
      </header>
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">{error}</div>}
      <div className="flex min-h-0 flex-1 flex-col">
        {load.state === "loading" && <p className="p-4 text-sm text-muted-foreground">加载中…</p>}
        {load.state === "error" && <p className="p-4 text-sm text-destructive" role="alert">{load.message}</p>}
        {load.state === "text" && (
          <>
            {load.truncated && (
              <div role="status" className="border-b bg-amber-500/10 px-4 py-1.5 text-xs">
                文件较大，仅显示开头部分；完整内容请下载。
              </div>
            )}
            <textarea
              aria-label={`查看 ${path}`}
              readOnly
              spellCheck={false}
              className="min-h-0 w-full flex-1 resize-none bg-background p-4 font-mono text-sm leading-6 outline-none"
              value={load.contents}
            />
          </>
        )}
        {load.state === "image" && (
          <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-4">
            <img src={vfsFileUrl(path)} alt={path} className="max-h-full max-w-full object-contain" />
          </div>
        )}
        {load.state === "binary" && (
          <div className="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center text-sm text-muted-foreground">
            <p>该文件不是 UTF-8 文本（或超出文本预览上限）。</p>
            <Button size="sm" variant="outline" disabled={saving} onClick={() => void download()}>下载文件</Button>
          </div>
        )}
      </div>
    </section>
  );
}
