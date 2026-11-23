import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ImageIcon, Paperclip, X } from "lucide-react";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { contextKindLabel, resourceKey, type ContextResource } from "./context-model";

export function DraftContextAttachments({ resources, onRemove }: { resources: ContextResource[]; onRemove: (resource: ContextResource) => void }) {
  const { t } = useTranslation("context");
  const [preview, setPreview] = useState<ContextResource | null>(null);
  if (!resources.length) return null;
  return <section aria-label={t("draftAttachments.sectionLabel")} className="flex flex-wrap gap-1 px-2.5 pt-2">
    {resources.map(resource => <span key={resourceKey(resource)} className="inline-flex h-6 min-w-0 max-w-56 items-center rounded-md bg-secondary text-xs text-muted-foreground">
      <button type="button" className="flex h-full min-w-0 items-center gap-1 rounded-sm pl-2 pr-1 outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring" title={t("draftAttachments.previewTitle", { title: resource.title })} onClick={() => setPreview(resource)}>
        {resource.kind === "image" ? <ImageIcon className="size-3 shrink-0" /> : <Paperclip className="size-3 shrink-0" />}
        <span className="truncate">{resource.title}</span>
      </button>
      <button type="button" className="flex size-6 shrink-0 items-center justify-center rounded-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring" aria-label={t("draftAttachments.removeAttachment", { title: resource.title })} onClick={() => { onRemove(resource); if (preview === resource) setPreview(null); }}><X className="size-3" /></button>
    </span>)}
    <Dialog open={Boolean(preview)} onOpenChange={open => { if (!open) setPreview(null); }}>
      <DialogContent className="max-h-[85dvh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle className="pr-6 break-words">{preview?.title}</DialogTitle><DialogDescription>{preview && contextKindLabel[preview.kind]} · {preview?.scope}</DialogDescription></DialogHeader>
        {preview?.kind === "image" ? <img src={preview.content} alt={preview.title} className="max-h-[65dvh] max-w-full object-contain" /> : <pre className="whitespace-pre-wrap break-words text-xs" data-draft-context-content>{preview?.content}</pre>}
      </DialogContent>
    </Dialog>
  </section>;
}
