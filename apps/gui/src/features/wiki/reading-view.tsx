/**
 * SiYuan-style 阅读模式: the fully rendered document with live wikilinks
 * (resolved against the wiki — missing targets become one-click create
 * prompts), copy-able code blocks, and the 阅读/编辑 view switch that
 * SiYuan exposes per document.
 */
import { BookOpen, SquarePen, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { extractWikiTargets, renderWikiMarkdown } from "./markdown";
import { findEntryByTitle, formatTime, type WikiEntry } from "./wiki-api";

export type WikiMode = "read" | "edit";

/** SiYuan-style per-document view switch (阅读模式 / 编辑模式). */
export function ModeSwitch({
  mode,
  onChange,
}: {
  mode: WikiMode;
  onChange: (mode: WikiMode) => void;
}) {
  const item = (active: boolean) =>
    `flex items-center gap-1 rounded px-2 py-1 text-xs transition-colors ${
      active ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:text-foreground"
    }`;
  return (
    <span className="inline-flex items-center rounded-md border p-0.5" role="group" aria-label="视图切换">
      <button
        type="button"
        className={item(mode === "read")}
        aria-pressed={mode === "read"}
        title="阅读模式"
        onClick={() => onChange("read")}
      >
        <BookOpen className="h-3.5 w-3.5" /> 阅读
      </button>
      <button
        type="button"
        className={item(mode === "edit")}
        aria-pressed={mode === "edit"}
        title="编辑模式"
        onClick={() => onChange("edit")}
      >
        <SquarePen className="h-3.5 w-3.5" /> 编辑
      </button>
    </span>
  );
}

export function ReadingView({
  entry,
  onModeChange,
  onDelete,
  onOpenEntry,
  onMissingLink,
}: {
  entry: WikiEntry;
  onModeChange: (mode: WikiMode) => void;
  onDelete: () => void;
  onOpenEntry: (id: string) => void;
  onMissingLink: (target: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [linkExists, setLinkExists] = useState<Record<string, boolean>>({});

  // Resolve every wikilink target once per content revision; the DOM pass
  // below paints missing ones as create prompts (SiYuan's broken-link look).
  useEffect(() => {
    let cancelled = false;
    const targets = extractWikiTargets(entry.content);
    setLinkExists({});
    void Promise.all(
      targets.map(async (target) => {
        try {
          const found = await findEntryByTitle(target);
          return [target, Boolean(found)] as const;
        } catch {
          return [target, false] as const;
        }
      }),
    ).then((pairs) => {
      if (!cancelled) setLinkExists(Object.fromEntries(pairs));
    });
    return () => {
      cancelled = true;
    };
  }, [entry.id, entry.content]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    container.querySelectorAll<HTMLElement>("[data-wikilink]").forEach((node) => {
      const known = linkExists[node.dataset.wikilink ?? ""];
      node.classList.toggle("md-wikilink--missing", known === false);
      node.classList.toggle("md-wikilink--resolved", known === true);
    });
  }, [linkExists, entry.content]);

  // Click routing for rendered content: code copy buttons and wikilinks.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const onClick = (event: MouseEvent) => {
      const hit = event.target instanceof HTMLElement ? event.target : null;
      const copyButton = hit?.closest("[data-copy-code]");
      if (copyButton instanceof HTMLElement) {
        const code = copyButton.closest(".md-code")?.querySelector("pre");
        if (code) {
          void navigator.clipboard.writeText(code.textContent ?? "");
          copyButton.textContent = "已复制";
          window.setTimeout(() => {
            copyButton.textContent = "Copy";
          }, 1400);
        }
        return;
      }
      const link = hit?.closest<HTMLElement>("[data-wikilink]");
      if (!link) return;
      const target = link.dataset.wikilink;
      if (!target) return;
      void (async () => {
        try {
          const found = await findEntryByTitle(target);
          if (found) onOpenEntry(found.id);
          else onMissingLink(target);
        } catch (error) {
          toast.error((error as Error).message);
        }
      })();
    };
    container.addEventListener("click", onClick);
    return () => container.removeEventListener("click", onClick);
  }, [onOpenEntry, onMissingLink]);

  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="pb-2">
        <div className="flex items-start justify-between gap-2">
          <h1 className="min-w-0 break-words text-lg font-semibold">{entry.title}</h1>
          <span className="flex shrink-0 items-center gap-1">
            <ModeSwitch mode="read" onChange={onModeChange} />
            <Button size="icon-sm" variant="outline" title="移入回收站" onClick={() => void onDelete()}>
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          {entry.tags.map((tag) => (
            <Badge key={tag} variant="secondary">{tag}</Badge>
          ))}
          {entry.alias.map((name) => (
            <Badge key={name} variant="outline">alias: {name}</Badge>
          ))}
          <span className="ml-auto">
            创建 {formatTime(entry.ctime)} · 修改 {formatTime(entry.mtime)}
          </span>
        </div>
      </CardHeader>
      <CardContent className="min-h-0 flex-1">
        <ScrollArea className="h-full pr-3">
          {/* Wikilinks/`<a data-wikilink>` survive DOMPurify (data attrs
              allowed) — clicks are routed by the delegated handler above. */}
          <div
            ref={containerRef}
            className="md max-w-none break-words text-sm"
            dangerouslySetInnerHTML={{ __html: renderWikiMarkdown(entry.content) }}
          />
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
