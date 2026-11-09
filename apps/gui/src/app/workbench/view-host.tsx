import { useCallback, useLayoutEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { useWorkbenchViews, type WorkbenchViewProps } from "./view-registry";

const slots = new Map<string, HTMLDivElement>();
const visited = new Set<string>();
const listeners = new Set<() => void>();
let revision = 0;
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const snapshot = () => revision;
function emit() { revision++; for (const listener of listeners) listener(); }

export function WorkbenchViewSlot({ id }: { id: string }) {
  const owned = useRef<HTMLDivElement | null>(null);
  const ref = useCallback((node: HTMLDivElement | null) => {
    if (node) { slots.set(id, node); visited.add(id); }
    else if (slots.get(id) === owned.current) slots.delete(id);
    owned.current = node;
    emit();
  }, [id]);
  return <div ref={ref} data-view-slot={id} className="flex min-h-0 min-w-0 flex-1 flex-col" />;
}

function ViewHost({ id, body: Body, ...props }: WorkbenchViewProps & { id: string; body: React.ComponentType<WorkbenchViewProps> }) {
  const host = useMemo(() => {
    const node = document.createElement("div");
    node.className = "flex min-h-0 min-w-0 flex-1 flex-col";
    node.dataset.viewHost = id;
    return node;
  }, [id]);
  useSyncExternalStore(subscribe, snapshot);
  const slot = slots.get(id);
  useLayoutEffect(() => {
    if (slot && host.parentElement !== slot) slot.appendChild(host);
    else if (!slot) host.remove();
  }, [slot, host]);
  useLayoutEffect(() => () => { host.remove(); }, [host]);
  return createPortal(<Body {...props} />, host);
}

/** A View's React instance survives slot changes between workbench regions. */
export function WorkbenchViewHosts(props: WorkbenchViewProps) {
  const views = useWorkbenchViews();
  useSyncExternalStore(subscribe, snapshot);
  return <>{views.filter(view => visited.has(view.id)).map(view => <ViewHost key={view.id} id={view.id} body={view.body} {...props} />)}</>;
}
