import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Eye, FileText, Link2, MousePointer2, Unlink, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { ContextResource } from "@/features/context/context-model";
import { editorKind } from "./editor-registry";
import { allGroups, getWorkbenchState, useWorkbenchState } from "./workbench-store";

/** The host owns one binding per agent, including before its first session.
 * Resolve the document again on send: document models may change independently
 * of the workbench layout and must never be captured in a stale render. */
export function useAgentContextBinding() {
  const { t } = useTranslation("workbench");
  const workbench = useWorkbenchState();
  const [mode, setMode] = useState("");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const candidates = allGroups(workbench.groups).flatMap(([, group]) => group.editors)
    .filter(editor => Boolean(editorKind(editor.kind)?.agentContext));

  useEffect(() => {
    if (mode && mode !== "$follow" && !candidates.some(editor => editor.id === mode)) setMode("");
  }, [mode, workbench.groups]);

  const getResource = (): ContextResource | null => {
    const current = getWorkbenchState();
    const id = mode === "$follow" ? current.lastContextEditorId : mode;
    const editor = allGroups(current.groups).flatMap(([, group]) => group.editors).find(editor => editor.id === id);
    const kind = editor && editorKind(editor.kind);
    const snapshot = editor && kind?.agentContext?.(editor);
    if (!editor || !kind || !snapshot) return null;
    return {
      kind: "editor", id: editor.id, title: kind.title(editor), source: editor.id,
      scope: typeof snapshot.scope === "string" ? snapshot.scope : "文档当前状态（每次发送时更新）",
      revision: String(current.revision), content: JSON.stringify(snapshot, null, 2),
    };
  };
  const resource = getResource();
  const label = mode === "$follow" ? t("contextBinding.followLabel", { title: resource?.title ?? t("contextBinding.noDoc") }) : resource?.title ?? t("contextBinding.defaultLabel");
  const choose = (value: string) => { setMode(value); setPickerOpen(false); };

  const control = <div className="flex min-w-0 max-w-full items-center gap-0.5" data-agent-context-binding="">
    <Button size="xs" variant="secondary" className="min-w-0 max-w-56 justify-start rounded-md font-normal text-muted-foreground"
      aria-label={t("contextBinding.bindAria")} title={mode ? t("contextBinding.changeTitle", { label }) : t("contextBinding.pickTitle")}
      onClick={() => setPickerOpen(true)}>
      {mode === "$follow" ? <MousePointer2 data-icon="inline-start" /> : <Link2 data-icon="inline-start" />}
      <span className="min-w-0 truncate">{label}</span>
    </Button>
    {resource && <Button size="icon-xs" variant="ghost" aria-label={t("contextBinding.previewAria")} title={t("contextBinding.previewTitle")} onClick={() => setPreviewOpen(true)}><Eye /></Button>}
    {mode && <Button size="icon-xs" variant="ghost" aria-label={t("contextBinding.unbindAria")} title={t("contextBinding.unbindAria")} onClick={() => setMode("")}><X /></Button>}
    <Dialog open={pickerOpen} onOpenChange={setPickerOpen}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("contextBinding.dialogTitle")}</DialogTitle>
          <DialogDescription>{t("contextBinding.dialogDescription")}</DialogDescription>
        </DialogHeader>
        <Command>
          <CommandInput placeholder={t("contextBinding.searchPlaceholder")} />
          <CommandList>
            <CommandEmpty>{t("contextBinding.empty")}</CommandEmpty>
            <CommandGroup heading={t("contextBinding.modeHeading")}>
              <CommandItem value={t("contextBinding.unbind")} data-checked={!mode} onSelect={() => choose("")}><Unlink /><span>{t("contextBinding.unbind")}</span></CommandItem>
              <CommandItem value={t("contextBinding.follow")} data-checked={mode === "$follow"} onSelect={() => choose("$follow")}>
                <MousePointer2 /><div className="flex min-w-0 flex-1 flex-col gap-0.5"><span>{t("contextBinding.follow")}</span><span className="truncate text-xs text-muted-foreground">{resource && mode === "$follow" ? resource.title : t("contextBinding.followDesc")}</span></div>
              </CommandItem>
            </CommandGroup>
            <CommandGroup heading={t("contextBinding.pinHeading")}>
              {candidates.map(editor => {
                const kind = editorKind(editor.kind)!;
                const Icon = kind.icon?.(editor) ?? FileText;
                return <CommandItem key={editor.id} value={`${kind.title(editor)} ${editor.id}`} data-checked={mode === editor.id} onSelect={() => choose(editor.id)}>
                  <Icon /><span className="min-w-0 flex-1 truncate">{kind.title(editor)}</span>
                </CommandItem>;
              })}
              {!candidates.length && <p className="px-2 py-3 text-xs text-muted-foreground">{t("contextBinding.candidatesEmpty")}</p>}
            </CommandGroup>
          </CommandList>
        </Command>
      </DialogContent>
    </Dialog>
    <Dialog open={previewOpen && Boolean(resource)} onOpenChange={setPreviewOpen}>
      <DialogContent className="max-h-[80dvh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle>{resource?.title}</DialogTitle><DialogDescription>{resource?.scope}</DialogDescription></DialogHeader>
        <pre className="whitespace-pre-wrap break-words text-xs">{resource?.content}</pre>
      </DialogContent>
    </Dialog>
  </div>;
  return { control, getResource };
}
