/**
 * 首页 tab:纯浏览入口 —— 列出全部词条,服务端每页封顶 200 条,
 * 「加载更多」用 offset 续拉直到短页。检索/标签过滤已收进检索弹窗
 * (search-dialog),点击结果在新标签页打开,浏览位置不被打断。
 * 视觉与文献库一致:无卡片,密铺行(h-9 细线分隔 + hover 加亮)。
 */
import { Loader2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { formatTime, type EntrySummary } from "./wiki-api";

/** 服务端 `?limit=` 的硬上限(api-server clamp(1, 200))。 */
export const PAGE_SIZE = 200;

export function HomeView({
  entries,
  loading,
  hasMore,
  loadingMore,
  onLoadMore,
  onOpen,
}: {
  entries: EntrySummary[];
  loading: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  onOpen: (id: string, title: string, newTab: boolean) => void;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b px-2 text-xs text-muted-foreground">
        <span>
          共 {entries.length} 条词条
        </span>
        <span className="opacity-70"> 检索(Ctrl+K)</span>
      </div>
      <ScrollArea className="min-h-0 flex-1" contentClassName="pb-6">
        {loading ? (
          <div className="space-y-px">
            {Array.from({ length: 12 }).map((_, index) => (
              <div key={index} className="h-9 animate-pulse border-b bg-muted/30" />
            ))}
          </div>
        ) : entries.length === 0 ? (
          <div className="grid h-full place-items-center text-sm text-muted-foreground">
            还没有词条。
          </div>
        ) : (
          <div className="space-y-px" role="list" aria-label="词条列表">
            {entries.map((item) => (
              <div key={item.id} role="listitem">
                <button
                  type="button"
                  className="flex h-9 w-full items-center gap-2 border-b border-border/60 px-2 text-left transition-colors hover:bg-accent/70"
                  onClick={() => onOpen(item.id, item.title, true)}
                  onAuxClick={(event) => {
                    if (event.button === 1) {
                      event.preventDefault();
                      onOpen(item.id, item.title, true);
                    }
                  }}
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
        )}
        {hasMore && !loading ? (
          <div className="p-2 text-center">
            <Button
              size="sm"
              variant="ghost"
              disabled={loadingMore}
              onClick={onLoadMore}
            >
              {loadingMore ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
              加载更多(已显示 {entries.length} 条)
            </Button>
          </div>
        ) : null}
      </ScrollArea>
    </div>
  );
}
