/**
 * Reader editor body: hosts the imperative PDF/TXT reader inside an
 * ephemeral workbench editor tab.
 *
 * Lifecycle — readers keep ONE global session (reader-target), so the kind
 * mounts "ephemeral": the body exists only while its tab is the group's
 * active tab; deactivation destroys the reader (state persists via
 * saveReadingLocation) and reactivation reopens it at the stored position.
 * The reader's own close button destroys as well; `onClosed` then closes
 * the whole tab. A reader superseded by another one (split view) is
 * treated the same way — its tab closes, reopening goes through the
 * bibliography.
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
  const [superseded, setSuperseded] = useState(false);
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
          // The reader destroyed while we are still mounted (its own close
          // button, or superseded by another reader) → close the tab. The
          // wrapper's own cleanup sets disposedRef first and swallows it.
          onClosed: () => {
            if (!disposedRef.current) closeEditor(editor.id);
          },
        });
        if (disposedRef.current) handle.close();
      } catch (cause) {
        if (cause instanceof DOMException && cause.name === "AbortError") {
          // Superseded during load — another reader won the session while
          // this body is still mounted (split view): offer a reopen.
          setSuperseded(true);
          return;
        }
        setError(String(cause));
      }
    })();
    return () => {
      disposedRef.current = true;
      handle?.close();
      handle = null;
    };
    // search is part of the editor identity; params object identity is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [params.articleId, params.attachmentKey, params.title, params.expectedHash, reopenTick]);

  if (error) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
        <p className="text-sm text-destructive">阅读器打开失败:{error}</p>
        <Button variant="outline" size="sm" onClick={() => { setError(null); setSuperseded(false); setReopenTick((tick) => tick + 1); }}>
          <RotateCw className="size-3.5" />重试
        </Button>
      </div>
    );
  }
  if (superseded) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
        <p className="text-sm text-muted-foreground">该阅读器已被另一篇文献的阅读器接管。</p>
        <Button variant="outline" size="sm" onClick={() => { setSuperseded(false); setReopenTick((tick) => tick + 1); }}>
          <RotateCw className="size-3.5" />重新打开
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
    // One global reader session — the body only exists while the tab is
    // active; reactivation reopens from the persisted reading position.
    mount: "ephemeral",
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
