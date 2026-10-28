import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

export function SearchDialog({ title, onClose, children, footer }: { title: string; onClose: () => void; children: ReactNode; footer?: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    return () => dialog.close();
  }, []);
  return <dialog ref={ref} aria-label={title} onCancel={event => { event.preventDefault(); onClose(); }} className="fixed inset-0 m-auto max-h-[85vh] w-[min(760px,95vw)] overflow-hidden rounded-lg border bg-background p-0 text-foreground shadow-xl backdrop:bg-black/40">
    <div className="flex max-h-[calc(85vh-2px)] flex-col">
      <div className="flex shrink-0 items-center justify-between gap-3 border-b px-4 py-3"><h2 className="font-semibold">{title}</h2><Button size="xs" variant="ghost" aria-label={`Close ${title}`} onClick={onClose}>Close</Button></div>
      <div className="min-h-0 overflow-y-auto p-4">{children}</div>
      {footer && <div className="shrink-0 border-t p-4">{footer}</div>}
    </div>
  </dialog>;
}
