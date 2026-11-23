import { ViewLocationMenu } from "@/app/workbench/view-location-menu";
/**
 * DeckSidebarView — the "decks" sidebar container: deck list (title /
 * slide count / updated time), click opens the deck tab; "新建" opens a
 * title + template dialog. Wiki-sidebar shape, deck-sized.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus, Presentation, RefreshCw, Search } from "lucide-react";
import "./i18n";
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
import { i18next } from "./i18n";
import { openDeck } from "./deck-state";
import { DeckOutline, outlineOrPages } from "./deck-outline";
import { getGroup, useWorkbenchState } from "@/app/workbench/workbench-store";
import type { DeckPayload } from "./deck-api";

const TEMPLATES: readonly { id: DeckTemplate; labelKey: string; hintKey: string }[] = [
  { id: "meeting", labelKey: "templates.meeting", hintKey: "templates.meetingHint" },
  { id: "defense", labelKey: "templates.defense", hintKey: "templates.defenseHint" },
  { id: "figures", labelKey: "templates.figures", hintKey: "templates.figuresHint" },
  { id: "blank", labelKey: "templates.blank", hintKey: "templates.blankHint" },
];

export function DeckSidebarView(): React.ReactNode {
  const { t } = useTranslation("deck");
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
        <span className="flex-1 truncate text-xs font-semibold tracking-wide text-muted-foreground">{t("sidebar.header")}</span>
        <Button size="icon-sm" variant="ghost" aria-label={t("sidebar.refreshAria")} title={t("sidebar.refresh")} onClick={() => void refresh()}><RefreshCw /></Button>
        <Button size="icon-sm" variant="ghost" aria-label={t("sidebar.newAria")} title={t("sidebar.new")} onClick={() => setDialogOpen(true)}><Plus /></Button>
        <ViewLocationMenu id="decks" primaryOnly />
      </div>
      <div className="flex border-b px-2" role="tablist" aria-label={t("sidebar.tabsAria")}>
        <button role="tab" aria-selected={tab === "decks"} className="px-3 py-2 text-xs aria-selected:border-b-2 aria-selected:border-primary" onClick={() => setTab("decks")}>{t("sidebar.decksTab")}</button>
        <button role="tab" aria-selected={tab === "slides"} className="px-3 py-2 text-xs aria-selected:border-b-2 aria-selected:border-primary" onClick={() => setTab("slides")}>{t("sidebar.slidesTab")}</button>
      </div>
      {tab === "slides" ? <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">{!activeId ? <p className="px-2 py-3 text-xs text-muted-foreground">{t("sidebar.openDeckHint")}</p> : !payload ? <p className="px-2 py-3 text-xs text-muted-foreground">{t("sidebar.loadingOutline")}</p> : <DeckOutline entries={outlineOrPages(payload.outline, payload.slides)} currentSlide={currentSlide} onNavigate={slide => window.dispatchEvent(new CustomEvent("deck-go-slide", { detail: { deckId: activeId, slide } }))} />}</ScrollArea> : <>
      <div className="shrink-0 px-3 pt-3"><div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input aria-label={t("sidebar.searchAria")} placeholder={t("sidebar.searchPlaceholder")} value={query} onChange={e => setQuery(e.target.value)} onKeyDown={e => { if (e.key === "Escape") setQuery(""); }} className="h-8 pl-8 text-sm" />
      </div></div>
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-4 text-xs text-muted-foreground">
        <span>{query.trim() ? t("sidebar.resultsCount", { count: shown.length }) : t("sidebar.shownCount", { count: decks.length })}</span>
        <button type="button" className="shrink-0 hover:text-foreground" aria-label={t("sidebar.sortAria")} onClick={() => setSort(sort === "name" ? "recent" : "name")}>{sort === "name" ? t("sidebar.sortName") : t("sidebar.sortRecent")} ↕</button>
      </div>
      <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">
        {error && <div role="alert" className="px-2 py-2 text-xs text-destructive">{t("sidebar.loadFailed", { error })} <button type="button" className="underline" onClick={() => void refresh()}>{t("editor.retry")}</button></div>}
        {loading ? <div className="px-2 py-3 text-xs text-muted-foreground">{t("sidebar.loading")}</div> : shown.length === 0 ? <div className="px-2 py-3 text-xs text-muted-foreground">{query.trim() ? t("sidebar.noMatch") : error ? t("sidebar.loadUnavailable") : t("sidebar.empty")}</div> :
          <div role="list" aria-label={t("sidebar.listAria")}>{shown.map(deck => <div role="listitem" key={deck.id}>
            <button type="button" className="flex min-h-7 w-full items-start gap-1 rounded-sm px-1 py-1 text-left text-[13px] leading-5 text-muted-foreground hover:bg-accent hover:text-foreground" title={`${deck.title} · ${t("sidebar.pages", { count: deck.slideCount })} · ${formatTime(deck.updatedAt)}`} onClick={() => openDeck(deck.id, { title: deck.title })}>
              <Presentation className="mt-0.5 size-4 shrink-0 opacity-50" /><span className="min-w-0 flex-1 break-words">{deck.title || t("sidebar.untitled")}</span><span className="shrink-0 text-xs opacity-70">{t("sidebar.pages", { count: deck.slideCount })}</span>
            </button>
          </div>)}</div>}
      </ScrollArea>
      </>}

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("sidebar.dialogTitle")}</DialogTitle>
            <DialogDescription>{t("sidebar.dialogDescription")}</DialogDescription>
          </DialogHeader>
          <Input
            autoFocus
            placeholder={t("sidebar.titlePlaceholder")}
            value={title}
            onChange={e => setTitle(e.target.value)}
            onKeyDown={e => {
              if (e.key === "Enter") void submit();
            }}
          />
          <div className="flex flex-col gap-1" role="radiogroup" aria-label={t("sidebar.templateAria")}>
            {TEMPLATES.map(tpl => (
              <button
                key={tpl.id}
                type="button"
                role="radio"
                aria-checked={template === tpl.id}
                className={`rounded-md border px-3 py-2 text-left text-xs ${template === tpl.id ? "border-primary bg-primary/5" : "border-transparent hover:bg-sidebar-accent"}`}
                onClick={() => setTemplate(tpl.id)}
              >
                <span className="block font-medium">{t(tpl.labelKey)}</span>
                <span className="block text-[10px] text-muted-foreground">{t(tpl.hintKey)}</span>
              </button>
            ))}
          </div>
          {createError ? <div className="text-xs text-destructive">{createError}</div> : null}
          <DialogFooter>
            <Button size="sm" variant="ghost" onClick={() => setDialogOpen(false)}>
              {t("sidebar.cancel")}
            </Button>
            <Button size="sm" disabled={!title.trim() || creating} onClick={() => void submit()}>
              {creating ? <Loader2 className="size-3.5 animate-spin" /> : null}
              {t("sidebar.create")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function formatTime(iso: string): string {
  const time = Date.parse(iso);
  if (Number.isNaN(time)) return "";
  const { t } = i18next;
  const diff = Date.now() - time;
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return t("deck:sidebar.justNow");
  if (minutes < 60) return t("deck:sidebar.minutesAgo", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("deck:sidebar.hoursAgo", { count: hours });
  return new Date(time).toLocaleDateString();
}
