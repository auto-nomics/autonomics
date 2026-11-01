/**
 * 检索弹窗:BM25 全文检索 + 标签过滤(逗号分隔,AND)收进一个 popup,
 * 250ms 防抖即时搜索;首页不再常驻检索栏。结果行点击在新标签页打开
 * 词条并关闭弹窗(思源式全局搜索),浏览列表不被打断。
 */
import { Loader2, Search, Tag } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  formatTime,
  getEntry,
  listEntries,
  searchEntries,
  type EntrySummary,
  type SearchHit,
} from "./wiki-api";

/** 弹窗内检索的结果封顶(BM25 与标签浏览同限)。 */
const SEARCH_LIMIT = 50;
const DEBOUNCE_MS = 250;

const parseTags = (raw: string) =>
  raw.split(",").map((tag) => tag.trim()).filter(Boolean);

export function SearchDialog({
  open,
  onOpenChange,
  onOpen,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onOpen: (id: string, title: string, newTab: boolean) => void;
}) {
  const [query, setQuery] = useState("");
  const [tags, setTags] = useState("");
  const [results, setResults] = useState<EntrySummary[] | null>(null);
  const [loading, setLoading] = useState(false);
  const runIdRef = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const run = useCallback(async (term: string, tagRaw: string) => {
    const runId = ++runIdRef.current;
    setLoading(true);
    try {
      const parsed = parseTags(tagRaw);
      const hits: SearchHit[] = term.trim()
        ? await searchEntries(term, { limit: SEARCH_LIMIT, tags: parsed })
        : parsed.length
          ? (await listEntries({ tags: parsed, limit: SEARCH_LIMIT })).map((entry) => ({
            entry_id: entry.id,
            score: 0,
          }))
          : [];
      // BM25 只回 id+score,逐条回填摘要;取数中被净删的丢弃。
      const hydrated = await Promise.all(
        hits.map(async (hit) => {
          try {
            const entry = await getEntry(hit.entry_id);
            const { content: _content, ...summary } = entry;
            return summary;
          } catch {
            return null;
          }
        }),
      );
      if (runId !== runIdRef.current) return;
      setResults(hydrated.filter((item): item is EntrySummary => item !== null));
    } catch (error) {
      if (runId !== runIdRef.current) return;
      setResults([]);
      toast.error((error as Error).message);
    } finally {
      if (runId === runIdRef.current) setLoading(false);
    }
  }, []);

  // 打开时聚焦检索框;空查询+空标签时不发请求,展示提示。
  useEffect(() => {
    if (!open) return;
    inputRef.current?.focus();
    const hasTerm = query.trim().length > 0;
    const hasTags = parseTags(tags).length > 0;
    if (!hasTerm && !hasTags) {
      runIdRef.current += 1;
      setLoading(false);
      setResults(null);
      return;
    }
    const timer = window.setTimeout(() => void run(query, tags), DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [open, query, tags, run]);

  const openResult = (item: EntrySummary) => {
    onOpenChange(false);
    onOpen(item.id, item.title, true);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="top-[12%] max-w-[calc(100%-2rem)] translate-y-0 gap-3 sm:max-w-lg">
        <DialogHeader className="sr-only">
          <DialogTitle>检索词条</DialogTitle>
          <DialogDescription>BM25 全文检索与标签过滤。</DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-2">
          <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
          <Input
            ref={inputRef}
            className="h-9 border-0 bg-transparent px-0 shadow-none focus-visible:ring-0"
            placeholder="全文检索(BM25,中英文)…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void run(query, tags);
            }}
          />
          {loading ? <Loader2 className="h-4 w-4 shrink-0 animate-spin text-muted-foreground" /> : null}
        </div>
        <div className="flex items-center gap-2 border-b pb-2">
          <Tag className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <Input
            className="h-7 border-0 bg-transparent px-0 text-xs shadow-none focus-visible:ring-0"
            placeholder="标签过滤(逗号分隔,AND),可与关键词同用"
            value={tags}
            onChange={(event) => setTags(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void run(query, tags);
            }}
          />
        </div>
        {results === null ? (
          <p className="py-6 text-center text-xs text-muted-foreground">
            输入关键词或标签开始检索,结果最多 {SEARCH_LIMIT} 条。
          </p>
        ) : results.length === 0 && !loading ? (
          <p className="py-6 text-center text-sm text-muted-foreground">没有匹配的词条。</p>
        ) : (
          <ScrollArea className="-mr-2 max-h-[min(20rem,45vh)] pr-2">
            <div role="list" aria-label="检索结果">
              {results.map((item) => (
                <div key={item.id} role="listitem">
                  <button
                    type="button"
                    className="flex h-9 w-full items-center gap-2 border-b border-border/60 px-2 text-left transition-colors hover:bg-accent/70"
                    onClick={() => openResult(item)}
                  >
                    <span className="min-w-0 flex-1 truncate text-[13px]">{item.title}</span>
                    <span className="shrink-0 text-xs text-muted-foreground">{formatTime(item.mtime)}</span>
                    {(item.tags ?? []).slice(0, 3).map((tag) => (
                      <Badge key={tag} variant="secondary" className="shrink-0 px-1 py-0 text-[10px]">
                        {tag}
                      </Badge>
                    ))}
                  </button>
                </div>
              ))}
            </div>
          </ScrollArea>
        )}
        {results !== null && results.length > 0 ? (
          <p className="text-xs text-muted-foreground">{results.length} 条结果,点击在新标签页打开。</p>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
