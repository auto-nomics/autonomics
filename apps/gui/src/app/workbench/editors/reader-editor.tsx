/**
 * Reader editor body: hosts the imperative PDF/TXT reader inside a
 * keep-alive workbench editor tab.
 *
 * Lifecycle — readers coexist (per-tab sessions in reader-target), and the
 * kind mounts keep-alive with "visibility" occlusion: switching tabs keeps
 * the reader instance, its scroll position, zoom and rendered pages fully
 * alive (only invisible + inert). The body mounts once per tab; only the
 * reader's own close button (onClosed → close the tab) or a param change
 * (e.g. an evidence jump into the same attachment) tears it down. Group
 * moves (split) still remount the body — the reader then reopens from the
 * persisted reading position.
 */

import { useEffect, useRef, useState } from "react";
import { FileText, Loader2, RotateCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { openPdfReader } from "@/features/reader";
import { registerReaderOpener } from "@/features/reader/reader-opener";
import type { ReaderSearchTarget } from "@/features/reader/reader-target";
import { gatewayRequest } from "@/gateway";
import type { BibArticle } from "@/types";
import { useContextBasket } from "@/features/context/context-basket";
import { registerEditorKind, type EditorContext } from "../editor-registry";
import { closeEditor, openEditor } from "../workbench-store";

export type ReaderEditorParams = {
  articleId: string;
  title: string;
  attachmentKey?: string | null;
  expectedHash?: string | null;
  search?: ReaderSearchTarget | null;
};

function ReaderEditorBody({ editor }: EditorContext) {
  const params = editor.params as ReaderEditorParams;
  const basket = useContextBasket();
  // The open effect captures callbacks once; reads of the basket must
  // always see the CURRENT agent scope (mirrors bibliography's
  // contextBasketRef pattern).
  const basketRef = useRef(basket);
  basketRef.current = basket;
  const hostRef = useRef<HTMLDivElement | null>(null);
  const disposedRef = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [reopenTick, setReopenTick] = useState(0);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    disposedRef.current = false;
    let handle: { close: () => void } | null = null;
    void (async () => {
      try {
        // The reader's info pane renders article metadata when provided;
        // tolerate a failed fetch (null article = reader fetches less).
        let article: BibArticle | null = null;
        try {
          const response = await gatewayRequest("GET", `/bib/articles/${encodeURIComponent(params.articleId)}`);
          article = response?.article ?? null;
        } catch {
          article = null;
        }
        if (disposedRef.current) return;
        handle = await openPdfReader({
          // Tabs coexist: never supersede other live readers.
          supersede: false,
          articleId: params.articleId,
          title: params.title,
          article,
          attachmentKey: params.attachmentKey ?? null,
          expectedHash: params.expectedHash ?? null,
          search: params.search ?? undefined,
          host,
          onReference: (selection) => {
            const current = basketRef.current;
            if (!current?.available) { window.alert("请先选择 Agent 会话"); return; }
            current.add({
              kind: "evidence",
              id: JSON.stringify([params.articleId, params.attachmentKey ?? null, selection.page ?? null, selection.start ?? null, selection.end ?? null, selection.text]),
              title: `${params.title} · 选中文本`,
              scope: "阅读器选中文本（非全文）",
              source: `bib:${params.articleId}${params.attachmentKey ? ` · ${params.attachmentKey}` : ""}${selection.page ? ` · 第 ${selection.page} 页` : ""}${selection.start !== undefined ? ` · 字符 ${selection.start}–${selection.end}` : ""}`,
              content: selection.text,
            });
          },
          // The reader destroyed while we are still mounted (its own
          // close button) → close the tab. The wrapper's own cleanup sets
          // disposedRef first and swallows the event.
          onClosed: () => {
            if (!disposedRef.current) closeEditor(editor.id);
          },
        });
        if (disposedRef.current) handle.close();
      } catch (cause) {
        // AbortError = this tab was closed while the reader was still
        // loading — the unmount already happened, nothing to report.
        if (!(cause instanceof DOMException && cause.name === "AbortError")) setError(String(cause));
      }
    })();
    return () => {
      disposedRef.current = true;
      handle?.close();
      handle = null;
    };
    // The search target re-applies on change (evidence jumps into an
    // already-open attachment re-target the same tab).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [params.articleId, params.attachmentKey, params.title, params.expectedHash, JSON.stringify(params.search ?? null), reopenTick]);

  if (error) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
        <p className="text-sm text-destructive">阅读器打开失败:{error}</p>
        <Button variant="outline" size="sm" onClick={() => { setError(null); setReopenTick((tick) => tick + 1); }}>
          <RotateCw className="size-3.5" />重试
        </Button>
      </div>
    );
  }
  return (
    <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
      {!error && <Loader2 className="pointer-events-none absolute left-1/2 top-1/2 -ml-4 -mt-4 size-8 animate-spin text-muted-foreground" />}
      <div ref={hostRef} className="relative flex min-h-0 min-w-0 flex-1 flex-col" data-reader-host={params.articleId} />
    </div>
  );
}

let registered = false;

/** Idempotent registration; called from registerWorkbenchEditorKinds. */
export function registerReaderEditorKind(): void {
  if (registered) return;
  registered = true;

  registerEditorKind({
    kind: "reader",
    agentContext: (editor) => {
      const params = editor.params as ReaderEditorParams;
      return { articleId: params.articleId, title: params.title, attachmentKey: params.attachmentKey ?? null, expectedHash: params.expectedHash ?? null, scope: "阅读器文档标识，不含全文或当前选区" };
    },
    title: (editor) => {
      const params = editor.params as ReaderEditorParams;
      return params.attachmentKey ? `${params.title} · ${params.attachmentKey}` : params.title;
    },
    name: (editor) => {
      const params = editor.params as ReaderEditorParams;
      return `${params.title}${params.attachmentKey ? ` (${params.attachmentKey})` : ""} — ${params.articleId}`;
    },
    icon: () => FileText,
    body: ReaderEditorBody,
    // Keep-alive with visibility occlusion: tab switches keep the reader
    // instance (scroll/zoom/pages) fully alive.
    mount: "keep-alive",
    hideMode: "visibility",
  });

  // Reader opener bridge: bibliography asks for a reader, we open a tab.
  registerReaderOpener((spec) => {
    openEditor({
      kind: "reader",
      params: {
        articleId: spec.articleId,
        title: spec.title,
        attachmentKey: spec.attachmentKey ?? null,
        expectedHash: spec.expectedHash ?? null,
        search: spec.search ?? null,
      },
      activate: true,
    });
  });
}
