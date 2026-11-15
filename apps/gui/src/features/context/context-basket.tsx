import { createContext, useContext, useMemo, useState, type ReactNode } from "react";
import { Check, Plus, X, Paperclip } from "lucide-react";
import { Button } from "@/components/ui/button";
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
  // `value` must be memoized: the provider sits above the whole workbench,
  // which re-renders on every streaming token, and a fresh basket object per
  // render would re-render every useContextBasket consumer with it.
  const value = useMemo<Basket>(() => {
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
    return forScope(scope, contextLabel);
  }, [baskets, revealVersion, scope, contextLabel, onAdd]);
  return <BasketContext.Provider value={value}>{children}</BasketContext.Provider>;
}

/** Select another agent's basket while sharing the same underlying store. */
export function ContextBasketScope({ scope, label, children }: { scope: string | null; label: string; children: ReactNode }) {
  const parent = useContext(BasketContext);
  const value = useMemo(() => parent?.forScope(scope, label) ?? null, [parent, scope, label]);
  return <BasketContext.Provider value={value}>{children}</BasketContext.Provider>;
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
  return <section aria-label="上下文篮子" className="flex min-w-0 shrink-0 flex-col gap-1.5 px-3 py-1">
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      {actions && <div className="min-w-0 max-w-full">{actions}</div>}
      {basket.resources.map(resource => (
        <span key={resourceKey(resource)} className="inline-flex h-6 min-w-0 max-w-56 items-center rounded-md bg-secondary text-xs text-muted-foreground">
          <button type="button" aria-label={`${contextKindLabel[resource.kind]} ${resource.title}`} className="flex h-full min-w-0 items-center gap-1 rounded-sm pl-2 pr-1 outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring" title={`预览：${resource.title} · ${resource.scope}`} onClick={() => setPreview(resource)}>
            <Paperclip className="size-3 shrink-0" aria-hidden="true" />
            <span className="truncate">{resource.title}</span>
          </button>
          <button type="button" className="flex size-6 shrink-0 items-center justify-center rounded-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring" aria-label={`移除引用：${resource.title}`} title="移除引用" onClick={() => basket.remove(resource)}><X className="size-3" aria-hidden="true" /></button>
        </span>
      ))}
    </div>
    <Dialog open={Boolean(preview)} onOpenChange={open => { if (!open) setPreview(null); }}>
      <DialogContent className="max-h-[85dvh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle className="pr-6 leading-relaxed [overflow-wrap:anywhere]">{preview?.title}</DialogTitle><DialogDescription>{preview?.scope}</DialogDescription></DialogHeader>
        <p className="break-all text-xs text-muted-foreground">来源：{preview?.source}{preview?.revision ? ` · 版本：${preview.revision}` : ""}</p>
        <pre className="whitespace-pre-wrap text-sm leading-relaxed [overflow-wrap:anywhere]">{preview?.content}</pre>
      </DialogContent>
    </Dialog>
  </section>;
}
