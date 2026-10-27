import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

export function SearchDialog({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    return () => dialog.close();
  }, []);
  return <dialog ref={ref} aria-label={title} onCancel={event => { event.preventDefault(); onClose(); }} className="fixed inset-0 m-auto max-h-[85vh] w-[min(760px,95vw)] overflow-y-auto rounded-lg border bg-background p-4 text-foreground shadow-xl backdrop:bg-black/40">
    <div className="mb-3 flex items-center justify-between gap-3"><h2 className="font-semibold">{title}</h2><Button size="xs" variant="ghost" aria-label={`Close ${title}`} onClick={onClose}>Close</Button></div>
    {children}
  </dialog>;
}
