import { createContext, useContext, useState, type ReactNode } from "react";
import { Check, Plus, X, Paperclip, FileText, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { contextKindLabel, resourceKey, type ContextResource } from "./context-model";

type Basket = {
  resources: ContextResource[];
  available: boolean;
  revealVersion: number;
  contextLabel: string;
  add: (resource: ContextResource) => void;
  remove: (resource: ContextResource) => void;
  clear: () => void;
  clearSent: (sent: ContextResource[]) => void;
  forScope: (scope: string | null, label: string) => Basket;
};
const BasketContext = createContext<Basket | null>(null);
export const useContextBasket = () => useContext(BasketContext);

/** Kept above the module views, scoped to the agent AND context. Browsing
 * never implicitly adds resources. Snapshots stay stable until re-added. */
export function ContextBasketProvider({ scope, contextLabel, onAdd, children }: {
  scope: string | null; contextLabel: string; onAdd: () => void; children: ReactNode;
}) {
  const [revealVersion, setRevealVersion] = useState(0);
  const [baskets, setBaskets] = useState<Record<string, ContextResource[]>>({});
  const forScope = (targetScope: string | null, label: string): Basket => {
    const resources = targetScope ? baskets[targetScope] ?? [] : [];
    const update = (transform: (current: ContextResource[]) => ContextResource[]) => {
      if (targetScope) setBaskets(current => ({ ...current, [targetScope]: transform(current[targetScope] ?? []) }));
    };
    return {
      resources, available: Boolean(targetScope), contextLabel: label, revealVersion, forScope,
      add: resource => {
        update(current => [...current.filter(item => resourceKey(item) !== resourceKey(resource)), resource]);
        setRevealVersion(version => version + 1);
        onAdd();
      },
      remove: resource => update(current => current.filter(item => resourceKey(item) !== resourceKey(resource))),
      clear: () => update(() => []),
      clearSent: sent => update(current => current.filter(item => !sent.includes(item))),
    };
  };
  return <BasketContext.Provider value={forScope(scope, contextLabel)}>{children}</BasketContext.Provider>;
}

/** Select another agent's basket while sharing the same underlying store. */
export function ContextBasketScope({ scope, label, children }: { scope: string | null; label: string; children: ReactNode }) {
  const parent = useContext(BasketContext);
  return <BasketContext.Provider value={parent?.forScope(scope, label) ?? null}>{children}</BasketContext.Provider>;
}

export function AddToContext({ resource }: { resource: ContextResource }) {
  const basket = useContextBasket();
  if (!basket) return null;
  const added = basket.resources.some(item => resourceKey(item) === resourceKey(resource));
  return <Button size="xs" variant="outline" disabled={!basket.available}
    title={basket.available ? `${resource.scope} · 加入 ${basket.contextLabel}` : "请先选择 Agent 上下文"}
    onClick={() => basket.add(resource)}>
    {added ? <Check data-icon="inline-start" /> : <Plus data-icon="inline-start" />}
    {added ? "更新会话引用" : "加入当前会话"}
  </Button>;
}

export function ContextBasket({ actions }: { actions?: ReactNode }) {
  const basket = useContextBasket();
  const [preview, setPreview] = useState<ContextResource | null>(null);
  if (!basket) return null;
  return <section aria-label="上下文篮子" className="flex min-w-0 shrink-0 flex-col gap-2 px-3 py-2">
    <div className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
      <Paperclip className="size-3.5 shrink-0" aria-hidden="true" />
      <span className="shrink-0">会话引用</span><Badge variant="secondary" className="h-4 min-w-4 px-1 text-[10px] tabular-nums" aria-live="polite">{basket.resources.length}</Badge>
      <span className="flex-1" />
      {basket.resources.length > 0 && <Button size="icon-xs" variant="ghost" aria-label="清空引用" title="清空引用" onClick={basket.clear}><Trash2 /></Button>}
    </div>
    {actions && <div className="min-w-0 max-w-full [&>[data-agent-context-binding]]:w-full">{actions}</div>}
    {basket.resources.length > 0 && <ul className="flex max-h-48 min-w-0 flex-col gap-1.5 overflow-y-auto" aria-label="已附加的引用">
      {basket.resources.map(resource => <li key={resourceKey(resource)} className="group flex min-w-0 items-start overflow-hidden rounded-lg border bg-muted/20 transition-colors hover:bg-muted/40">
        <button type="button" aria-label={`${contextKindLabel[resource.kind]} ${resource.title}`} className="flex min-w-0 flex-1 items-start gap-2 rounded-md p-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring" title={`预览：${resource.title}`} onClick={() => setPreview(resource)}>
          <span className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md border bg-background text-muted-foreground"><FileText className="size-3.5" aria-hidden="true" /></span>
          <span className="min-w-0 flex-1">
            <span className="line-clamp-2 text-xs leading-5 [overflow-wrap:anywhere]">{resource.title}</span>
            <span className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[10px] text-muted-foreground"><span className="shrink-0">{contextKindLabel[resource.kind]}</span><span aria-hidden="true">·</span><span className="truncate">{resource.scope}</span></span>
          </span>
        </button>
        <Button size="icon-xs" variant="ghost" className="mt-1.5 mr-1 shrink-0 text-muted-foreground" aria-label={`移除引用：${resource.title}`} title="移除引用" onClick={() => basket.remove(resource)}><X /></Button>
      </li>)}
    </ul>}
    <Dialog open={Boolean(preview)} onOpenChange={open => { if (!open) setPreview(null); }}>
      <DialogContent className="max-h-[85dvh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle className="pr-6 leading-relaxed [overflow-wrap:anywhere]">{preview?.title}</DialogTitle><DialogDescription>{preview?.scope}</DialogDescription></DialogHeader>
        <p className="break-all text-xs text-muted-foreground">来源：{preview?.source}{preview?.revision ? ` · 版本：${preview.revision}` : ""}</p>
        <pre className="whitespace-pre-wrap text-sm leading-relaxed [overflow-wrap:anywhere]">{preview?.content}</pre>
      </DialogContent>
    </Dialog>
  </section>;
}
