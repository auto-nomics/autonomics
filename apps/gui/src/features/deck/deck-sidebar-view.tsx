/**
 * DeckSidebarView — the "decks" sidebar container: deck list (title /
 * slide count / updated time), click opens the deck tab; "新建" opens a
 * title + template dialog. Wiki-sidebar shape, deck-sized.
 */
import { useCallback, useEffect, useState } from "react";
import { Loader2, Plus, Presentation, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { createDeck, fetchDeckList, type DeckSummary, type DeckTemplate } from "./deck-api";
import { openDeck } from "./deck-state";

const TEMPLATES: readonly { id: DeckTemplate; label: string; hint: string }[] = [
  { id: "meeting", label: "组会/文献报告", hint: "背景·方法·panel-grid 结果·参考文献" },
  { id: "defense", label: "答辩/会议", hint: "click 渐进·统计表·图版·致谢" },
  { id: "figures", label: "投稿配图/PPT", hint: "figure 布局·300 DPI 导出就绪" },
  { id: "blank", label: "空白", hint: "仅封面页" },
];

export function DeckSidebarView(): React.ReactNode {
  const [decks, setDecks] = useState<DeckSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [title, setTitle] = useState("");
  const [template, setTemplate] = useState<DeckTemplate>("meeting");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await fetchDeckList();
      setDecks(list);
    } catch {
      // transient — keep the last list
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
      <div className="flex shrink-0 items-center gap-1 px-2 py-1.5">
        <span className="flex-1 text-xs text-muted-foreground">{decks.length} 个 deck</span>
        <Button size="sm" variant="ghost" className="size-6 p-0" title="刷新" onClick={() => void refresh()}>
          <RefreshCw className="size-3.5" />
        </Button>
        <Button size="sm" variant="secondary" className="gap-1 px-2" onClick={() => setDialogOpen(true)}>
          <Plus className="size-3.5" /> 新建
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
        {loading ? (
          <div className="flex items-center gap-2 px-2 py-4 text-xs text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" /> 加载…
          </div>
        ) : decks.length === 0 ? (
          <div className="px-2 py-4 text-xs text-muted-foreground">
            还没有 deck。点“新建”,或让 agent 用 deck_create 生成。
          </div>
        ) : (
          decks.map(deck => (
            <button
              key={deck.id}
              type="button"
              className="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left hover:bg-sidebar-accent"
              title={`${deck.title} · ${deck.slideCount} 页`}
              onClick={() => openDeck(deck.id, { title: deck.title })}
            >
              <Presentation className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-xs">{deck.title || "未命名 deck"}</span>
                <span className="block text-[10px] text-muted-foreground">
                  {deck.slideCount} 页 · {formatTime(deck.updatedAt)}
                </span>
              </span>
            </button>
          ))
        )}
      </div>

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
