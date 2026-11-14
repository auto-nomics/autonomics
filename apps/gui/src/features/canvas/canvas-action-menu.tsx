import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ChevronDown, type LucideIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Keep action components mounted so closing the popup never discards their dialogs. */
export function CanvasActionMenu({ label, icon: Icon, children }: { label: string; icon: LucideIcon; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLDivElement>(null);
  const id = useId();
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, []);
  return <div ref={root} data-canvas-action-menu={label} className="relative" onKeyDown={event => {
    if (event.key === "Escape") { setOpen(false); trigger.current?.querySelector("button")?.focus(); }
  }}>
    <div ref={trigger}><Button size="sm" variant={open ? "secondary" : "ghost"} aria-label={label} aria-expanded={open} aria-controls={id} onClick={() => setOpen(value => !value)}><Icon />{label}<ChevronDown className="size-3 opacity-60" /></Button></div>
    <div id={id} role="group" aria-label={`${label}操作`} className={cn("canvas-action-popup absolute bottom-full right-0 mb-2 max-h-[min(65vh,34rem)] w-60 max-w-[calc(100cqw-1.5rem)] overflow-y-auto rounded-lg p-2", !open && "hidden")} onClick={event => {
      if (event.target instanceof Element && event.target.closest("button")) setOpen(false);
    }}>{children}</div>
  </div>;
}
export function CanvasActionGroup({ title, children }: { title: string; children: ReactNode }) {
  return <div className="canvas-action-group">
    <p className="canvas-action-heading">{title}</p>
    <div className="canvas-action-items grid grid-cols-1">{children}</div>
  </div>;
}
