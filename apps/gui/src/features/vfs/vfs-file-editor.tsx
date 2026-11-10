/**
 * `vfs-file` editor body: one tab per virtual file. Text files edit
 * inline with a conflict-checked Ctrl+S save (`original` 409 → reload
 * prompt); images preview inline; binary files fall back to a download
 * panel. Read-only mounts always render the read-only surface with a
 * lock badge. The dirty flag lives in `editor.state` so the workbench
 * tab-close veto can guard unsaved edits.
 */

import { useEffect, useState } from "react";
import { boundedContextText, useEditorContextSnapshot } from "@/app/workbench/editor-context-snapshots";
import { FileText, Lock, RotateCw, Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { patchEditorState, type EditorRef } from "@/app/workbench/workbench-store";
import { downloadVfsFile, formatBytes, isImagePath, readTextFile, saveTextFile, vfsFileUrl } from "./vfs-api";
import { reloadSubtree } from "./vfs-tree";

type Load =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "text"; contents: string; size: number; truncated: boolean }
  | { state: "image" }
  | { state: "binary" };

export function VfsFileEditorBody({ editor }: { editor: EditorRef }) {
  const path = String(editor.params.path ?? "");
  const readOnly = Boolean(editor.params.readOnly);
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [draft, setDraft] = useState("");
  const [original, setOriginal] = useState("");
  const [saving, setSaving] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [error, setError] = useState("");
  const dirty = load.state === "text" && draft !== original;
  useEditorContextSnapshot(editor.id, {
    path, readOnly, dirty, state: load.state,
    ...(load.state === "text" ? boundedContextText(draft) : {}),
    scope: load.state === "text" ? "当前文件内容，包含未保存的修改（最多 24000 字符）" : "文件标识与加载状态",
  });

  useEffect(() => {
    patchEditorState(editor.id, { dirty, path });
  }, [editor.id, dirty, path]);

  useEffect(() => {
    let cancelled = false;
    setLoad({ state: "loading" });
    setError("");
    setConflict(false);
    if (isImagePath(path)) {
      setLoad({ state: "image" });
      return () => { cancelled = true; };
    }
    void readTextFile(path)
      .then((result) => {
        if (cancelled) return;
        setLoad({ state: "text", contents: result.contents, size: result.size, truncated: result.truncated });
        setDraft(result.contents);
        setOriginal(result.contents);
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

  const reload = async () => {
    const result = await readTextFile(path);
    setLoad({ state: "text", contents: result.contents, size: result.size, truncated: result.truncated });
    setDraft(result.contents);
    setOriginal(result.contents);
    setConflict(false);
    setError("");
  };

  const save = async () => {
    if (load.state !== "text" || !dirty || saving) return;
    setSaving(true);
    setError("");
    setConflict(false);
    try {
      await saveTextFile(path, draft, original);
      setOriginal(draft);
      // Refresh the sidebar's view of the containing directory.
      const trimmed = path.replace(/\/+$/, "");
      const parent = trimmed.lastIndexOf("/") <= 0 ? "/" : trimmed.slice(0, trimmed.lastIndexOf("/"));
      void reloadSubtree(parent);
    } catch (cause: unknown) {
      const message = String((cause as { message?: string } | null)?.message ?? cause);
      if (message.includes("409") || message.includes("changed on the server")) {
        setConflict(true);
      } else {
        setError(message);
      }
    } finally {
      setSaving(false);
    }
  };

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
    <section
      aria-label={`VFS 文件 ${path}`}
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      onKeyDown={(event) => {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
          event.preventDefault();
          void save();
        }
      }}
    >
      <header className="flex items-center gap-2 border-b px-4 py-2 text-sm">
        <FileText className="size-4 shrink-0 text-primary" />
        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={path} data-vfs-file-path={path}>{path}{dirty ? " •" : ""}</span>
        {readOnly && (
          <span className="flex shrink-0 items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground" title="该文件位于只读挂载">
            <Lock className="size-3" /> 只读挂载
          </span>
        )}
        {load.state === "text" && (
          <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
            {formatBytes(load.size)} · {draft.length} 字符
          </span>
        )}
        {load.state === "text" && !readOnly && (
          <Button size="sm" variant="outline" disabled={!dirty || saving} onClick={() => void save()}>
            <Save /> 保存
          </Button>
        )}
        <Button size="sm" variant="outline" disabled={saving} onClick={() => void download()}>下载</Button>
      </header>
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">{error}</div>}
      {conflict && (
        <div role="alert" className="flex items-center gap-2 border-b bg-amber-500/10 px-4 py-2 text-xs">
          <span className="min-w-0 flex-1">文件在服务器上已变更(或保存窗口外被修改)，本次保存被拒绝。重新加载会丢弃本地修改。</span>
          <Button size="sm" variant="outline" onClick={() => void reload()}>
            <RotateCw /> 重新加载
          </Button>
        </div>
      )}
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
              aria-label={readOnly ? `查看 ${path}` : `编辑 ${path}`}
              readOnly={readOnly || load.truncated}
              spellCheck={false}
              data-vfs-editor={path}
              className="min-h-0 w-full flex-1 resize-none bg-background p-4 font-mono text-sm leading-6 outline-none"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
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
