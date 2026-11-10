import { createContext, useContext, useState, type ReactNode } from "react";
import { Check, Plus, X, Paperclip } from "lucide-react";
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
  return <section aria-label="上下文篮子" className="flex shrink-0 flex-col gap-1 px-3 py-1.5">
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs">
      <Paperclip className="size-3.5 shrink-0" />
      <span>会话引用</span><Badge variant="secondary" aria-live="polite">{basket.resources.length}</Badge>
      <span className="flex-1" />
      {actions}
      {basket.resources.length > 0 && <Button size="xs" variant="ghost" onClick={basket.clear}>清空引用</Button>}
    </div>
    {basket.resources.length > 0 && <ul className="flex max-h-32 flex-col gap-1 overflow-y-auto">
      {basket.resources.map(resource => <li key={resourceKey(resource)} className="flex min-w-0 items-center gap-1">
        <Button size="xs" variant="secondary" className="min-w-0 flex-1 justify-start" title={`预览：${resource.title}`} onClick={() => setPreview(resource)}>
          <span className="shrink-0">{contextKindLabel[resource.kind]}</span><span className="truncate">{resource.title}</span>
        </Button>
        <Button size="icon-xs" variant="ghost" aria-label={`移除引用：${resource.title}`} onClick={() => basket.remove(resource)}><X /></Button>
      </li>)}
    </ul>}
    <Dialog open={Boolean(preview)} onOpenChange={open => { if (!open) setPreview(null); }}>
      <DialogContent className="max-h-[85dvh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle>{preview?.title}</DialogTitle><DialogDescription>{preview?.scope}</DialogDescription></DialogHeader>
        <p className="break-all text-xs text-muted-foreground">来源：{preview?.source}{preview?.revision ? ` · 版本：${preview.revision}` : ""}</p>
        <pre className="whitespace-pre-wrap break-words text-sm">{preview?.content}</pre>
      </DialogContent>
    </Dialog>
  </section>;
}
