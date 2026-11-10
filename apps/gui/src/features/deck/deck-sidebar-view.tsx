import { ViewLocationMenu } from "@/app/workbench/view-location-menu";
/**
 * DeckSidebarView — the "decks" sidebar container: deck list (title /
 * slide count / updated time), click opens the deck tab; "新建" opens a
 * title + template dialog. Wiki-sidebar shape, deck-sized.
 */
import { useCallback, useEffect, useState } from "react";
import { Loader2, Plus, Presentation, RefreshCw, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { createDeck, fetchDeck, fetchDeckList, type DeckSummary, type DeckTemplate } from "./deck-api";
import { openDeck } from "./deck-state";
import { DeckOutline, outlineOrPages } from "./deck-outline";
import { getGroup, useWorkbenchState } from "@/app/workbench/workbench-store";
import type { DeckPayload } from "./deck-api";

const TEMPLATES: readonly { id: DeckTemplate; label: string; hint: string }[] = [
  { id: "meeting", label: "组会/文献报告", hint: "背景·方法·panel-grid 结果·参考文献" },
  { id: "defense", label: "答辩/会议", hint: "click 渐进·统计表·图版·致谢" },
  { id: "figures", label: "投稿配图/PPT", hint: "figure 布局·300 DPI 导出就绪" },
  { id: "blank", label: "空白", hint: "仅封面页" },
];

export function DeckSidebarView(): React.ReactNode {
  const workbench = useWorkbenchState();
  const group = getGroup(workbench.groups, workbench.groups.active);
  const active = group?.editors.find(e => e.id === group.activeId && e.kind === "deck");
  const activeId = active?.params.deckId as string | undefined;
  const [tab, setTab] = useState<"decks" | "slides">("decks");
  const [payload, setPayload] = useState<DeckPayload | null>(null);
  const [currentSlide, setCurrentSlide] = useState(1);
  useEffect(() => {
    let cancelled = false;
    setPayload(null); setCurrentSlide(Number((active?.state as { slideNo?: number } | null)?.slideNo) || 1);
    const refreshOutline = () => { if (activeId) void fetchDeck(activeId).then(p => { if (!cancelled) setPayload(p); }).catch(() => {}); };
    refreshOutline();
    const timer = window.setInterval(refreshOutline, 4000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [activeId]);
  useEffect(() => {
    const handler = (event: Event) => { const d = (event as CustomEvent<{deckId: string; slide: number}>).detail; if (d.deckId === activeId) setCurrentSlide(d.slide); };
    window.addEventListener("deck-slide-changed", handler);
    return () => window.removeEventListener("deck-slide-changed", handler);
  }, [activeId]);
  const [decks, setDecks] = useState<DeckSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<"name" | "recent">("recent");
  const [error, setError] = useState<string | null>(null);
  const shown = decks.filter(d => d.title.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())).sort((a, b) => sort === "name" ? a.title.localeCompare(b.title, undefined, { numeric: true }) : Date.parse(b.updatedAt) - Date.parse(a.updatedAt));
  const [dialogOpen, setDialogOpen] = useState(false);
  const [title, setTitle] = useState("");
  const [template, setTemplate] = useState<DeckTemplate>("meeting");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await fetchDeckList();
      setDecks(list); setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const submit = useCallback(async () => {
    const trimmed = title.trim();
    if (!trimmed || creating) return;
    setCreating(true);
    setCreateError(null);
    try {
      const payload = await createDeck({ title: trimmed, template });
      setDialogOpen(false);
      setTitle("");
      await refresh();
      openDeck(payload.deck.id, { title: payload.deck.title, forceNew: false });
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  }, [title, template, creating, refresh]);

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-deck-sidebar="">
      <div className="flex h-10 shrink-0 items-center gap-0.5 border-b px-3">
        <span className="flex-1 truncate text-xs font-semibold tracking-wide text-muted-foreground">幻灯片</span>
        <Button size="icon-sm" variant="ghost" aria-label="刷新幻灯片" title="刷新" onClick={() => void refresh()}><RefreshCw /></Button>
        <Button size="icon-sm" variant="ghost" aria-label="新建幻灯片" title="新建" onClick={() => setDialogOpen(true)}><Plus /></Button>
        <ViewLocationMenu id="decks" primaryOnly />
      </div>
      <div className="flex border-b px-2" role="tablist" aria-label="幻灯片侧栏">
        <button role="tab" aria-selected={tab === "decks"} className="px-3 py-2 text-xs aria-selected:border-b-2 aria-selected:border-primary" onClick={() => setTab("decks")}>演示文稿</button>
        <button role="tab" aria-selected={tab === "slides"} className="px-3 py-2 text-xs aria-selected:border-b-2 aria-selected:border-primary" onClick={() => setTab("slides")}>页面目录</button>
      </div>
      {tab === "slides" ? <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">{!activeId ? <p className="px-2 py-3 text-xs text-muted-foreground">打开演示文稿以查看页面目录。</p> : !payload ? <p className="px-2 py-3 text-xs text-muted-foreground">加载页面目录…</p> : <DeckOutline entries={outlineOrPages(payload.outline, payload.slides)} currentSlide={currentSlide} onNavigate={slide => window.dispatchEvent(new CustomEvent("deck-go-slide", { detail: { deckId: activeId, slide } }))} />}</ScrollArea> : <>
      <div className="shrink-0 px-3 pt-3"><div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input aria-label="搜索幻灯片" placeholder="搜索幻灯片" value={query} onChange={e => setQuery(e.target.value)} onKeyDown={e => { if (e.key === "Escape") setQuery(""); }} className="h-8 pl-8 text-sm" />
      </div></div>
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-4 text-xs text-muted-foreground">
        <span>{query.trim() ? `${shown.length} 条结果` : `已显示 ${decks.length} 个演示文稿`}</span>
        <button type="button" className="shrink-0 hover:text-foreground" aria-label="切换幻灯片排序" onClick={() => setSort(sort === "name" ? "recent" : "name")}>{sort === "name" ? "名称 (A to Z)" : "最近更新"} ↕</button>
      </div>
      <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">
        {error && <div role="alert" className="px-2 py-2 text-xs text-destructive">加载失败：{error} <button type="button" className="underline" onClick={() => void refresh()}>重试</button></div>}
        {loading ? <div className="px-2 py-3 text-xs text-muted-foreground">加载中…</div> : shown.length === 0 ? <div className="px-2 py-3 text-xs text-muted-foreground">{query.trim() ? "没有匹配的演示文稿。" : error ? "无法加载演示文稿。" : "还没有演示文稿。"}</div> :
          <div role="list" aria-label="演示文稿列表">{shown.map(deck => <div role="listitem" key={deck.id}>
            <button type="button" className="flex min-h-7 w-full items-start gap-1 rounded-sm px-1 py-1 text-left text-[13px] leading-5 text-muted-foreground hover:bg-accent hover:text-foreground" title={`${deck.title} · ${deck.slideCount} 页 · ${formatTime(deck.updatedAt)}`} onClick={() => openDeck(deck.id, { title: deck.title })}>
              <Presentation className="mt-0.5 size-4 shrink-0 opacity-50" /><span className="min-w-0 flex-1 break-words">{deck.title || "未命名演示文稿"}</span><span className="shrink-0 text-xs opacity-70">{deck.slideCount} 页</span>
            </button>
          </div>)}</div>}
      </ScrollArea>
      </>}

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>新建 deck</DialogTitle>
            <DialogDescription>选择生物医学模板,生成后直接打开编辑标签。</DialogDescription>
          </DialogHeader>
          <Input
            autoFocus
            placeholder="deck 标题(如:组会 · 单细胞图谱)"
            value={title}
            onChange={e => setTitle(e.target.value)}
            onKeyDown={e => {
              if (e.key === "Enter") void submit();
            }}
          />
          <div className="flex flex-col gap-1" role="radiogroup" aria-label="模板">
            {TEMPLATES.map(t => (
              <button
                key={t.id}
                type="button"
                role="radio"
                aria-checked={template === t.id}
                className={`rounded-md border px-3 py-2 text-left text-xs ${template === t.id ? "border-primary bg-primary/5" : "border-transparent hover:bg-sidebar-accent"}`}
                onClick={() => setTemplate(t.id)}
              >
                <span className="block font-medium">{t.label}</span>
                <span className="block text-[10px] text-muted-foreground">{t.hint}</span>
              </button>
            ))}
          </div>
          {createError ? <div className="text-xs text-destructive">{createError}</div> : null}
          <DialogFooter>
            <Button size="sm" variant="ghost" onClick={() => setDialogOpen(false)}>
              取消
            </Button>
            <Button size="sm" disabled={!title.trim() || creating} onClick={() => void submit()}>
              {creating ? <Loader2 className="size-3.5 animate-spin" /> : null}
              创建
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function formatTime(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const diff = Date.now() - t;
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return new Date(t).toLocaleDateString();
}
