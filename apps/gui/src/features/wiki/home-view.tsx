/**
 * 首页 tab: BM25 全文检索 + 标签过滤的浏览入口。服务端每页封顶 200 条,
 * 「加载更多」用 offset 续拉直到短页;点击结果在新标签页打开,浏览位置
 * 不被打断。
 */
import { Loader2, Search } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { formatTime, type EntrySummary } from "./wiki-api";

/** 服务端 `?limit=` 的硬上限(api-server clamp(1, 200))。 */
export const PAGE_SIZE = 200;

export function HomeView({
  entries,
  loading,
  hasMore,
  loadingMore,
  query,
  tagFilter,
  onQueryChange,
  onTagFilterChange,
  onSearch,
  onBrowse,
  onLoadMore,
  onOpen,
}: {
  entries: EntrySummary[];
  loading: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  query: string;
  tagFilter: string;
  onQueryChange: (value: string) => void;
  onTagFilterChange: (value: string) => void;
  onSearch: () => void;
  onBrowse: () => void;
  onLoadMore: () => void;
  onOpen: (id: string, title: string, newTab: boolean) => void;
}) {
  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="gap-2 pb-2">
        <div className="flex flex-wrap items-center gap-2">
          <Input
            className="h-8 max-w-md flex-1"
            placeholder="全文检索(BM25,中英文)…"
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") onSearch();
            }}
          />
          <Button size="sm" variant="ghost" title="搜索" onClick={onSearch}>
            <Search className="h-4 w-4" />
          </Button>
          <Input
            className="h-8 w-44"
            placeholder="标签过滤(逗号分隔,AND)"
            value={tagFilter}
            onChange={(event) => onTagFilterChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") onBrowse();
            }}
          />
          <Button size="sm" variant="outline" onClick={onBrowse}>
            浏览全部
          </Button>
        </div>
        <span className="text-xs text-muted-foreground">
          {entries.length} 条词条{hasMore ? "(可继续加载)" : ""}
        </span>
      </CardHeader>
      <CardContent className="min-h-0 flex-1">
        <ScrollArea className="h-full pr-2">
          {loading ? (
            <div className="space-y-2">
              {Array.from({ length: 6 }).map((_, index) => (
                <Skeleton key={index} className="h-12 w-full" />
              ))}
            </div>
          ) : entries.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">
              没有匹配的词条。
            </p>
          ) : (
            <ul className="space-y-1">
              {entries.map((item) => (
                <li key={item.id}>
                  <button
                    type="button"
                    className="w-full rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent"
                    onClick={() => onOpen(item.id, item.title, true)}
                    onAuxClick={(event) => {
                      if (event.button === 1) {
                        event.preventDefault();
                        onOpen(item.id, item.title, true);
                      }
                    }}
                  >
                    <div className="truncate text-sm font-medium">{item.title}</div>
                    <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                      <span>{formatTime(item.mtime)}</span>
                      {(item.tags ?? []).slice(0, 3).map((tag) => (
                        <Badge key={tag} variant="secondary" className="px-1 py-0 text-[10px]">
                          {tag}
                        </Badge>
                      ))}
                    </div>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {hasMore && !loading ? (
            <div className="pb-1 pt-2">
              <Button
                size="sm"
                variant="outline"
                className="w-full"
                disabled={loadingMore}
                onClick={onLoadMore}
              >
                {loadingMore ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                加载更多(已显示 {entries.length} 条)
              </Button>
            </div>
          ) : null}
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
